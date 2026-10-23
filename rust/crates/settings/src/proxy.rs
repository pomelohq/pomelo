use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};

pub const DEFAULT_PROXY_BIND: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
pub const LOCAL_DOMAIN: &str = "localhost";
pub const DEFAULT_PROXY_TOKEN_ENV: &str = "POM_PROXY_TOKEN";

/// How other machines reach the dev proxy: where it listens, the names services go by, and the scheme and
/// port a reverse proxy in front of it serves them on. The defaults keep it to this machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxySettings {
    pub bind: String,
    pub domain: String,
    pub public_scheme: String,
    /// 0 means the proxy's own port.
    pub public_port: u16,
    pub trusted_proxies: Vec<String>,
    /// The environment variable holding the token other machines must present; the token itself is never stored here.
    pub token_env: String,
    /// Serve a non-loopback bind with no token: anyone who reaches it opens every workspace.
    pub allow_unauthenticated: bool,
}

impl Default for ProxySettings {
    fn default() -> Self {
        ProxySettings {
            bind: DEFAULT_PROXY_BIND.to_string(),
            domain: LOCAL_DOMAIN.into(),
            public_scheme: "http".into(),
            public_port: 0,
            trusted_proxies: Vec::new(),
            token_env: DEFAULT_PROXY_TOKEN_ENV.into(),
            allow_unauthenticated: false,
        }
    }
}

impl ProxySettings {
    /// The bind address, or loopback when the setting does not parse.
    pub fn bind_ip(&self) -> IpAddr {
        parse_bind(&self.bind).unwrap_or(DEFAULT_PROXY_BIND)
    }

    pub fn domain_name(&self) -> String {
        parse_domain(&self.domain).unwrap_or_else(|_| LOCAL_DOMAIN.into())
    }

    pub fn https(&self) -> bool {
        self.public_scheme.trim().eq_ignore_ascii_case("https")
    }

    pub fn token_env_name(&self) -> String {
        let name = self.token_env.trim();
        if name.is_empty() {
            DEFAULT_PROXY_TOKEN_ENV.into()
        } else {
            name.into()
        }
    }

    /// The trusted ranges that parse; a bad entry trusts nothing rather than everything.
    pub fn trusted(&self) -> Vec<Cidr> {
        self.trusted_proxies
            .iter()
            .filter_map(|entry| Cidr::parse(entry).ok())
            .collect()
    }
}

/// An IP address, bracketed or not (`[::]` and `::` are the same).
pub fn parse_bind(text: &str) -> Result<IpAddr, String> {
    let text = text.trim();
    let bare = text
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(text);
    bare.parse::<IpAddr>()
        .map_err(|_| format!("{text} is not an IP address (like 127.0.0.1 or 100.64.0.10)"))
}

/// A lowercase DNS name without leading or trailing dots; empty means `localhost`.
pub fn parse_domain(text: &str) -> Result<String, String> {
    let domain = text.trim().trim_matches('.').to_ascii_lowercase();
    if domain.is_empty() {
        return Ok(LOCAL_DOMAIN.into());
    }
    let valid_label = |label: &str| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    };
    if domain.len() > 253 || !domain.split('.').all(valid_label) {
        return Err(format!(
            "{text} is not a domain name (like dev.example.net)"
        ));
    }
    Ok(domain)
}

/// A shell-style variable name, so `token_env` can only point at an environment variable.
pub fn parse_env_name(text: &str) -> Result<String, String> {
    let name = text.trim();
    let valid = name
        .bytes()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if valid {
        Ok(name.to_string())
    } else {
        Err(format!(
            "{text} is not an environment variable name (like POM_PROXY_TOKEN)"
        ))
    }
}

pub fn parse_scheme(text: &str) -> Result<&'static str, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "http" => Ok("http"),
        "https" => Ok("https"),
        _ => Err(format!("{text}: the public scheme is http or https")),
    }
}

/// An address range like `10.0.0.0/8` or `fd7a:115c:a1e0::/48`; a bare address is a range of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    network: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn parse(text: &str) -> Result<Cidr, String> {
        let text = text.trim();
        let unreadable = || format!("{text} is not an address or range (like 10.0.0.0/8)");
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text, None),
        };
        let network = parse_bind(address).map_err(|_| unreadable())?;
        let max = if network.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix.parse::<u8>().map_err(|_| unreadable())?,
            None => max,
        };
        if prefix > max {
            return Err(unreadable());
        }
        Ok(Cidr { network, prefix })
    }

    pub fn contains(&self, address: IpAddr) -> bool {
        // A dual-stack listener reports IPv4 peers as ::ffff:a.b.c.d.
        match (self.network, address.to_canonical()) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                masked(u32::from(network).into(), 32, self.prefix)
                    == masked(u32::from(address).into(), 32, self.prefix)
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                masked(u128::from(network), 128, self.prefix)
                    == masked(u128::from(address), 128, self.prefix)
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for Cidr {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.network, self.prefix)
    }
}

fn masked(bits: u128, width: u8, prefix: u8) -> u128 {
    if prefix == 0 {
        return 0;
    }
    let keep = u32::from(width - prefix);
    (bits >> keep) << keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_parse_and_bad_ones_fall_back() {
        assert_eq!(parse_bind("[::]"), Ok("::".parse().unwrap()));
        assert!(parse_bind("example.com").is_err());
        assert_eq!(
            parse_domain(" .Dev.Example.NET. "),
            Ok("dev.example.net".into())
        );
        assert_eq!(parse_domain(""), Ok("localhost".into()));
        assert!(parse_domain("dev_example.net").is_err());
        assert_eq!(parse_scheme("HTTPS"), Ok("https"));
        assert!(parse_scheme("ftp").is_err());
        let settings = ProxySettings {
            bind: "nope".into(),
            domain: "bad domain".into(),
            trusted_proxies: vec!["10.0.0.0/8".into(), "garbage".into()],
            ..ProxySettings::default()
        };
        assert_eq!(settings.bind_ip(), DEFAULT_PROXY_BIND);
        assert_eq!(settings.domain_name(), "localhost");
        assert_eq!(settings.trusted().len(), 1);
        assert!(!settings.https());
    }

    #[test]
    fn ranges_hold_their_addresses() {
        let range = Cidr::parse("10.1.0.0/16").unwrap();
        assert!(range.contains("10.1.200.3".parse().unwrap()));
        assert!(!range.contains("10.2.0.1".parse().unwrap()));
        let single = Cidr::parse("127.0.0.1").unwrap();
        assert!(single.contains("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!single.contains("127.0.0.2".parse().unwrap()));
        let tailnet = Cidr::parse("fd7a:115c:a1e0::/48").unwrap();
        assert!(tailnet.contains("fd7a:115c:a1e0:ab12::1".parse().unwrap()));
        assert!(!tailnet.contains("10.0.0.1".parse().unwrap()));
        assert!(Cidr::parse("0.0.0.0/0")
            .unwrap()
            .contains("8.8.8.8".parse().unwrap()));
        assert!(Cidr::parse("10.0.0.0/33").is_err());
    }
}
