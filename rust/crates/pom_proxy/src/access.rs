//! Who may use the proxy: the token other machines present, the hosts it answers to, and the browser requests
//! its own endpoints refuse.

use hyper::header::{HeaderMap, HeaderValue, COOKIE, ORIGIN};
use pom_paths::StateDir;
use pom_secrets::SecretStore;

pub const TOKEN_HEADER: &str = "x-pom-token";
pub const TOKEN_PARAM: &str = "pom_token";
pub const TOKEN_COOKIE: &str = "pom_token";
const TOKEN_SECRETS: &str = "_machine";
const TOKEN_SECRET: &str = "PROXY_TOKEN";
pub const MIN_TOKEN_LENGTH: usize = 16;
const COOKIE_MAX_AGE_SECONDS: u64 = 30 * 24 * 60 * 60;

/// Where the token came from, for messages that must not show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenSource {
    Env(String),
    Secret,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyToken {
    pub value: String,
    pub source: TokenSource,
}

/// The token from `proxy.token_env`'s variable, else the one `pom proxy token new` stored for this machine.
pub fn proxy_token(network: &pom_env::DevNetwork, state: &StateDir) -> Option<ProxyToken> {
    let from_env = std::env::var(&network.token_env)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(value) = from_env {
        return Some(ProxyToken {
            value,
            source: TokenSource::Env(network.token_env.clone()),
        });
    }
    match token_store(state).get(TOKEN_SECRET) {
        Ok(Some(value)) if !value.is_empty() => Some(ProxyToken {
            value,
            source: TokenSource::Secret,
        }),
        Ok(_) => None,
        Err(error) => {
            eprintln!("dev proxy: reading the proxy token: {error}");
            None
        }
    }
}

/// A new random token for this machine, kept encrypted in pom's secrets.
pub fn new_proxy_token(state: &StateDir) -> Result<String, String> {
    let value = pom_secrets::random_hex(24);
    token_store(state)
        .set(TOKEN_SECRET, &value)
        .map_err(|error| error.to_string())?;
    Ok(value)
}

pub fn remove_proxy_token(state: &StateDir) -> Result<(), String> {
    token_store(state)
        .set(TOKEN_SECRET, "")
        .map_err(|error| error.to_string())
}

fn token_store(state: &StateDir) -> SecretStore {
    SecretStore::new(state.clone(), TOKEN_SECRETS)
}

/// Why the proxy will not listen where `network` says, if it will not.
pub fn refusal(network: &pom_env::DevNetwork, token: Option<&ProxyToken>) -> Option<String> {
    network.reach()?;
    let (target, outsiders) = if network.exposed() {
        (format!("listen on {}", network.bind), "other machines")
    } else {
        (
            format!("serve {}", network.domain),
            "anyone a reverse proxy forwards",
        )
    };
    match token {
        Some(token) if token.value.len() < MIN_TOKEN_LENGTH => Some(format!(
            "refusing to {target}: the proxy token is shorter than {MIN_TOKEN_LENGTH} characters"
        )),
        Some(token) if !token.value.bytes().all(token_byte) => Some(format!(
            "refusing to {target}: the proxy token may only use letters, digits, `-`, `_`, `.` and `~`"
        )),
        Some(_) => None,
        None if network.allow_unauthenticated => None,
        None => Some(format!(
            "refusing to {target}: {outsiders} could open every workspace with no token. Run `pom proxy token new` (or set {}), or accept the risk with `pom proxy config set allow_unauthenticated true`",
            network.token_env
        )),
    }
}

/// A `localhost` name or a loopback address: what someone on this machine types, never the public domain a
/// reverse proxy serves.
pub(crate) fn localhost_name(host: &str) -> bool {
    let name = crate::routing::host_name(host)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let bare = name
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(&name);
    name == crate::routing::DOMAIN
        || name.ends_with(&format!(".{}", crate::routing::DOMAIN))
        || bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.to_canonical().is_loopback())
}

fn token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

/// The host names the proxy answers to, so a page on another name that resolves here (DNS rebinding) is refused.
pub(crate) fn host_allowed(host: &str, network: &pom_env::DevNetwork) -> bool {
    let name = crate::routing::host_name(host)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let under = |domain: &str| name == domain || name.ends_with(&format!(".{domain}"));
    let bare = name
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(&name);
    let own_address = bare.parse::<std::net::IpAddr>().is_ok_and(|address| {
        address.to_canonical().is_loopback() || (network.exposed() && address == network.bind)
    });
    !name.is_empty() && (under(crate::routing::DOMAIN) || under(&network.domain) || own_address)
}

