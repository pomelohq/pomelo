use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{
    HeaderMap, HeaderName, HeaderValue, CONNECTION, CONTENT_TYPE, HOST, SET_COOKIE,
};
use hyper::{Request, Response, StatusCode, Uri};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};

use crate::routing::{rewrite_external_cookie, rewrite_local_cookie, Route, Router};
use crate::{clock, ProxyLog, ProxyLogEntry};

pub(crate) type Body = BoxBody<Bytes, hyper::Error>;

const WEBHOOK_BODY_CAP: usize = 32 << 20;
const WEBHOOK_FORWARD_TIMEOUT: Duration = Duration::from_secs(20);
const WEBHOOK_HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const LOCAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const EXTERNAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const LISTENING_PROBE_TIMEOUT: Duration = Duration::from_millis(300);
const HOP_HEADERS: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

pub(crate) struct Shared {
    pub router: Arc<Router>,
    pub log: Arc<ProxyLog>,
    local: Client<HttpConnector, Body>,
    external: Client<hyper_tls::HttpsConnector<HttpConnector>, Body>,
}

impl Shared {
    pub fn new(router: Arc<Router>, log: Arc<ProxyLog>) -> Shared {
        let mut local = HttpConnector::new();
        local.set_connect_timeout(Some(LOCAL_CONNECT_TIMEOUT));
        let mut external = HttpConnector::new();
        external.set_connect_timeout(Some(EXTERNAL_CONNECT_TIMEOUT));
        external.enforce_http(false);
        Shared {
            router,
            log,
            local: Client::builder(TokioExecutor::new())
                .pool_idle_timeout(Duration::from_secs(30))
                .pool_max_idle_per_host(24)
                .build(local),
            external: Client::builder(TokioExecutor::new())
                .pool_idle_timeout(Duration::from_secs(30))
                .pool_max_idle_per_host(12)
                .build(hyper_tls::HttpsConnector::new_with_connector(external)),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Proxy,
    Webhook,
}

pub(crate) async fn accept(listener: TcpListener, shared: Arc<Shared>, kind: Kind) {
    loop {
        let (stream, client) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                eprintln!("proxy: accept: {error}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let shared = shared.clone();
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |request| {
                let shared = shared.clone();
                async move {
                    Ok::<_, Infallible>(match kind {
                        Kind::Proxy => handle_proxy(&shared, client, request).await,
                        Kind::Webhook => handle_webhook(&shared, request).await,
                    })
                }
            });
            let mut builder = hyper::server::conn::http1::Builder::new();
            if matches!(kind, Kind::Webhook) {
                builder
                    .timer(TokioTimer::new())
                    .header_read_timeout(WEBHOOK_HEADER_TIMEOUT);
            }
            // A client hanging up mid-response is routine for a dev server; nothing to report.
            let _ = builder
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades()
                .await;
        });
    }
}

fn full(bytes: impl Into<Bytes>) -> Body {
    Full::new(bytes.into())
        .map_err(|never| match never {})
        .boxed()
}

fn text_response(status: u16, message: &str) -> Response<Body> {
    let mut response = Response::new(full(format!("{message}\n")));
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn is_upgrade(headers: &HeaderMap) -> bool {
    headers.contains_key(hyper::header::UPGRADE)
        && headers
            .get_all(CONNECTION)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| {
                value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
            })
}

/// Drops hop-by-hop headers (and any the Connection header names); an upgrade keeps its handshake pair.
fn strip_hop_headers(headers: &mut HeaderMap, keep_upgrade: bool) {
    let named: Vec<String> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| {
            value
                .split(',')
                .map(|token| token.trim().to_ascii_lowercase())
        })
        .filter(|token| !token.is_empty())
        .collect();
    for name in named.iter().map(String::as_str).chain(HOP_HEADERS) {
        if keep_upgrade && (name == "connection" || name == "upgrade") {
            continue;
        }
        headers.remove(name);
    }
}

enum Upstream {
    Local(u16),
    External { scheme: String, authority: String },
}

