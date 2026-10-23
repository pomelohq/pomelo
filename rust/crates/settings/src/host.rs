//! The settings that act on the machine a project runs on rather than on the app showing it, so a remote window
//! reads and changes them on the host (`settings.get` / `settings.set` of `pom server`).

use std::io::Write;
use std::path::Path;

use serde_json::{Map, Value};

use crate::{parse_bind, parse_domain, parse_env_name, parse_scheme, Cidr, Settings};

/// Every host-scoped key, dotted for the nested ones; `settings.set` takes no other.
pub const HOST_KEYS: &[&str] = &[
    "agent_command",
    "terminal_shell",
    "modules_store_enabled",
    "modules_fallback",
    "modules_size_limit_gb",
    "modules_unused_days",
    "dev_proxy_port",
    "webhook_port",
    "proxy.bind",
    "proxy.domain",
    "proxy.public_scheme",
    "proxy.public_port",
    "proxy.trusted_proxies",
    "proxy.token_env",
    "proxy.allow_unauthenticated",
    "server.idle_exit_minutes",
    "server.reconnect_retention_minutes",
    "workspaces.idle_sleep_minutes",
    "workspaces.max_running",
];

/// Listening ports stay out of the privileged range.
pub const PORT_MIN: u16 = 1024;
pub const MODULES_LIMIT_MAX_GB: u64 = 1024;
pub const MODULES_DAYS_MAX: u64 = 365;
pub const MODULES_FALLBACK_VALUES: [&str; 3] = ["hardlink", "copy", "install"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostSettingsError {
    /// The change was not a JSON object.
    NotAnObject,
    /// Keys outside [`HOST_KEYS`], dotted.
    Refused(Vec<String>),
    Invalid {
        key: String,
        message: String,
    },
    /// settings.json could not be read, parsed or written.
    File(String),
}

impl std::fmt::Display for HostSettingsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostSettingsError::NotAnObject => {
                formatter.write_str("the change must be a JSON object of settings")
            }
            HostSettingsError::Refused(keys) => write!(
                formatter,
                "not a setting of this machine: {} (it takes {})",
                keys.join(", "),
                HOST_KEYS.join(", ")
            ),
            HostSettingsError::Invalid { key, message } => write!(formatter, "{key}: {message}"),
            HostSettingsError::File(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for HostSettingsError {}

fn lookup<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.')
        .try_fold(value, |value, part| value.get(part))
}

fn insert(root: &mut Map<String, Value>, key: &str, value: Value) {
    let mut parts: Vec<&str> = key.split('.').collect();
    let Some(last) = parts.pop() else {
        return;
    };
    let mut object = root;
    for part in parts {
        let slot = object
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !slot.is_object() {
            *slot = Value::Object(Map::new());
        }
        let Value::Object(inner) = slot else {
            return;
        };
        object = inner;
    }
    object.insert(last.to_string(), value);
}

/// `value` as `key` holds it once read into [`Settings`], or `None` when it is not of the key's type.
fn typed(key: &str, value: &Value) -> Option<Value> {
    let mut alone = Map::new();
    insert(&mut alone, key, value.clone());
    let settings: Settings = serde_json::from_value(Value::Object(alone)).ok()?;
    let written = serde_json::to_value(settings).ok()?;
    lookup(&written, key).cloned()
}

/// The host keys of a settings.json document, nested as the file nests them; a key missing or of the wrong
/// type is its default, as the host's readers take it.
pub fn host_values(file: &Value) -> Value {
    let defaults = serde_json::to_value(Settings::default()).unwrap_or(Value::Null);
    let mut values = Map::new();
    for key in HOST_KEYS {
        let chosen = lookup(file, key)
            .and_then(|value| typed(key, value))
            .or_else(|| lookup(&defaults, key).cloned())
            .unwrap_or(Value::Null);
        insert(&mut values, key, chosen);
    }
    Value::Object(values)
}

/// The leaves of a change, dotted: `{"proxy": {"bind": ..}}` and `{"proxy.bind": ..}` both name `proxy.bind`.
fn leaves(prefix: &str, object: &Map<String, Value>, out: &mut Vec<(String, Value)>) {
    for (name, value) in object {
        let key = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        let is_section = HOST_KEYS
            .iter()
            .any(|known| known.starts_with(&format!("{key}.")));
        match value {
            Value::Object(inner) if is_section => leaves(&key, inner, out),
            _ => out.push((key, value.clone())),
        }
    }
}

fn whole_number(key: &str, value: &Value, min: u64, max: u64) -> Result<Value, HostSettingsError> {
    let invalid = |message: String| HostSettingsError::Invalid {
        key: key.to_string(),
        message,
    };
    let number = value
        .as_u64()
        .ok_or_else(|| invalid(format!("{value} is not a whole number")))?;
    if number < min || number > max {
        return Err(invalid(format!("{number} is not between {min} and {max}")));
    }
    Ok(Value::from(number))
}

/// `value` checked and normalized as the Settings window keeps it.
pub fn validate(key: &str, value: &Value) -> Result<Value, HostSettingsError> {
    let invalid = |message: String| HostSettingsError::Invalid {
        key: key.to_string(),
        message,
    };
    let text = || {
        value
            .as_str()
            .map(str::trim)
            .ok_or_else(|| invalid(format!("{value} is not text")))
    };
    let flag = || {
        value
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| invalid(format!("{value} is not true or false")))
    };
    match key {
        "agent_command" => {
            let command = text()?;
            Ok(Value::from(if command.is_empty() {
                Settings::default().agent_command
            } else {
                command.to_string()
            }))
        }
        "terminal_shell" => Ok(Value::from(text()?)),
        "modules_store_enabled" | "proxy.allow_unauthenticated" => flag(),
        "modules_fallback" => {
            let fallback = text()?;
            if MODULES_FALLBACK_VALUES.contains(&fallback) {
                Ok(Value::from(fallback))
            } else {
                Err(invalid(format!(
                    "{fallback} is not one of {}",
                    MODULES_FALLBACK_VALUES.join(", ")
                )))
            }
        }
        "modules_size_limit_gb" => whole_number(key, value, 1, MODULES_LIMIT_MAX_GB),
        "modules_unused_days" => whole_number(key, value, 0, MODULES_DAYS_MAX),
        "dev_proxy_port" | "webhook_port" => {
            whole_number(key, value, PORT_MIN.into(), u16::MAX.into())
        }
        "proxy.bind" => {
            let bind = text()?;
            if bind.is_empty() {
                return Ok(Value::from(crate::DEFAULT_PROXY_BIND.to_string()));
            }
            parse_bind(bind)
                .map(|address| Value::from(address.to_string()))
                .map_err(invalid)
        }
        "proxy.domain" => parse_domain(text()?).map(Value::from).map_err(invalid),
        "proxy.public_scheme" => parse_scheme(text()?).map(Value::from).map_err(invalid),
        "proxy.public_port" => whole_number(key, value, 0, u16::MAX.into()),
        "proxy.trusted_proxies" => {
            let entries = value
                .as_array()
                .ok_or_else(|| invalid(format!("{value} is not a list of address ranges")))?;
            entries
                .iter()
                .map(|entry| {
                    let entry = entry
                        .as_str()
                        .ok_or_else(|| invalid(format!("{entry} is not an address range")))?;
                    Cidr::parse(entry)
                        .map(|range| Value::from(range.to_string()))
                        .map_err(invalid)
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        "proxy.token_env" => {
            let name = text()?;
            if name.is_empty() {
                return Ok(Value::from(crate::DEFAULT_PROXY_TOKEN_ENV));
            }
            parse_env_name(name).map(Value::from).map_err(invalid)
        }
        "server.idle_exit_minutes"
        | "server.reconnect_retention_minutes"
        | "workspaces.idle_sleep_minutes"
        | "workspaces.max_running" => whole_number(key, value, 0, u64::MAX),
        _ => Err(HostSettingsError::Refused(vec![key.to_string()])),
    }
}

/// Lays `change` over the settings.json document `file`, every other key and unknown field kept; nothing
/// changes when any key is refused or invalid.
pub fn apply_host_change(file: &mut Value, change: &Value) -> Result<(), HostSettingsError> {
    let Value::Object(change) = change else {
        return Err(HostSettingsError::NotAnObject);
    };
    let mut changed = Vec::new();
    leaves("", change, &mut changed);
    let refused: Vec<String> = changed
        .iter()
        .filter(|(key, _)| !HOST_KEYS.contains(&key.as_str()))
        .map(|(key, _)| key.clone())
        .collect();
    if !refused.is_empty() {
        return Err(HostSettingsError::Refused(refused));
    }
    let checked = changed
        .iter()
        .map(|(key, value)| validate(key, value).map(|value| (key.as_str(), value)))
        .collect::<Result<Vec<_>, _>>()?;
    if !file.is_object() {
        *file = Value::Object(Map::new());
    }
    let Value::Object(root) = file else {
        return Ok(());
    };
    for (key, value) in checked {
        insert(root, key, value);
    }
    Ok(())
}

/// The settings.json at `path` as a document; a missing file is an empty one.
fn read_document(path: &Path) -> Result<Value, HostSettingsError> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Value::Object(Map::new())),
        Ok(text) => serde_json::from_str(&text).map_err(|error| {
            HostSettingsError::File(format!("{} does not parse: {error}", path.display()))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(error) => Err(HostSettingsError::File(format!(
            "{}: {error}",
            path.display()
        ))),
    }
}

/// The host keys of the settings.json at `path`, defaults filled.
pub fn read_host_settings(path: &Path) -> Result<Value, HostSettingsError> {
    read_document(path).map(|file| host_values(&file))
}

/// Applies `change` to the settings.json at `path` and writes it in one step (a reader never sees half a
/// file); returns the host keys as they now are.
pub fn write_host_settings(path: &Path, change: &Value) -> Result<Value, HostSettingsError> {
    let mut file = read_document(path)?;
    apply_host_change(&mut file, change)?;
    let failed =
        |error: std::io::Error| HostSettingsError::File(format!("{}: {error}", path.display()));
    let dir = path
        .parent()
        .ok_or_else(|| HostSettingsError::File(format!("{} has no folder", path.display())))?;
    std::fs::create_dir_all(dir).map_err(failed)?;
    let text = serde_json::to_string_pretty(&file)
        .map_err(|error| HostSettingsError::File(error.to_string()))?;
    let temporary = dir.join(format!(".settings.json.{}.tmp", std::process::id()));
    let written = std::fs::File::create(&temporary).and_then(|mut out| {
        out.write_all(text.as_bytes())?;
        out.sync_all()
    });
    if let Err(error) = written.and_then(|()| std::fs::rename(&temporary, path)) {
        if let Err(cleanup) = std::fs::remove_file(&temporary) {
            if cleanup.kind() != std::io::ErrorKind::NotFound {
                eprintln!("settings: {}: {cleanup}", temporary.display());
            }
        }
        return Err(failed(error));
    }
    Ok(host_values(&file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_host_key_is_a_setting_with_a_default() {
        let values = host_values(&json!({}));
        let defaults = serde_json::to_value(Settings::default()).unwrap();
        for key in HOST_KEYS {
            assert_eq!(lookup(&values, key), lookup(&defaults, key), "{key}");
            assert!(lookup(&values, key).is_some(), "{key}");
        }
    }

    #[test]
    fn a_value_of_the_wrong_type_reads_as_its_default() {
        let values = host_values(&json!({"dev_proxy_port": "x", "proxy": {"bind": "0.0.0.0"}}));
        assert_eq!(values["dev_proxy_port"], json!(8767));
        assert_eq!(values["proxy"]["bind"], json!("0.0.0.0"));
        assert_eq!(values["proxy"]["domain"], json!("localhost"));
    }

    #[test]
    fn a_change_is_checked_and_normalized_as_the_settings_window_does() {
        let mut file =
            json!({"theme": "One Dark", "future": {"x": 1}, "proxy": {"bind": "127.0.0.1"}});
        apply_host_change(
            &mut file,
            &json!({"agent_command": "  ", "proxy": {"domain": ".Dev.Example.NET", "trusted_proxies": ["10.0.0.0/8"]},
                    "workspaces.max_running": 3}),
        )
        .unwrap();
        assert_eq!(file["agent_command"], json!("claude"));
        assert_eq!(file["proxy"]["domain"], json!("dev.example.net"));
        assert_eq!(file["proxy"]["bind"], json!("127.0.0.1"));
        assert_eq!(file["proxy"]["trusted_proxies"], json!(["10.0.0.0/8"]));
        assert_eq!(file["workspaces"]["max_running"], json!(3));
        assert_eq!(file["theme"], json!("One Dark"));
        assert_eq!(file["future"], json!({"x": 1}));
    }

    #[test]
    fn a_bad_change_leaves_the_file_as_it_was() {
        let before = json!({"dev_proxy_port": 9000});
        for (change, wanted) in [
            (
                json!({"theme": "x"}),
                "not a setting of this machine: theme",
            ),
            (
                json!({"server": {"unconfirmed_input_kib": 1}}),
                "server.unconfirmed_input_kib",
            ),
            (
                json!({"dev_proxy_port": 80}),
                "dev_proxy_port: 80 is not between 1024",
            ),
            (json!({"modules_fallback": "zip"}), "zip is not one of"),
            (
                json!({"proxy": {"bind": "example.com"}}),
                "not an IP address",
            ),
            (
                json!({"proxy": {"token_env": "1BAD"}}),
                "not an environment variable name",
            ),
            (
                json!({"modules_size_limit_gb": 0}),
                "0 is not between 1 and 1024",
            ),
            (
                json!({"modules_store_enabled": "yes"}),
                "is not true or false",
            ),
            (json!(["agent_command"]), "must be a JSON object"),
        ] {
            let mut file = before.clone();
            let error = apply_host_change(&mut file, &change)
                .unwrap_err()
                .to_string();
            assert!(error.contains(wanted), "{change}: {error}");
            assert_eq!(file, before, "{change}");
        }
    }

    #[test]
    fn writing_keeps_the_rest_of_the_file_and_replaces_it_whole() {
        let dir = std::env::temp_dir().join(format!("pom-host-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"theme": "One Dark", "newer_key": [1, 2]}"#).unwrap();
        let values = write_host_settings(&path, &json!({"terminal_shell": "/bin/zsh -l"})).unwrap();
        assert_eq!(values["terminal_shell"], json!("/bin/zsh -l"));
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["theme"], json!("One Dark"));
        assert_eq!(file["newer_key"], json!([1, 2]));
        assert_eq!(read_host_settings(&path).unwrap(), values);
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(leftovers.len(), 1, "no temporary file left");
        std::fs::write(&path, "{ oops").unwrap();
        assert!(matches!(
            write_host_settings(&path, &json!({"terminal_shell": ""})),
            Err(HostSettingsError::File(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ oops");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
