use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use settings::{Cidr, ProxySettings, LOCAL_DOMAIN};

pub const DEFAULT_WEBHOOK_PORT: u16 = 8766;
pub const DEFAULT_PROXY_PORT: u16 = 8767;
pub const PROXY_PORT_KEY: &str = "dev_proxy_port";
pub const WEBHOOK_PORT_KEY: &str = "webhook_port";
pub const PROXY_KEY: &str = "proxy";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DevPorts {
    pub webhook: u16,
    pub proxy: u16,
}

impl DevPorts {
    pub fn from_base(base: u16) -> DevPorts {
        DevPorts {
            webhook: base.saturating_add(1),
            proxy: base.saturating_add(2),
        }
    }
}

impl Default for DevPorts {
    fn default() -> Self {
        DevPorts {
            webhook: DEFAULT_WEBHOOK_PORT,
            proxy: DEFAULT_PROXY_PORT,
        }
    }
}

/// Where the dev proxy listens and how the services behind it are named and reached from outside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevNetwork {
    pub ports: DevPorts,
    pub bind: IpAddr,
    pub domain: String,
    pub https: bool,
    /// The port in public URLs: the proxy's own unless a reverse proxy in front serves another.
    pub public_port: u16,
    pub trusted_proxies: Vec<Cidr>,
    pub token_env: String,
    pub allow_unauthenticated: bool,
}

impl DevNetwork {
    /// This machine only, under `localhost`, as before any of it was configurable.
    pub fn local(ports: DevPorts) -> DevNetwork {
        DevNetwork::new(ports, &ProxySettings::default())
    }

    pub fn new(ports: DevPorts, proxy: &ProxySettings) -> DevNetwork {
        DevNetwork {
            ports,
            bind: proxy.bind_ip(),
            domain: proxy.domain_name(),
            https: proxy.https(),
            public_port: if proxy.public_port == 0 {
                ports.proxy
            } else {
                proxy.public_port
            },
            trusted_proxies: proxy.trusted(),
            token_env: proxy.token_env_name(),
            allow_unauthenticated: proxy.allow_unauthenticated,
        }
    }

    pub fn public(&self) -> PublicAddress {
        PublicAddress {
            https: self.https,
            domain: self.domain.clone(),
            port: self.public_port,
        }
    }

    /// Other machines can connect, so they must present the proxy token unless the user accepted the risk.
    pub fn exposed(&self) -> bool {
        !self.bind.is_loopback()
    }

    pub fn trusts(&self, peer: IpAddr) -> bool {
        self.trusted_proxies
            .iter()
            .any(|range| range.contains(peer))
    }
}

/// How a browser reaches services through the proxy: scheme, the domain hosts end in, and the port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicAddress {
    pub https: bool,
    pub domain: String,
    pub port: u16,
}

impl PublicAddress {
    pub fn local(port: u16) -> PublicAddress {
        PublicAddress {
            https: false,
            domain: LOCAL_DOMAIN.into(),
            port,
        }
    }

    pub fn is_local(&self) -> bool {
        self.domain == LOCAL_DOMAIN
    }

    pub fn url(&self, host: &str) -> String {
        let scheme = if self.https { "https" } else { "http" };
        format!("{scheme}://{host}{}", self.port_suffix())
    }

    pub fn ws(&self, host: &str) -> String {
        let scheme = if self.https { "wss" } else { "ws" };
        format!("{scheme}://{host}{}", self.port_suffix())
    }

    fn port_suffix(&self) -> String {
        let default_port = if self.https { 443 } else { 80 };
        if self.port == default_port {
            String::new()
        } else {
            format!(":{}", self.port)
        }
    }
}

type Cached = Option<(PathBuf, Option<SystemTime>, DevNetwork)>;

// Every env resolution asks for the proxy's address; re-read the settings file only when it changed.
static CACHE: Mutex<Cached> = Mutex::new(None);

/// The ports the dev proxy and webhook relay listen on: `POM_WEB_PORT` (a base, so a dev build can run
/// beside an installed one), else the user's settings, else the defaults. The app, the CLI and service URL
/// templates all read this so they agree.
pub fn dev_ports() -> DevPorts {
    dev_network().ports
}