/// A browser request another page made: pom's own clients and webhook providers send neither header this way.
pub(crate) fn cross_site(headers: &HeaderMap, host: &str) -> bool {
    let fetch_site = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase);
    if matches!(fetch_site.as_deref(), Some("cross-site" | "same-site")) {
        return true;
    }
    headers.get(ORIGIN).is_some_and(|origin| {
        let origin = origin.to_str().unwrap_or_default();
        let authority = origin.split_once("://").map_or("", |(_, rest)| rest);
        origin == "null" || !authority.eq_ignore_ascii_case(host)
    })
}

/// Compares in time independent of where the first difference is.
pub(crate) fn token_matches(candidate: &str, token: &str) -> bool {
    let (candidate, token) = (candidate.as_bytes(), token.as_bytes());
    if candidate.len() != token.len() {
        return false;
    }
    candidate
        .iter()
        .zip(token)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

/// The token a request carries in the header or the cookie.
pub(crate) fn presented(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers
        .get(TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        return Some(value.trim().to_string());
    }
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == TOKEN_COOKIE)
        .map(|(_, value)| value.to_string())
}

/// Takes the token out before a request goes on, so no service or log downstream sees it.
pub(crate) fn strip_token(headers: &mut HeaderMap) {
    headers.remove(TOKEN_HEADER);
    let kept: Vec<String> = headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .map(str::trim)
                .filter(|pair| {
                    let name = pair.split_once('=').map_or(*pair, |(name, _)| name);
                    !pair.is_empty() && name != TOKEN_COOKIE
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|value| !value.is_empty())
        .collect();
    headers.remove(COOKIE);
    for value in kept {
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.append(COOKIE, value);
        }
    }
}

/// The query without `pom_token`, and the token if it was there.
pub(crate) fn split_query(query: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(query) = query else {
        return (None, None);
    };
    let mut token = None;
    let rest: Vec<&str> = query
        .split('&')
        .filter(|pair| match pair.split_once('=') {
            Some((TOKEN_PARAM, value)) => {
                token = Some(value.to_string());
                false
            }
            _ => !pair.is_empty(),
        })
        .collect();
    let rest = (!rest.is_empty()).then(|| rest.join("&"));
    (rest, token)
}

/// The cookie a browser keeps after opening a link with `?pom_token=`: every service host under the proxy's
/// domain shares it, and pages cannot read it.
pub(crate) fn token_cookie(token: &str, host: &str, network: &pom_env::DevNetwork) -> String {
    let name = crate::routing::host_name(host).to_ascii_lowercase();
    let mut cookie = format!(
        "{TOKEN_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={COOKIE_MAX_AGE_SECONDS}"
    );
    if network.domain != crate::routing::DOMAIN && name.ends_with(&format!(".{}", network.domain)) {
        cookie.push_str(&format!("; Domain={}", network.domain));
    }
    if network.https {
        cookie.push_str("; Secure");
    }
    cookie
}

#[cfg(test)]
mod tests {
    use super::*;
    use pom_env::{DevNetwork, DevPorts, ProxySettings};

    fn network(bind: &str, domain: &str) -> DevNetwork {
        DevNetwork::new(
            DevPorts::default(),
            &ProxySettings {
                bind: bind.into(),
                domain: domain.into(),
                ..ProxySettings::default()
            },
        )
    }

    fn token(value: &str) -> ProxyToken {
        ProxyToken {
            value: value.into(),
            source: TokenSource::Secret,
        }
    }

    #[test]
    fn a_non_loopback_bind_needs_a_long_token_or_the_risk_flag() {
        assert_eq!(refusal(&network("127.0.0.1", "localhost"), None), None);
        let exposed = network("0.0.0.0", "localhost");
        let refused = refusal(&exposed, None).unwrap_or_default();
        assert!(
            refused.contains("refusing to listen on 0.0.0.0"),
            "{refused}"
        );
        assert!(refused.contains("POM_PROXY_TOKEN"), "{refused}");
        assert!(refusal(&exposed, Some(&token("short"))).is_some());
        assert!(refusal(&exposed, Some(&token(&format!("{};x", "a".repeat(32))))).is_some());
        assert_eq!(refusal(&exposed, Some(&token(&"a".repeat(32)))), None);
        let accepted = DevNetwork {
            allow_unauthenticated: true,
            ..exposed
        };
        assert_eq!(refusal(&accepted, None), None);
    }

    #[test]
    fn a_public_domain_on_loopback_needs_a_token_or_the_risk_flag() {
        let public = network("127.0.0.1", "dev.example.net");
        let refused = refusal(&public, None).unwrap_or_default();
        assert!(
            refused.contains("refusing to serve dev.example.net"),
            "{refused}"
        );
        assert!(refusal(&public, Some(&token("short"))).is_some());
        assert_eq!(refusal(&public, Some(&token(&"a".repeat(32)))), None);
        let accepted = DevNetwork {
            allow_unauthenticated: true,
            ..public
        };
        assert_eq!(refusal(&accepted, None), None);
        assert_eq!(refusal(&network("127.0.0.1", "pom.localhost"), None), None);
    }

