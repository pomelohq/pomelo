use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{
    HeaderMap, HeaderName, HeaderValue, CACHE_CONTROL, CONNECTION, CONTENT_TYPE, HOST, SET_COOKIE,
};
use hyper::{Request, Response, StatusCode, Uri};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};

use crate::routing::{
    host_name, rewrite_external_cookie, rewrite_local_cookie, rewrite_public_cookie, Problem,
    ProblemKind, Route, Router,
};
use crate::{
    capture_of, clock, finish, record, Capture, Delivery, GithubDelivery, GithubHook, ProxyLog,
    ProxyLogEntry, RequestKind, GITHUB_PATH,
};

pub(crate) type Body = BoxBody<Bytes, hyper::Error>;

/// Passes a body through untouched while copying what flows into `capture`.
struct Tee<B> {
    inner: B,
    capture: Capture,
}

impl<B> hyper::body::Body for Tee<B>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut();
        let polled = std::pin::Pin::new(&mut this.inner).poll_frame(context);
        match &polled {
            std::task::Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    record(&this.capture, data);
                }
            }
            std::task::Poll::Ready(None) => finish(&this.capture),
            _ => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        let ended = self.inner.is_end_stream();
        if ended {
            finish(&self.capture);
        }
        ended
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

fn header_list(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let shown = if crate::sensitive_header(name.as_str()) {
                crate::REDACTED.to_string()
            } else {
                String::from_utf8_lossy(value.as_bytes()).into_owned()
            };
            (name.as_str().to_string(), shown)
        })
        .collect()
}

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
    network: pom_env::DevNetwork,
    token: Option<String>,
    github: std::sync::RwLock<Option<GithubHook>>,
    local: Client<HttpConnector, Body>,
    external: Client<hyper_tls::HttpsConnector<HttpConnector>, Body>,
}

