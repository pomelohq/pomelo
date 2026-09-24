use indexmap::IndexMap;
use pom_config::{Config, Dir};

use crate::branch::{branch_hash, branch_host, branch_safe, workspace_label};
use crate::template;

pub const BIND_IP: &str = "127.0.0.1";
pub const DEFAULT_PROXY_PORT: u16 = 8767;
pub const DEV_PROXY_PREFIX: &str = "/_pom_dev";
const LOCAL_DOMAIN: &str = "localhost";
const LOCAL_PROFILE: &str = "local";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlotAllocation {
    pub instance: u16,
    pub slot: u32,
}

/// Machine state the resolver reads: port leases, shared-service slot allocations, secrets.
/// A trait so tests and the oracle can pin every value.
pub trait EnvSources {
    /// Leased base port of a shared service, or `None` when nothing is leased yet.
    fn shared_port(&self, name: &str) -> Option<u16>;
    fn service_port(&self, ws_key: &str, service_key: &str) -> Option<u16>;
    fn slot(&self, shared_name: &str, ws_key: &str) -> Option<SlotAllocation>;
    fn secret(&self, session: &str, name: &str) -> Option<String>;
}

/// Everything needed to turn `{{...}}` tokens into values for one workspace.
pub struct ResolveContext<'a> {
    pub config: &'a Config,
    pub branch: &'a str,
    pub ws_key: &'a str,
    /// Active environment profile; empty or `local` means no remote overrides.
    pub env_name: &'a str,
    pub db_names: &'a IndexMap<String, String>,
    pub sources: &'a dyn EnvSources,
}

