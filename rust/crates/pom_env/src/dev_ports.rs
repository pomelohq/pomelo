use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

pub const DEFAULT_WEBHOOK_PORT: u16 = 8766;
pub const DEFAULT_PROXY_PORT: u16 = 8767;
pub const PROXY_PORT_KEY: &str = "dev_proxy_port";
pub const WEBHOOK_PORT_KEY: &str = "webhook_port";

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

type Cached = Option<(PathBuf, Option<SystemTime>, DevPorts)>;

// Every env resolution asks for the proxy port; re-read the settings file only when it changed.
static CACHE: Mutex<Cached> = Mutex::new(None);

/// The ports the dev proxy and webhook relay listen on: `POM_WEB_PORT` (a base, so a dev build can run
/// beside an installed one), else the user's settings, else the defaults. The app, the CLI and service URL
/// templates all read this so they agree.
pub fn dev_ports() -> DevPorts {
    if let Some(base) = std::env::var("POM_WEB_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
    {
        return DevPorts::from_base(base);
    }
    let Some(path) = pom_paths::config_dir().map(|dir| dir.join("settings.json")) else {
        return DevPorts::default();
    };
    let modified = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok();
    let Ok(mut cache) = CACHE.lock() else {
        return read(&path);
    };
    if let Some((cached_path, cached_modified, ports)) = cache.as_ref() {
        if *cached_path == path && *cached_modified == modified {
            return *ports;
        }
    }
    let ports = read(&path);
    *cache = Some((path, modified, ports));
    ports
}

fn read(path: &std::path::Path) -> DevPorts {
    let Some(value) = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    else {
        return DevPorts::default();
    };
    from_settings(&value)
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