impl Shared {
    pub fn new(
        router: Arc<Router>,
        log: Arc<ProxyLog>,
        network: pom_env::DevNetwork,
        token: Option<String>,
    ) -> Shared {
        let mut local = HttpConnector::new();
        local.set_connect_timeout(Some(LOCAL_CONNECT_TIMEOUT));
        let mut external = HttpConnector::new();
        external.set_connect_timeout(Some(EXTERNAL_CONNECT_TIMEOUT));
        external.enforce_http(false);
        Shared {
            router,
            log,
            network,
            token,
            github: std::sync::RwLock::new(None),
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

impl Shared {
    pub fn set_github(&self, hook: GithubHook) {
        if let Ok(mut current) = self.github.write() {
            *current = Some(hook);
        }
    }

    fn github(&self) -> Option<GithubHook> {
        self.github.read().ok()?.clone()
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Proxy,
    Webhook,
}

#[derive(Clone, Copy)]
pub(crate) struct Peer {
    address: SocketAddr,
    /// Connected from this machine: loopback, or one of its own addresses (a service calling a public URL).
    local: bool,
}

fn same_machine(client: SocketAddr, local: Option<SocketAddr>) -> bool {
    let client = client.ip().to_canonical();
    client.is_loopback() || local.is_some_and(|local| local.ip().to_canonical() == client)
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
        let peer = Peer {
            address: client,
            local: same_machine(client, stream.local_addr().ok()),
        };
        let shared = shared.clone();
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |request| {
                let shared = shared.clone();
                async move {
                    Ok::<_, Infallible>(match kind {
                        Kind::Proxy => handle_proxy(&shared, peer, request).await,
                        Kind::Webhook => handle_webhook(&shared, peer, request).await,
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

/// Marks a response the proxy made itself because the backend took no connection, so a browser can get a page.
const UNREACHABLE: &str = "x-pom-unreachable";

/// A browser navigating to the page gets an explanation it can act on; scripts, `fetch` and curl keep the text.
fn wants_page(headers: &HeaderMap) -> bool {
    let navigation = headers
        .get("sec-fetch-mode")
        .is_some_and(|mode| mode.as_bytes() == b"navigate");
    let html = headers
        .get(hyper::header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));
    navigation || html
}

fn error_response(status: u16, message: &str, problem: &Problem, page: bool) -> Response<Body> {
    if !page {
        return text_response(status, message);
    }
    let mut response = Response::new(full(crate::error_page::render(status, message, problem)));
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
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

/// What the request looked like to the browser: the `X-Forwarded-Host`/`-Proto` sent upstream, and how
/// response cookies are rewritten.
struct Forwarding {
    peer: SocketAddr,
    host: String,
    proto: String,
    cookies: CookiePolicy,
}

enum CookiePolicy {
    /// A `localhost` name: the long-standing rewrite.
    Local,
    Public {
        host: String,
        https: bool,
    },
}

impl Forwarding {
    /// Forwarded headers are believed only from a trusted proxy; anyone else could forge them.
    fn of(
        headers: &HeaderMap,
        host: &str,
        peer: SocketAddr,
        network: &pom_env::DevNetwork,
    ) -> Forwarding {
        let trusted = network.trusts(peer.ip());
        let claimed = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
                .map(str::trim)
                .filter(|value| trusted && !value.is_empty())
                .map(str::to_string)
        };
        let host = claimed("x-forwarded-host").unwrap_or_else(|| host.to_string());
        let proto = claimed("x-forwarded-proto")
            .map(|proto| proto.to_ascii_lowercase())
            .unwrap_or_else(|| "http".to_string());
        let name = host_name(&host).to_ascii_lowercase();
        let local = network.public().is_local()
            || name == crate::routing::DOMAIN
            || name.ends_with(&format!(".{}", crate::routing::DOMAIN));
        let cookies = if local {
            CookiePolicy::Local
        } else {
            CookiePolicy::Public {
                host: name,
                https: network.https,
            }
        };
        Forwarding {
            peer,
            host,
            proto,
            cookies,
        }
    }

    fn rewrite_cookie(&self, cookie: &str, prefix: &str, external: bool) -> String {
        match &self.cookies {
            CookiePolicy::Local if external => rewrite_external_cookie(cookie, prefix),
            CookiePolicy::Local => rewrite_local_cookie(cookie, prefix),
            CookiePolicy::Public { host, https } => {
                rewrite_public_cookie(cookie, prefix, external, host, *https)
            }
        }
    }
}

enum Upstream {
    Local(SocketAddr),
    External { scheme: String, authority: String },
}

fn request_host<B>(request: &Request<B>) -> String {
    request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| request.uri().host().map(str::to_string))
        .unwrap_or_default()
}

/// `/_pom/` on the proxy is pom's own, never a service's: a page elsewhere must not reach it.
fn handle_control(
    shared: &Shared,
    peer: Peer,
    request: &Request<Incoming>,
    host: &str,
) -> Response<Body> {
    if crate::access::cross_site(request.headers(), host) {
        return text_response(
            403,
            "dev-proxy: pom's own endpoints do not take requests from web pages",
        );
    }
    if request.uri().path() != crate::faults::RELOAD_PATH {
        return text_response(404, "dev-proxy: no such pom endpoint");
    }
    if !is_fault_reload(request, peer.address) {
        return text_response(
            405,
            "dev-proxy: `pom proxy fault` reloads the rules here (POST from this machine)",
        );
    }
    let loaded = shared.router.reload_faults();
    match serde_json::to_string(&loaded) {
        Ok(body) => json_response(200, body),
        Err(error) => text_response(500, &format!("dev-proxy: {error}")),
    }
}

/// What a request from another machine must show: the token by header or cookie, or once in the query, which
/// trades it for a cookie so the links a reviewer opens keep working.
fn check_token<B>(
    token: Option<&str>,
    network: &pom_env::DevNetwork,
    local: bool,
    request: &mut Request<B>,
    host: &str,
) -> Result<Option<String>, Box<Response<Body>>> {
    let query = request.uri().query().map(str::to_string);
    let Some(token) = token else {
        return Ok(query);
    };
    let presented = crate::access::presented(request.headers());
    crate::access::strip_token(request.headers_mut());
    let (rest, from_query) = crate::access::split_query(query.as_deref());
    if local {
        return Ok(rest);
    }
    if presented.is_some_and(|candidate| crate::access::token_matches(&candidate, token)) {
        return Ok(rest);
    }
    if from_query.is_some_and(|candidate| crate::access::token_matches(&candidate, token))
        && request.method() == hyper::Method::GET
    {
        let location = match &rest {
            Some(rest) => format!("{}?{rest}", request.uri().path()),
            None => request.uri().path().to_string(),
        };
        let mut response = text_response(303, "dev-proxy: signed in");
        let cookie = crate::access::token_cookie(token, host, network);
        for (name, value) in [(hyper::header::LOCATION, location), (SET_COOKIE, cookie)] {
            if let Ok(value) = HeaderValue::from_str(&value) {
                response.headers_mut().insert(name, value);
            }
        }
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return Err(Box::new(response));
    }
    let message = "dev-proxy: this address needs the proxy token. Open the link once with ?pom_token=<token> added (`pom proxy token` shows it), or send it in the X-Pom-Token header";
    let mut response = text_response(401, message);
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Err(Box::new(response))
}

async fn handle_proxy(
    shared: &Shared,
    peer: Peer,
    mut request: Request<Incoming>,
) -> Response<Body> {
    let host = request_host(&request);
    if !crate::access::host_allowed(&host, &shared.network) {
        return text_response(
            421,
            &format!("dev-proxy: {host:?} is not a name this proxy serves (use <service>.<repo>.<workspace>.{} or localhost)", shared.network.domain),
        );
    }
    let query = match check_token(
        shared.token.as_deref(),
        &shared.network,
        peer.local,
        &mut request,
        &host,
    ) {
        Ok(query) => query,
        Err(response) => return *response,
    };
    if request.uri().path().starts_with(CONTROL_PREFIX) {
        return handle_control(shared, peer, &request, &host);
    }
    if !shared.router.has_projects() {
        return text_response(503, "dev proxy not configured");
    }
    let client = peer.address;
    let page = wants_page(request.headers());
    let started = Instant::now();
    let path = request.uri().path().to_string();
    let method = request.method().to_string();
    let request_headers = header_list(request.headers());
    let navigation = request
        .headers()
        .get("sec-fetch-mode")
        .is_some_and(|mode| mode.as_bytes() == b"navigate");
    let forwarding = Forwarding::of(request.headers(), &host, client, &shared.network);
    let workspace_label = crate::routing::host_labels(&host, shared.router.domain())
        .and_then(|labels| labels.last().cloned())
        .unwrap_or_default();
    let request_capture = Capture::default();
    let (request_parts, request_body) = request.into_parts();
    let mut request = Request::from_parts(
        request_parts,
        Tee {
            inner: request_body,
            capture: request_capture.clone(),
        },
    );
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
    if let Some(value) = decision
        .upstream_host
        .as_deref()
        .and_then(|host| HeaderValue::from_str(host).ok())
    {
        request.headers_mut().insert(HOST, value);
    }
    let fault = decision.fault.clone();
    if let Some(delay) = fault
        .as_ref()
        .map(|fault| fault.delay_ms)
        .filter(|ms| *ms > 0)
    {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
    let injected = fault
        .as_ref()
        .and_then(|fault| fault.status.map(|status| (status, fault.id.clone())));
    let response = match (injected, decision.route) {
        (Some((status, id)), _) => text_response(
            status,
            &format!("dev-proxy: fault {id} injected by `pom proxy fault` (remove it with `pom proxy fault rm {id}`)"),
        ),
        (
            None,
            Route::Error {
                status,
                message,
                problem,
            },
        ) => error_response(status, &message, &problem, page),
        (None, Route::Local { address, prefix }) => {
            let mut response = forward(
                shared,
                request,
                Upstream::Local(address),
                &decision.path,
                query.as_deref(),
                &prefix,
                &forwarding,
            )
            .await;
            if response.headers_mut().remove(UNREACHABLE).is_some() && page {
                let problem = Problem {
                    kind: ProblemKind::Unreachable,
                    target: decision
                        .logged
                        .as_ref()
                        .map(|logged| format!("{}/{}", logged.repo, logged.service))
                        .unwrap_or_default(),
                    workspace: workspace_label.clone(),
                    elsewhere: String::new(),
                };
                response = error_response(
                    502,
                    &format!("dev-proxy: backend not reachable at {address}"),
                    &problem,
                    true,
                );
            }
            response
        }
        (None, Route::External { url, prefix }) => match url.parse::<Uri>() {
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
                    &forwarding,
                )
                .await
            }
            _ => text_response(502, &format!("dev-proxy: bad env override URL: {url}")),
        },
    };
    let response_headers = header_list(response.headers());
    let response_capture = Capture::default();
    let (response_parts, response_body) = response.into_parts();
    let response = Response::from_parts(
        response_parts,
        Tee {
            inner: response_body,
            capture: response_capture.clone(),
        }
        .boxed(),
    );
    let worth_logging = |logged: &crate::routing::Logged| {
        !logged.host_routed || navigation || fault.is_some() || response.status().as_u16() >= 400
    };
    if let Some(logged) = decision.logged.filter(worth_logging) {
        let logged_path = match &query {
            Some(query) => format!("{path}?{query}"),
            None => path,
        };
        shared.log.add(
            ProxyLogEntry {
                kind: RequestKind::Proxy,
                time: clock(),
                method,
                path: logged_path,
                repo: logged.repo,
                service: logged.service,
                profile: logged.profile,
                target: logged.target,
                status: response.status().as_u16(),
                ms: started.elapsed().as_millis() as u64,
                request_headers,
                response_headers,
                fault: fault.map(|fault| fault.id).unwrap_or_default(),
                ..ProxyLogEntry::default()
            },
            request_capture,
            response_capture,
        );
    }
    response
}

async fn forward(
    shared: &Shared,
    mut request: Request<Tee<Incoming>>,
    upstream: Upstream,
    path: &str,
    query: Option<&str>,
    prefix: &str,
    forwarding: &Forwarding,
) -> Response<Body> {
    let upgrade = is_upgrade(request.headers());
    let path_and_query = match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    let (scheme, authority, external) = match &upstream {
        Upstream::Local(address) => ("http".to_string(), address.to_string(), false),
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
        Some(prior) => format!("{prior}, {}", forwarding.peer.ip()),
        None => forwarding.peer.ip().to_string(),
    };
    if let Ok(value) = HeaderValue::from_str(&chain) {
        parts.headers.insert(forwarded_for, value);
    }
    for (name, value) in [
        ("x-forwarded-host", forwarding.host.as_str()),
        ("x-forwarded-proto", forwarding.proto.as_str()),
    ] {
        match HeaderValue::from_str(value) {
            Ok(value) => {
                parts.headers.insert(HeaderName::from_static(name), value);
            }
            Err(_) => {
                parts.headers.remove(name);
            }
        }
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
            if let Upstream::Local(address) = upstream {
                shared.router.forget(address);
            }
            let mut response = text_response(
                502,
                &format!("dev-proxy: backend not reachable - is the service running? ({error})"),
            );
            response
                .headers_mut()
                .insert(UNREACHABLE, HeaderValue::from_static("1"));
            return response;
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
        .map(|cookie| forwarding.rewrite_cookie(cookie, prefix, external))
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

/// The loopback address a workspace's service answers on, if any.
async fn listening(port: u16) -> Option<SocketAddr> {
    for ip in [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ] {
        let address = SocketAddr::new(ip, port);
        if let Ok(Ok(_)) =
            tokio::time::timeout(LISTENING_PROBE_TIMEOUT, TcpStream::connect(address)).await
        {
            return Some(address);
        }
    }
    None
}

async fn handle_webhook(
    shared: &Shared,
    peer: Peer,
    mut request: Request<Incoming>,
) -> Response<Body> {
    let host = request_host(&request);
    if crate::access::cross_site(request.headers(), &host) {
        return text_response(
            403,
            "relay: webhooks come from servers, not from another web page",
        );
    }
    if request.uri().path().trim_end_matches('/') == GITHUB_PATH {
        return handle_github(shared, request).await;
    }
    let query = match relay_token(shared.token.as_deref(), peer.local, &mut request) {
        Ok(query) => query,
        Err(response) => return *response,
    };
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
    let method = request.method().clone();
    let (parts, body) = request.into_parts();
    let body = match Limited::new(body, WEBHOOK_BODY_CAP).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            eprintln!("webhook relay: reading the body: {error}");
            Bytes::new()
        }
    };
    let started = Instant::now();
    let time = clock();
    let request_headers = header_list(&parts.headers);
    let request_capture = capture_of(&body);
    let mut live = Vec::new();
    for (workspace, port) in ports {
        if let Some(address) = listening(port).await {
            live.push((workspace, address));
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
    let response_headers = header_list(response.headers());
    let ack = format!(
        "{{\"ok\":true,\"service\":{},\"fanout\":{}}}",
        json_string(&target),
        live.len()
    );
    let response_capture = capture_of(ack.as_bytes());
    let client = shared.local.clone();
    let log = shared.log.clone();
    let mut headers = parts.headers;
    headers.remove(HOST);
    let logged_path = match &query {
        Some(query) => format!("{forward_path}?{query}"),
        None => forward_path.clone(),
    };
    tokio::spawn(async move {
        let mut deliveries = Vec::new();
        for (workspace, address) in live {
            let url = format!("http://{address}{logged_path}");
            let Ok(uri) = url.parse::<Uri>() else {
                continue;
            };
            let mut outgoing = Request::new(full(body.clone()));
            *outgoing.method_mut() = method.clone();
            *outgoing.uri_mut() = uri;
            *outgoing.headers_mut() = headers.clone();
            let mut delivery = Delivery {
                workspace,
                port: address.port(),
                ..Delivery::default()
            };
            let sent = Instant::now();
            match tokio::time::timeout(WEBHOOK_FORWARD_TIMEOUT, client.request(outgoing)).await {
                Ok(Ok(answer)) => delivery.status = Some(answer.status().as_u16()),
                Ok(Err(error)) => delivery.error = error.to_string(),
                Err(_) => delivery.error = "timed out".into(),
            }
            delivery.ms = sent.elapsed().as_millis() as u64;
            deliveries.push(delivery);
        }
        let failed = deliveries
            .iter()
            .any(|delivery| delivery.status.is_none_or(|status| status >= 400));
        log.add(
            ProxyLogEntry {
                kind: RequestKind::Webhook,
                time,
                method: method.to_string(),
                path: logged_path,
                repo,
                service,
                profile: String::new(),
                target: format!("{} workspaces", deliveries.len()),
                status: if deliveries.is_empty() {
                    404
                } else if failed {
                    502
                } else {
                    200
                },
                ms: started.elapsed().as_millis() as u64,
                deliveries,
                request_headers,
                response_headers,
                ..ProxyLogEntry::default()
            },
            request_capture,
            response_capture,
        );
    });
    response
}

/// A provider cannot sign in, so from another machine the relay takes the token in the URL it is given (or the
/// header); `/_pom/github` checks GitHub's own signature instead.
fn relay_token<B>(
    token: Option<&str>,
    local: bool,
    request: &mut Request<B>,
) -> Result<Option<String>, Box<Response<Body>>> {
    let query = request.uri().query().map(str::to_string);
    let Some(token) = token else {
        return Ok(query);
    };
    let presented = request
        .headers()
        .get(crate::access::TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string());
    request.headers_mut().remove(crate::access::TOKEN_HEADER);
    let (rest, from_query) = crate::access::split_query(query.as_deref());
    let shown = presented
        .or(from_query)
        .is_some_and(|candidate| crate::access::token_matches(&candidate, token));
    if local || shown {
        return Ok(rest);
    }
    Err(Box::new(text_response(
        401,
        "relay: from another machine, add ?pom_token=<token> to the webhook URL (`pom proxy token` shows it) or send the X-Pom-Token header",
    )))
}

fn no_webhook_route(path: &str) -> Response<Body> {
    text_response(
        404,
        &format!("no webhook route for {path} (use /<repo>/<service>)"),
    )
}

const CONTROL_PREFIX: &str = "/_pom/";

fn is_fault_reload(request: &Request<Incoming>, client: SocketAddr) -> bool {
    request.method() == hyper::Method::POST
        && request.uri().path() == crate::faults::RELOAD_PATH
        && request
            .headers()
            .get(crate::faults::RELOAD_HEADER)
            .is_some_and(|value| value.as_bytes() == b"faults")
        && client.ip().to_canonical().is_loopback()
}

fn json_response(status: u16, body: String) -> Response<Body> {
    let mut response = Response::new(full(body));
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

/// `/_pom/github`: hands the delivery to the hook (which checks the signature) and logs what it decided.
async fn handle_github(shared: &Shared, request: Request<Incoming>) -> Response<Body> {
    let started = Instant::now();
    let time = clock();
    let Some(hook) = shared.github() else {
        return text_response(404, "GitHub sync is not served by this process");
    };
    if request.method() != hyper::Method::POST {
        return text_response(405, "POST a GitHub webhook delivery here");
    }
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let mut delivery = GithubDelivery {
        event: header("x-github-event"),
        id: header("x-github-delivery"),
        signature: header("x-hub-signature-256"),
        content_type: header("content-type"),
        body: Vec::new(),
    };
    let method = request.method().to_string();
    let request_headers = header_list(request.headers());
    let (_, body) = request.into_parts();
    match Limited::new(body, WEBHOOK_BODY_CAP).collect().await {
        Ok(collected) => delivery.body = collected.to_bytes().to_vec(),
        Err(error) => {
            return text_response(
                413,
                &format!("the delivery could not be read whole ({error})"),
            )
        }
    }
    let request_capture = capture_of(&delivery.body);
    let routes = shared.router.routes();
    let event = delivery.event.clone();
    let reply = match tokio::task::spawn_blocking(move || hook(&delivery, &routes)).await {
        Ok(reply) => reply,
        Err(error) => return text_response(500, &format!("relay: {error}")),
    };
    let response = json_response(reply.status, reply.body.clone());
    shared.log.add(
        ProxyLogEntry {
            kind: RequestKind::Webhook,
            time,
            method,
            path: GITHUB_PATH.to_string(),
            repo: reply.repo,
            service: format!("github {event}"),
            target: reply.summary,
            status: reply.status,
            ms: started.elapsed().as_millis() as u64,
            request_headers,
            response_headers: header_list(response.headers()),
            ..ProxyLogEntry::default()
        },
        request_capture,
        capture_of(reply.body.as_bytes()),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123";

    fn network() -> pom_env::DevNetwork {
        pom_env::DevNetwork::new(
            pom_env::DevPorts::default(),
            &pom_env::ProxySettings {
                bind: "100.64.0.10".into(),
                domain: "dev.example.net".into(),
                public_scheme: "https".into(),
                ..pom_env::ProxySettings::default()
            },
        )
    }

    fn get(uri: &str, headers: &[(&'static str, &'static str)]) -> Request<()> {
        let mut request = Request::builder().uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.body(()).unwrap_or_default()
    }

    fn header(response: &Response<Body>, name: hyper::header::HeaderName) -> String {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    const HOST: &str = "web.web.main.dev.example.net";

    #[test]
    fn another_machine_signs_in_once_by_link_then_by_cookie() {
        let network = network();
        let check =
            |request: &mut Request<()>| check_token(Some(TOKEN), &network, false, request, HOST);

        let mut bare = get("/orders?page=2", &[]);
        let refused = check(&mut bare).err().map(|response| *response);
        assert_eq!(refused.as_ref().map(|r| r.status().as_u16()), Some(401));

        let mut wrong = get("/orders", &[("x-pom-token", "nope")]);
        assert!(check(&mut wrong).is_err());

        let mut link = get(&format!("/orders?page=2&pom_token={TOKEN}"), &[]);
        let Err(redirect) = check(&mut link) else {
            panic!("a link with the token signs in");
        };
        assert_eq!(redirect.status().as_u16(), 303);
        assert_eq!(header(&redirect, hyper::header::LOCATION), "/orders?page=2");
        let cookie = header(&redirect, SET_COOKIE);
        assert!(
            cookie.starts_with(&format!("pom_token={TOKEN}; Path=/; HttpOnly")),
            "{cookie}"
        );
        assert!(cookie.contains("Domain=dev.example.net") && cookie.ends_with("Secure"));

        let cookie_header = format!("sid=1; pom_token={TOKEN}");
        let mut with_cookie = get("/orders?page=2", &[]);
        with_cookie.headers_mut().insert(
            hyper::header::COOKIE,
            HeaderValue::from_str(&cookie_header).unwrap_or(HeaderValue::from_static("")),
        );
        assert_eq!(check(&mut with_cookie).ok(), Some(Some("page=2".into())));
        assert_eq!(header_of(&with_cookie, "cookie"), "sid=1");

        let mut script = get("/orders", &[]);
        script
            .headers_mut()
            .insert(crate::access::TOKEN_HEADER, HeaderValue::from_static(TOKEN));
        assert_eq!(check(&mut script).ok(), Some(None));
        assert!(script.headers().get(crate::access::TOKEN_HEADER).is_none());

        let mut post_link = get(&format!("/orders?pom_token={TOKEN}"), &[]);
        *post_link.method_mut() = hyper::Method::POST;
        assert!(check(&mut post_link).is_err_and(|response| response.status() == 401));
    }

    fn header_of(request: &Request<()>, name: &str) -> String {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn this_machine_needs_no_token_but_never_passes_it_on() {
        let network = network();
        let mut request = get(
            &format!("/x?a=1&pom_token={TOKEN}"),
            &[("x-pom-token", "nope")],
        );
        assert_eq!(
            check_token(Some(TOKEN), &network, true, &mut request, HOST).ok(),
            Some(Some("a=1".into()))
        );
        assert!(request.headers().get(crate::access::TOKEN_HEADER).is_none());
        let mut open = get("/x?a=1", &[]);
        assert_eq!(
            check_token(None, &network, false, &mut open, HOST).ok(),
            Some(Some("a=1".into()))
        );
    }

    #[test]
    fn the_relay_takes_the_token_in_its_url_from_another_machine() {
        let mut bare = get("/api/server/hook", &[]);
        assert!(relay_token(Some(TOKEN), false, &mut bare).is_err_and(|r| r.status() == 401));
        let mut in_url = get(&format!("/api/server/hook?pom_token={TOKEN}&x=1"), &[]);
        assert_eq!(
            relay_token(Some(TOKEN), false, &mut in_url).ok(),
            Some(Some("x=1".into()))
        );
        let mut local = get("/api/server/hook?x=1", &[]);
        assert_eq!(
            relay_token(Some(TOKEN), true, &mut local).ok(),
            Some(Some("x=1".into()))
        );
    }

    #[test]
    fn a_connection_to_one_of_our_own_addresses_is_this_machine() {
        let address = |text: &str| text.parse::<SocketAddr>().ok();
        let peer = |text: &str| address(text).unwrap_or(SocketAddr::from(([0, 0, 0, 0], 0)));
        assert!(same_machine(peer("127.0.0.1:5000"), None));
        assert!(same_machine(peer("[::ffff:127.0.0.1]:5000"), None));
        assert!(same_machine(
            peer("100.64.0.10:5000"),
            address("100.64.0.10:8767")
        ));
        assert!(!same_machine(
            peer("100.64.0.11:5000"),
            address("100.64.0.10:8767")
        ));
    }
}