impl ResolveContext<'_> {
    /// Filters (`|upper` etc.) are accepted syntactically but never transform: dot fields
    /// replaced them.
    pub fn resolve(&self, text: &str) -> String {
        template::resolve(text, |key| self.lookup(key), |_, value| value)
    }

    /// Resolves an env map; output is sorted by key so written env files are stable.
    pub fn resolve_env(&self, env: &IndexMap<String, String>) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = env
            .iter()
            .map(|(key, value)| (key.clone(), self.resolve(value)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub fn lookup(&self, key: &str) -> Option<String> {
        let parts: Vec<&str> = key.split('.').collect();
        let field = |index: usize| parts.get(index).copied().unwrap_or_default();
        let head = field(0);
        let has_name = parts.len() >= 2;
        match head {
            "bind_ip" => Some(BIND_IP.to_string()),
            "branch" => match field(1) {
                "" => Some(self.branch.to_string()),
                "safe" => Some(branch_safe(self.branch)),
                "host" => Some(branch_host(self.branch)),
                "hash" => Some(branch_hash(self.branch)),
                _ => None,
            },
            "slot" if has_name => Some(self.slot_index(field(1))),
            "db" if has_name => Some(self.resolve_db(field(1), field(2))),
            "secret" if has_name => self.sources.secret(&self.config.session, field(1)),
            "shared" if has_name => self.resolve_shared(field(1), field(2)),
            "slot" | "db" | "secret" | "shared" => None,
            _ if has_name => self.resolve_service(head, field(1), field(2)),
            _ => None,
        }
    }

    fn slot_index(&self, name: &str) -> String {
        self.sources
            .slot(name, self.ws_key)
            .map_or(0, |allocation| allocation.slot)
            .to_string()
    }

    fn resolve_db(&self, name: &str, field: &str) -> String {
        let db_name = self.db_names.get(name).cloned().unwrap_or_default();
        if field != "url" {
            return db_name;
        }
        self.shared_named("postgres")
            .and_then(|postgres| self.resolve_shared(postgres, "url"))
            .map_or(db_name.clone(), |conn| {
                format!("postgres://{conn}/{db_name}")
            })
    }

    fn resolve_shared(&self, name: &str, field: &str) -> Option<String> {
        let def = self.config.shared_services.get(name)?;
        let or_default = |value: &str| {
            if value.is_empty() {
                "postgres".to_string()
            } else {
                value.to_string()
            }
        };
        let user = or_default(&def.db_user);
        let pass = or_default(&def.db_password);
        // A capacity > 1 service runs one container per instance at base + instance.
        let instance = self
            .sources
            .slot(name, self.ws_key)
            .map_or(0, |allocation| allocation.instance);
        let port = self
            .sources
            .shared_port(name)
            .unwrap_or(0)
            .saturating_add(instance);
        // Explicit IPv4: clients resolving `localhost` to ::1 miss Docker's 0.0.0.0 publish.
        let host = BIND_IP;
        match field {
            "host" => Some(host.to_string()),
            "port" => Some(port.to_string()),
            "user" => Some(user),
            "pass" => Some(pass),
            "slot" => Some(self.slot_index(name)),
            "url" | "" => {
                let port = if port == 0 { 5432 } else { port };
                Some(format!("{user}:{pass}@{host}:{port}"))
            }
            _ => None,
        }
    }

    fn resolve_service(&self, repo: &str, service: &str, field: &str) -> Option<String> {
        let (dir, alias) = self.find_repo(repo)?;
        let field = if field.is_empty() { "url" } else { field };
        if field == "path" {
            return Some(format!("{DEV_PROXY_PREFIX}/{repo}/{service}"));
        }
        if let Some(value) = self.env_override(&format!("{repo}.{service}")) {
            return match field {
                "url" => Some(value),
                "ws" => Some(http_to_ws(&value)),
                "host" => Some(url_host_port(&value).0.to_string()),
                "port" => url_host_port(&value).1.map(|port| port.to_string()),
                _ => None,
            };
        }
        let host = format!(
            "{service}.{alias}.{}.{LOCAL_DOMAIN}",
            workspace_label(self.branch)
        );
        match field {
            "port" => Some(self.service_port(dir, &alias, service).to_string()),
            "host" => Some(host),
            "url" => Some(format!("http://{host}:{DEFAULT_PROXY_PORT}")),
            "ws" => Some(format!("ws://{host}:{DEFAULT_PROXY_PORT}")),
            _ => None,
        }
    }

    fn service_port(&self, dir: &Dir, alias: &str, service: &str) -> u16 {
        let lease = |name: &str| {
            self.sources
                .service_port(self.ws_key, &format!("{alias}~{name}"))
        };
        if !service.is_empty() {
            return lease(service).unwrap_or(0);
        }
        dir.services
            .keys()
            .next()
            .and_then(|first| lease(first))
            .or(dir.proxy_port)
            .unwrap_or(0)
    }

    fn env_override(&self, key: &str) -> Option<String> {
        if self.env_name.is_empty() || self.env_name == LOCAL_PROFILE {
            return None;
        }
        self.config
            .environments
            .get(self.env_name)?
            .get(key)
            .filter(|value| !value.is_empty())
            .cloned()
    }

    /// A repo by directory name or alias; the alias falls back to the directory name.
    fn find_repo(&self, name: &str) -> Option<(&Dir, String)> {
        self.config.repos.iter().find_map(|(dir_name, dir)| {
            let alias = if dir.alias.is_empty() {
                dir_name
            } else {
                &dir.alias
            };
            (dir_name == name || alias == name).then(|| (dir, alias.clone()))
        })
    }

    fn shared_named(&self, kind: &str) -> Option<&str> {
        self.config
            .shared_services
            .iter()
            .find(|(name, def)| {
                let def_kind = if def.kind.is_empty() {
                    name.as_str()
                } else {
                    &def.kind
                };
                def_kind == kind || name.as_str() == kind
            })
            .map(|(name, _)| name.as_str())
    }
}

/// Host and effective port of an absolute URL; the port falls back to the scheme default and is
/// `None` when neither is known.
fn url_host_port(url: &str) -> (&str, Option<u16>) {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or_default();
    let (host, port_text) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            (host, after.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    let explicit_port = port_text.and_then(|port| port.parse().ok());
    let default_port = match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    };
    (host, explicit_port.or(default_port))
}

/// `http(s)://` becomes `ws(s)://`; anything else passes through untouched.
pub fn http_to_ws(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        url.to_string()
    }
}