async fn handle_proxy(
    shared: &Shared,
    client: SocketAddr,
    request: Request<Incoming>,
) -> Response<Body> {
    if !shared.router.has_projects() {
        return text_response(503, "dev proxy not configured");
    }
    let started = Instant::now();
    let host = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| request.uri().host().map(str::to_string))
        .unwrap_or_default();
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(str::to_string);
    let method = request.method().to_string();
    let router = shared.router.clone();
    let (lookup_host, lookup_path) = (host, path.clone());
    let decision = match tokio::task::spawn_blocking(move || {
        router.decide(&lookup_host, &lookup_path)
    })
    .await
    {
        Ok(decision) => decision,
        Err(error) => return text_response(500, &format!("dev-proxy: {error}")),
    };
    let response = match decision.route {
        Route::Error { status, message } => text_response(status, &message),
        Route::Local { port, prefix } => {
            forward(
                shared,
                request,
                Upstream::Local(port),
                &decision.path,
                query.as_deref(),
                &prefix,
                client,
            )
            .await
        }
        Route::External { url, prefix } => match url.parse::<Uri>() {
            Ok(uri) if uri.authority().is_some() => {
                let upstream = Upstream::External {
                    scheme: uri.scheme_str().unwrap_or("http").to_string(),
                    authority: uri
                        .authority()
                        .map(|authority| authority.to_string())
                        .unwrap_or_default(),
                };
                forward(
                    shared,
                    request,
                    upstream,
                    &decision.path,
                    query.as_deref(),
                    &prefix,
                    client,
                )
                .await
            }
            _ => text_response(502, &format!("dev-proxy: bad env override URL: {url}")),
        },
    };
    if let Some(logged) = decision.logged {
        shared.log.add(ProxyLogEntry {
            time: clock(),
            method,
            path,
            repo: logged.repo,
            service: logged.service,
            profile: logged.profile,
            target: logged.target,
            status: response.status().as_u16(),
            ms: started.elapsed().as_millis() as u64,
        });
    }
    response
}

async fn forward(
    shared: &Shared,
    mut request: Request<Incoming>,
    upstream: Upstream,
    path: &str,
    query: Option<&str>,
    prefix: &str,
    client: SocketAddr,
) -> Response<Body> {
    let upgrade = is_upgrade(request.headers());
    let path_and_query = match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    let (scheme, authority, external) = match &upstream {
        Upstream::Local(port) => ("http".to_string(), format!("127.0.0.1:{port}"), false),
        Upstream::External { scheme, authority } => (scheme.clone(), authority.clone(), true),
    };
    let direct = upgrade && !external;
    let target = if direct {
        path_and_query.clone()
    } else {
        format!("{scheme}://{authority}{path_and_query}")
    };
    let uri: Uri = match target.parse() {
        Ok(uri) => uri,
        Err(error) => return text_response(502, &format!("dev-proxy: bad upstream URL: {error}")),
    };
    let client_upgrade = upgrade.then(|| hyper::upgrade::on(&mut request));
    let (mut parts, body) = request.into_parts();
    parts.uri = uri;
    strip_hop_headers(&mut parts.headers, upgrade);
    if external {
        if let Ok(value) = HeaderValue::from_str(&authority) {
            parts.headers.insert(HOST, value);
        }
    }
    let forwarded_for = HeaderName::from_static("x-forwarded-for");
    let chain = match parts
        .headers
        .get(&forwarded_for)
        .and_then(|value| value.to_str().ok())
    {
        Some(prior) => format!("{prior}, {}", client.ip()),
        None => client.ip().to_string(),
    };
    if let Ok(value) = HeaderValue::from_str(&chain) {
        parts.headers.insert(forwarded_for, value);
    }
    let outgoing = Request::from_parts(parts, body.boxed());
    let result = if direct {
        send_direct(&authority, outgoing).await
    } else if external {
        shared
            .external
            .request(outgoing)
            .await
            .map_err(|error| error.to_string())
    } else {
        shared
            .local
            .request(outgoing)
            .await
            .map_err(|error| error.to_string())
    };
    let mut response = match result {
        Ok(response) => response,
        Err(error) if external => {
            return text_response(
                502,
                &format!("dev-proxy: remote env not reachable ({error})"),
            )
        }
        Err(error) => {
            return text_response(
                502,
                &format!("dev-proxy: backend not reachable - is the service running? ({error})"),
            )
        }
    };
    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        if let Some(client_upgrade) = client_upgrade {
            let upstream_upgrade = hyper::upgrade::on(&mut response);
            tokio::spawn(async move {
                match tokio::try_join!(client_upgrade, upstream_upgrade) {
                    Ok((client_io, upstream_io)) => {
                        let mut client_io = TokioIo::new(client_io);
                        let mut upstream_io = TokioIo::new(upstream_io);
                        // Either side closing ends the tunnel; that is the normal way out.
                        let _ =
                            tokio::io::copy_bidirectional(&mut client_io, &mut upstream_io).await;
                    }
                    Err(error) => eprintln!("dev-proxy: upgrade failed: {error}"),
                }
            });
        }
        let (parts, _) = response.into_parts();
        return Response::from_parts(parts, full(Bytes::new()));
    }
    strip_hop_headers(response.headers_mut(), false);
    let cookies: Vec<String> = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|cookie| {
            if external {
                rewrite_external_cookie(cookie, prefix)
            } else {
                rewrite_local_cookie(cookie, prefix)
            }
        })
        .collect();
    if !cookies.is_empty() {
        response.headers_mut().remove(SET_COOKIE);
        for cookie in cookies {
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                response.headers_mut().append(SET_COOKIE, value);
            }
        }
    }
    response.map(BodyExt::boxed)
}