    #[test]
    fn only_localhost_names_count_as_this_machine() {
        for host in [
            "localhost:8767",
            "server.api.feat-login.localhost:8767",
            "LOCALHOST.",
            "127.0.0.1:8767",
            "[::1]:8767",
            "[::ffff:127.0.0.1]",
        ] {
            assert!(localhost_name(host), "{host}");
        }
        for host in [
            "server.api.feat-login.dev.example.net",
            "dev.example.net:443",
            "localhost.example.net",
            "100.64.0.10:8767",
            "",
        ] {
            assert!(!localhost_name(host), "{host}");
        }
    }

    #[test]
    fn only_the_proxys_own_names_are_served() {
        let local = network("127.0.0.1", "localhost");
        for host in [
            "server.api.feat-login.localhost:8767",
            "localhost:8767",
            "127.0.0.1:8767",
            "[::1]:8767",
            "WEB.WEB.MAIN.LOCALHOST",
        ] {
            assert!(host_allowed(host, &local), "{host}");
        }
        for host in [
            "evil.example.com:8767",
            "localhost.evil.example.com",
            "",
            "100.64.0.10:8767",
        ] {
            assert!(!host_allowed(host, &local), "{host}");
        }
        let remote = network("100.64.0.10", "dev.example.net");
        assert!(host_allowed("web.web.proj-101.dev.example.net", &remote));
        assert!(host_allowed("100.64.0.10:8767", &remote));
        assert!(!host_allowed("dev.example.net.evil.example.com", &remote));
    }

    #[test]
    fn browser_requests_from_other_pages_are_told_apart() {
        let headers = |pairs: &[(&'static str, &'static str)]| {
            let mut map = HeaderMap::new();
            for (name, value) in pairs {
                map.insert(*name, HeaderValue::from_static(value));
            }
            map
        };
        let host = "127.0.0.1:8767";
        assert!(!cross_site(&headers(&[]), host));
        assert!(!cross_site(
            &headers(&[
                ("origin", "http://127.0.0.1:8767"),
                ("sec-fetch-site", "same-origin")
            ]),
            host
        ));
        assert!(cross_site(
            &headers(&[("sec-fetch-site", "cross-site")]),
            host
        ));
        assert!(cross_site(
            &headers(&[("sec-fetch-site", "same-site")]),
            host
        ));
        assert!(cross_site(
            &headers(&[("origin", "http://localhost:3000")]),
            host
        ));
        assert!(cross_site(&headers(&[("origin", "null")]), host));
    }

    #[test]
    fn the_token_travels_by_header_cookie_or_query_and_is_stripped() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("sid=1; pom_token=abc; theme=dark"),
        );
        assert_eq!(presented(&headers).as_deref(), Some("abc"));
        headers.insert(TOKEN_HEADER, HeaderValue::from_static("def"));
        assert_eq!(presented(&headers).as_deref(), Some("def"));
        strip_token(&mut headers);
        assert!(headers.get(TOKEN_HEADER).is_none());
        assert_eq!(
            headers.get(COOKIE).and_then(|value| value.to_str().ok()),
            Some("sid=1; theme=dark")
        );
        let mut only = HeaderMap::new();
        only.insert(COOKIE, HeaderValue::from_static("pom_token=abc"));
        strip_token(&mut only);
        assert!(only.get(COOKIE).is_none());

        assert_eq!(
            split_query(Some("a=1&pom_token=xyz&b=2")),
            (Some("a=1&b=2".into()), Some("xyz".into()))
        );
        assert_eq!(
            split_query(Some("pom_token=xyz")),
            (None, Some("xyz".into()))
        );
        assert_eq!(split_query(Some("a=1")), (Some("a=1".into()), None));
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abd", "abc"));
        assert!(!token_matches("ab", "abc"));
    }

    #[test]
    fn the_cookie_covers_the_proxy_domain() {
        let remote = DevNetwork {
            https: true,
            ..network("100.64.0.10", "dev.example.net")
        };
        let cookie = token_cookie("abc", "web.web.main.dev.example.net", &remote);
        assert!(cookie.starts_with("pom_token=abc; Path=/; HttpOnly; SameSite=Lax"));
        assert!(cookie.contains("; Domain=dev.example.net"));
        assert!(cookie.ends_with("; Secure"));
        let ip = token_cookie("abc", "100.64.0.10:8767", &remote);
        assert!(!ip.contains("Domain="), "{ip}");
        let local = token_cookie(
            "abc",
            "web.web.main.localhost:8767",
            &network("0.0.0.0", ""),
        );
        assert!(
            !local.contains("Domain=") && !local.contains("Secure"),
            "{local}"
        );
    }
}