/// The proxy's ports (see `dev_ports`) with its bind address, domain and public scheme and port from settings.
pub fn dev_network() -> DevNetwork {
    let mut network = settings_network();
    if let Some(base) = std::env::var("POM_WEB_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
    {
        let proxy_port = network.ports.proxy;
        network.ports = DevPorts::from_base(base);
        if network.public_port == proxy_port {
            network.public_port = network.ports.proxy;
        }
    }
    network
}

/// Drops the cached settings, for a writer that reads them back within the file's timestamp resolution.
pub fn forget_dev_network() {
    if let Ok(mut cache) = CACHE.lock() {
        *cache = None;
    }
}

fn settings_network() -> DevNetwork {
    let Some(path) = pom_paths::config_dir().map(|dir| dir.join("settings.json")) else {
        return DevNetwork::local(DevPorts::default());
    };
    let modified = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok();
    let Ok(mut cache) = CACHE.lock() else {
        return read(&path);
    };
    if let Some((cached_path, cached_modified, network)) = cache.as_ref() {
        if *cached_path == path && *cached_modified == modified {
            return network.clone();
        }
    }
    let network = read(&path);
    *cache = Some((path, modified, network.clone()));
    network
}

fn read(path: &std::path::Path) -> DevNetwork {
    let Some(value) = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    else {
        return DevNetwork::local(DevPorts::default());
    };
    network_from_settings(&value)
}

pub fn network_from_settings(value: &serde_json::Value) -> DevNetwork {
    let proxy = value
        .get(PROXY_KEY)
        .cloned()
        .and_then(|proxy| serde_json::from_value::<ProxySettings>(proxy).ok())
        .unwrap_or_default();
    DevNetwork::new(from_settings(value), &proxy)
}

pub fn from_settings(value: &serde_json::Value) -> DevPorts {
    let port = |key: &str, fallback: u16| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port > 0)
            .unwrap_or(fallback)
    };
    DevPorts {
        webhook: port(WEBHOOK_PORT_KEY, DEFAULT_WEBHOOK_PORT),
        proxy: port(PROXY_PORT_KEY, DEFAULT_PROXY_PORT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_ports_override_defaults_and_bad_values_fall_back() {
        let value = serde_json::json!({"dev_proxy_port": 9100, "webhook_port": 0});
        assert_eq!(
            from_settings(&value),
            DevPorts {
                webhook: DEFAULT_WEBHOOK_PORT,
                proxy: 9100
            }
        );
        assert_eq!(
            from_settings(&serde_json::json!({"dev_proxy_port": 70000})),
            DevPorts::default()
        );
    }

    #[test]
    fn the_settings_defaults_match_and_their_keys_are_read() {
        let mut settings = settings::Settings::default();
        let value = serde_json::to_value(&settings).unwrap();
        assert_eq!(from_settings(&value), DevPorts::default());
        settings.dev_proxy_port = 9200;
        settings.webhook_port = 9201;
        let value = serde_json::to_value(&settings).unwrap();
        assert_eq!(
            from_settings(&value),
            DevPorts {
                webhook: 9201,
                proxy: 9200
            }
        );
    }

    #[test]
    fn the_network_defaults_to_this_machine_under_localhost() {
        let network = network_from_settings(&serde_json::json!({}));
        assert_eq!(network, DevNetwork::local(DevPorts::default()));
        assert!(!network.exposed());
        assert_eq!(network.public(), PublicAddress::local(DEFAULT_PROXY_PORT));
        assert_eq!(
            network.public().url("web.web.main.localhost"),
            "http://web.web.main.localhost:8767"
        );
    }

    #[test]
    fn the_settings_proxy_block_is_read() {
        let mut settings = settings::Settings::default();
        settings.proxy.bind = "100.64.0.10".into();
        settings.proxy.domain = "dev.example.net".into();
        settings.proxy.public_scheme = "https".into();
        settings.proxy.public_port = 443;
        settings.proxy.trusted_proxies = vec!["127.0.0.1".into()];
        let network = network_from_settings(&serde_json::to_value(&settings).unwrap());
        assert!(network.exposed());
        assert!(network.trusts("127.0.0.1".parse().unwrap()));
        assert!(!network.trusts("10.0.0.1".parse().unwrap()));
        assert_eq!(
            network.public().url("web.web.proj-101.dev.example.net"),
            "https://web.web.proj-101.dev.example.net"
        );
    }

    #[test]
    fn public_urls_drop_only_the_scheme_default_port() {
        let address = |https: bool, port: u16| PublicAddress {
            https,
            domain: "dev.example.net".into(),
            port,
        };
        assert_eq!(
            address(true, 443).url("a.b.c.dev.example.net"),
            "https://a.b.c.dev.example.net"
        );
        assert_eq!(
            address(true, 443).ws("a.b.c.dev.example.net"),
            "wss://a.b.c.dev.example.net"
        );
        assert_eq!(
            address(true, 8443).url("a.b.c.dev.example.net"),
            "https://a.b.c.dev.example.net:8443"
        );
        assert_eq!(
            address(false, 80).url("a.b.c.dev.example.net"),
            "http://a.b.c.dev.example.net"
        );
        assert_eq!(
            address(false, 443).url("a.b.c.dev.example.net"),
            "http://a.b.c.dev.example.net:443"
        );
        assert_eq!(address(true, 80).ws("h"), "wss://h:80");
    }

    #[test]
    fn a_base_puts_the_relay_one_above_and_the_proxy_two_above() {
        assert_eq!(
            DevPorts::from_base(8765),
            DevPorts {
                webhook: 8766,
                proxy: 8767
            }
        );
    }
}