/// An upgrade needs its own connection: a pooled one could not be handed over to the tunnel.
async fn send_direct(
    authority: &str,
    request: Request<Body>,
) -> Result<Response<Incoming>, String> {
    let stream = tokio::time::timeout(LOCAL_CONNECT_TIMEOUT, TcpStream::connect(authority))
        .await
        .map_err(|_| "connect timed out".to_string())?
        .map_err(|error| error.to_string())?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|error| error.to_string())?;
    tokio::spawn(async move {
        if let Err(error) = connection.with_upgrades().await {
            eprintln!("dev-proxy: upstream connection: {error}");
        }
    });
    sender
        .send_request(request)
        .await
        .map_err(|error| error.to_string())
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

async fn listening(port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            LISTENING_PROBE_TIMEOUT,
            TcpStream::connect(("127.0.0.1", port))
        )
        .await,
        Ok(Ok(_))
    )
}

async fn handle_webhook(shared: &Shared, request: Request<Incoming>) -> Response<Body> {
    if !shared.router.has_projects() {
        return text_response(503, "relay not configured");
    }
    let path = request.uri().path().to_string();
    let trimmed = path.strip_prefix('/').unwrap_or(&path);
    let mut parts = trimmed.splitn(3, '/');
    let repo = parts.next().unwrap_or_default().to_string();
    let service = parts.next().unwrap_or_default().to_string();
    let forward_path = format!("/{}", parts.next().unwrap_or_default());
    if repo.is_empty() || service.is_empty() {
        return no_webhook_route(&path);
    }
    let target = format!("{repo}/{service}");
    let router = shared.router.clone();
    let lookup = target.clone();
    let Ok(Some((_, ports))) =
        tokio::task::spawn_blocking(move || router.service_ports_everywhere(&lookup)).await
    else {
        return no_webhook_route(&path);
    };
    let query = request.uri().query().map(str::to_string);
    let method = request.method().clone();
    let (parts, body) = request.into_parts();
    let body = match Limited::new(body, WEBHOOK_BODY_CAP).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            eprintln!("webhook relay: reading the body: {error}");
            Bytes::new()
        }
    };
    let mut live = Vec::new();
    for port in ports {
        if listening(port).await {
            live.push(port);
        }
    }
    let mut response = Response::new(full(format!(
        "{{\"ok\":true,\"service\":{},\"fanout\":{}}}",
        json_string(&target),
        live.len()
    )));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let client = shared.local.clone();
    let mut headers = parts.headers;
    headers.remove(HOST);
    tokio::spawn(async move {
        for port in live {
            let url = match &query {
                Some(query) => format!("http://127.0.0.1:{port}{forward_path}?{query}"),
                None => format!("http://127.0.0.1:{port}{forward_path}"),
            };
            let Ok(uri) = url.parse::<Uri>() else {
                continue;
            };
            let mut outgoing = Request::new(full(body.clone()));
            *outgoing.method_mut() = method.clone();
            *outgoing.uri_mut() = uri;
            *outgoing.headers_mut() = headers.clone();
            match tokio::time::timeout(WEBHOOK_FORWARD_TIMEOUT, client.request(outgoing)).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => eprintln!("webhook fanout > :{port} {forward_path}: {error}"),
                Err(_) => eprintln!("webhook fanout > :{port} {forward_path}: timed out"),
            }
        }
    });
    response
}

fn no_webhook_route(path: &str) -> Response<Body> {
    text_response(
        404,
        &format!("no webhook route for {path} (use /<repo>/<service>)"),
    )
}
