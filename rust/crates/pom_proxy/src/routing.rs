use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use pom_config::Config;
use pom_ports::Lease;
use regex::Regex;

pub const DOMAIN: &str = "localhost";
pub const DEV_PREFIX: &str = "/_pom_dev/";

const BRANCH_MAP_TTL: Duration = Duration::from_secs(5);
const PORT_CACHE_TTL: Duration = Duration::from_secs(3);
const LEASE_CACHE_TTL: Duration = Duration::from_secs(1);

pub type ConfigSource = Arc<RwLock<Option<Arc<Config>>>>;

/// An open project the proxy routes into.
#[derive(Clone)]
pub struct ProjectRoute {
    pub root: PathBuf,
    pub config: ConfigSource,
}

impl ProjectRoute {
    pub fn config(&self) -> Option<Arc<Config>> {
        self.config.read().ok()?.clone()
    }
}

/// Where a request should go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// A local `127.0.0.1:<port>` backend; `prefix` was stripped from the path and is re-added to cookie paths.
    Local {
        port: u16,
        prefix: String,
    },
    /// A remote environment's base URL.
    External {
        url: String,
        prefix: String,
    },
    Error {
        status: u16,
        message: String,
    },
}

/// What the proxy log records about a `/_pom_dev/` request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Logged {
    pub repo: String,
    pub service: String,
    pub profile: String,
    pub target: String,
}

pub struct Decision {
    pub route: Route,
    /// The path to send upstream.
    pub path: String,
    pub logged: Option<Logged>,
}

/// The labels of a `<...>.localhost` host, without its port.
pub fn host_labels(host: &str) -> Option<Vec<String>> {
    let host = host.rsplit_once(':').map_or(host, |(name, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            name
        } else {
            host
        }
    });
    let rest = host.strip_suffix(&format!(".{DOMAIN}"))?;
    Some(rest.split('.').map(str::to_string).collect())
}

/// `<alias or repo>~<service>` for `repo/service`; a repo with one service may omit it.
pub fn resolve_service_key(config: &Config, target: &str) -> Option<String> {
    let (alias, service) = target.split_once('/').unwrap_or((target, ""));
    let (name, dir) = config
        .repos
        .iter()
        .find(|(name, dir)| repo_alias(name, &dir.alias) == alias || name.as_str() == alias)?;
    let service = if service.is_empty() {
        if dir.services.len() != 1 {
            return None;
        }
        dir.services.keys().next()?.clone()
    } else {
        service.to_string()
    };
    Some(format!("{}~{service}", repo_alias(name, &dir.alias)))
}

fn repo_alias<'a>(name: &'a str, alias: &'a str) -> &'a str {
    if alias.is_empty() {
        name
    } else {
        alias
    }
}

/// The service holder for a `<alias>~<service>` key in a branch.
pub fn service_holder(config: &Config, branch: &str, service_key: &str) -> Option<String> {
    let (alias, service) = service_key.split_once('~')?;
    let name = config
        .repos
        .iter()
        .find(|(name, dir)| repo_alias(name, &dir.alias) == alias || name.as_str() == alias)
        .map(|(name, _)| name)?;
    Some(format!(
        "svc-{}-{}-{name}-{service}",
        pom_env::branch_safe(&config.session),
        pom_env::branch_safe(branch)
    ))
}

/// Host label -> branch: every branch by its full host label, plus its ticket label when no other workspace
/// shares it (two workspaces on one ticket prefix would collide, so only full hosts resolve then).
pub fn branch_labels(branches: &[String]) -> HashMap<String, String> {
    let mut short: HashMap<String, usize> = HashMap::new();
    for branch in branches {
        *short.entry(pom_env::workspace_label(branch)).or_default() += 1;
    }
    let mut labels = HashMap::new();
    for branch in branches {
        labels.insert(pom_env::branch_host(branch), branch.clone());
        let label = pom_env::workspace_label(branch);
        if short.get(&label) == Some(&1) {
            labels.insert(label, branch.clone());
        }
    }
    labels
}

/// A lease wins over a live scan: the service is told to bind exactly it, and scanning while it builds can
/// latch onto a passing bundler socket.
pub fn pick_port(allocated: Option<u16>, live_scan: impl FnOnce() -> Option<u16>) -> Option<u16> {
    allocated.filter(|port| *port > 0).or_else(live_scan)
}

struct CookieRules {
    path: Regex,
    has_path: Regex,
    secure: Regex,
    domain: Regex,
    same_site_none: Regex,
}

fn cookie_rules() -> Option<&'static CookieRules> {
    static RULES: std::sync::OnceLock<Option<CookieRules>> = std::sync::OnceLock::new();
    RULES
        .get_or_init(|| {
            Some(CookieRules {
                path: Regex::new(r"(?i)(;\s*path=)(/[^;]*)").ok()?,
                has_path: Regex::new(r"(?i);\s*path=").ok()?,
                secure: Regex::new(r"(?i);\s*secure\b").ok()?,
                domain: Regex::new(r"(?i);\s*domain=[^;]*").ok()?,
                same_site_none: Regex::new(r"(?i);\s*samesite=none").ok()?,
            })
        })
        .as_ref()
}

/// A local backend's cookie: its path moves under the proxy prefix and `Secure` goes (the proxy is plain http).
pub fn rewrite_local_cookie(cookie: &str, prefix: &str) -> String {
    let Some(rules) = cookie_rules() else {
        return cookie.to_string();
    };
    let mut cookie = cookie.to_string();
    if !prefix.is_empty() && prefix != "/" {
        cookie = rules
            .path
            .replace_all(&cookie, |found: &regex::Captures| {
                format!("{}{prefix}{}", &found[1], &found[2])
            })
            .into_owned();
    }
    rules.secure.replace_all(&cookie, "").into_owned()
}

/// A remote environment's cookie must also drop its domain and cross-site demands to stick on localhost.
pub fn rewrite_external_cookie(cookie: &str, prefix: &str) -> String {
    let Some(rules) = cookie_rules() else {
        return cookie.to_string();
    };
    let mut cookie = cookie.to_string();
    if !prefix.is_empty() && prefix != "/" {
        if rules.has_path.is_match(&cookie) {
            cookie = rules
                .path
                .replace_all(&cookie, |found: &regex::Captures| {
                    format!("{}{prefix}{}", &found[1], &found[2])
                })
                .into_owned();
        } else {
            cookie.push_str(&format!("; Path={prefix}/"));
        }
    }
    let cookie = rules.domain.replace_all(&cookie, "");
    let cookie = rules.secure.replace_all(&cookie, "");
    rules
        .same_site_none
        .replace_all(&cookie, "; SameSite=Lax")
        .into_owned()
}

fn strip_prefix(path: &str, prefix: &str) -> String {
    let rest = path.strip_prefix(prefix).unwrap_or(path);
    if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{rest}")
    }
}

/// The machine state routing reads, behind a trait so tests pin it.
pub trait Machine: Send + Sync {
    fn branches(&self, project: &ProjectRoute, config: &Config) -> Vec<String>;
    fn leases(&self, session: &str) -> Vec<Lease>;
    fn live_port(&self, holder: &str) -> Option<u16>;
    fn holder_alive(&self, holder: &str) -> bool;
    fn service_envs(&self, project_root: &Path, branch: &str) -> pom_layout::WorkspaceState;
}

type BranchMap = (Instant, HashMap<String, String>);
type LeaseCache = (Instant, Vec<Lease>);

/// Routes requests into the open projects, caching what is expensive to look up per request (a dev server
/// fires hundreds of module requests per page load).
pub struct Router {
    machine: Box<dyn Machine>,
    projects: RwLock<Vec<ProjectRoute>>,
    branch_maps: Mutex<HashMap<PathBuf, BranchMap>>,
    leases: Mutex<HashMap<String, LeaseCache>>,
    ports: Mutex<HashMap<String, (Instant, u16)>>,
}

impl Router {
    pub fn new(machine: Box<dyn Machine>) -> Router {
        Router {
            machine,
            projects: RwLock::new(Vec::new()),
            branch_maps: Mutex::new(HashMap::new()),
            leases: Mutex::new(HashMap::new()),
            ports: Mutex::new(HashMap::new()),
        }
    }

    pub fn set_projects(&self, projects: Vec<ProjectRoute>) {
        if let Ok(mut current) = self.projects.write() {
            *current = projects;
        }
    }

    pub fn has_projects(&self) -> bool {
        self.projects
            .read()
            .is_ok_and(|projects| !projects.is_empty())
    }

    fn projects(&self) -> Vec<(ProjectRoute, Arc<Config>)> {
        self.projects
            .read()
            .map(|projects| {
                projects
                    .iter()
                    .filter_map(|project| Some((project.clone(), project.config()?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn branch_for_label(
        &self,
        project: &ProjectRoute,
        config: &Config,
        label: &str,
    ) -> Option<String> {
        let now = Instant::now();
        let mut maps = self.branch_maps.lock().ok()?;
        let fresh = maps
            .get(&project.root)
            .is_some_and(|(at, _)| now.duration_since(*at) < BRANCH_MAP_TTL);
        if !fresh {
            let branches = self.machine.branches(project, config);
            maps.insert(project.root.clone(), (now, branch_labels(&branches)));
        }
        maps.get(&project.root)?.1.get(label).cloned()
    }

    fn session_leases(&self, session: &str) -> Vec<Lease> {
        let now = Instant::now();
        let Ok(mut cache) = self.leases.lock() else {
            return self.machine.leases(session);
        };
        if let Some((at, leases)) = cache.get(session) {
            if now.duration_since(*at) < LEASE_CACHE_TTL {
                return leases.clone();
            }
        }
        let leases = self.machine.leases(session);
        cache.insert(session.to_string(), (now, leases.clone()));
        leases
    }

    fn lease_port(&self, session: &str, key: &str) -> Option<u16> {
        self.session_leases(session)
            .into_iter()
            .find(|lease| lease.key == key)
            .map(|lease| lease.port)
    }

    fn service_port(&self, config: &Config, branch: &str, service_key: &str) -> Option<u16> {
        let cache_key = format!("{}\0{branch}\0{service_key}", config.session);
        let now = Instant::now();
        if let Some((at, port)) = self.ports.lock().ok()?.get(&cache_key) {
            if now.duration_since(*at) < PORT_CACHE_TTL {
                return Some(*port);
            }
        }
        let lease_key = pom_ports::service_key(&pom_env::port_ws_key(branch), service_key);
        let port = pick_port(self.lease_port(&config.session, &lease_key), || {
            self.machine
                .live_port(&service_holder(config, branch, service_key)?)
        })?;
        if let Ok(mut ports) = self.ports.lock() {
            ports.insert(cache_key, (now, port));
        }
        Some(port)
    }

    /// Every listening-capable port the service has across the session's workspaces.
    pub fn service_ports_everywhere(&self, target: &str) -> Option<(String, Vec<u16>)> {
        for (_, config) in self.projects() {
            let Some(service_key) = resolve_service_key(&config, target) else {
                continue;
            };
            let suffix = format!("\u{1f}{service_key}");
            let ports = self
                .session_leases(&config.session)
                .into_iter()
                .filter(|lease| lease.key.ends_with(&suffix) && lease.port > 0)
                .map(|lease| lease.port)
                .collect();
            return Some((service_key, ports));
        }
        None
    }

    fn workspace_route(&self, config: &Config, branch: &str, target: &str, prefix: &str) -> Route {
        let Some(service_key) = resolve_service_key(config, target) else {
            return no_route(branch, target);
        };
        match self.service_port(config, branch, &service_key) {
            Some(port) => Route::Local {
                port,
                prefix: prefix.to_string(),
            },
            None => {
                let starting = service_holder(config, branch, &service_key)
                    .is_some_and(|holder| self.machine.holder_alive(&holder));
                if starting {
                    Route::Error {
                        status: 503,
                        message: format!(
                            "dev-proxy: {target} is still starting (building, not listening yet) - retry shortly"
                        ),
                    }
                } else {
                    no_route(branch, target)
                }
            }
        }
    }

    /// The remote URL a workspace's environment profile points this service at, if it picked one.
    fn override_profile(
        &self,
        project: &ProjectRoute,
        config: &Config,
        branch: &str,
        labels: &[String],
        repo: &str,
        service: &str,
    ) -> Option<(String, String)> {
        if config.environments.is_empty() {
            return None;
        }
        let state = self.machine.service_envs(&project.root, branch);
        let key = if labels.len() == 3 {
            format!("{}/{}", labels[1], labels[0])
        } else {
            format!("{repo}/{service}")
        };
        let profile = state.service_env(&key);
        if profile.is_empty() || profile == "local" {
            return None;
        }
        let url = config
            .environments
            .get(profile)?
            .get(&format!("{repo}.{service}"))?
            .clone();
        (!url.is_empty()).then(|| (profile.to_string(), url))
    }

    /// The project whose workspaces know this branch label and whose repos know the target.
    fn project_for(
        &self,
        label: &str,
        target: &str,
    ) -> Option<(ProjectRoute, Arc<Config>, String)> {
        let mut fallback = None;
        for (project, config) in self.projects() {
            let Some(branch) = self.branch_for_label(&project, &config, label) else {
                continue;
            };
            if resolve_service_key(&config, target).is_some() {
                return Some((project, config, branch));
            }
            fallback.get_or_insert((project, config, branch));
        }
        fallback
    }

    pub fn decide(&self, host: &str, path: &str) -> Decision {
        let unrouted = |route: Route| Decision {
            route,
            path: path.to_string(),
            logged: None,
        };
        let Some(labels) = host_labels(host) else {
            return unrouted(Route::Error {
                status: 404,
                message: format!("open a workspace at http://<service>.<repo>.<branch>.{DOMAIN}"),
            });
        };
        if labels.len() == 2 {
            for (_, config) in self.projects() {
                if labels[1] == config.session && config.shared_services.contains_key(&labels[0]) {
                    return unrouted(self.shared_route(&config, &labels[0]));
                }
            }
        }
        let Some(branch_label) = labels.last().cloned() else {
            return unrouted(no_route_for(host, path));
        };
        if let Some(rest) = path.strip_prefix(DEV_PREFIX) {
            let mut parts = rest.splitn(3, '/');
            let repo = parts.next().unwrap_or_default();
            let service = parts.next().unwrap_or_default();
            if !repo.is_empty() && !service.is_empty() {
                let prefix = format!("{DEV_PREFIX}{repo}/{service}");
                let target = format!("{repo}/{service}");
                let stripped = strip_prefix(path, &prefix);
                let mut logged = Logged {
                    repo: repo.to_string(),
                    service: service.to_string(),
                    profile: "local".into(),
                    target: String::new(),
                };
                let Some((project, config, branch)) = self.project_for(&branch_label, &target)
                else {
                    logged.target = "(no local port)".into();
                    return Decision {
                        route: no_route(&branch_label, &target),
                        path: stripped,
                        logged: Some(logged),
                    };
                };
                if let Some((profile, url)) =
                    self.override_profile(&project, &config, &branch, &labels, repo, service)
                {
                    logged.profile = profile;
                    logged.target = url.clone();
                    return Decision {
                        route: Route::External { url, prefix },
                        path: stripped,
                        logged: Some(logged),
                    };
                }
                let route = self.workspace_route(&config, &branch, &target, &prefix);
                logged.target = match &route {
                    Route::Local { port, .. } => format!("127.0.0.1:{port}"),
                    _ => "(no local port)".into(),
                };
                return Decision {
                    route,
                    path: stripped,
                    logged: Some(logged),
                };
            }
        }
        if labels.len() == 3 {
            let target = format!("{}/{}", labels[1], labels[0]);
            let route = match self.project_for(&branch_label, &target) {
                Some((_, config, branch)) => self.workspace_route(&config, &branch, &target, ""),
                None => no_route(&branch_label, &target),
            };
            return unrouted(route);
        }
        unrouted(no_route_for(host, path))
    }

    fn shared_route(&self, config: &Config, name: &str) -> Route {
        let port = self
            .lease_port(&config.session, &pom_ports::shared_key(name, 0))
            .unwrap_or_else(|| pom_env::stable_shared_port(&config.session, name));
        Route::Local {
            port,
            prefix: String::new(),
        }
    }
}

fn no_route(branch_label: &str, target: &str) -> Route {
    Route::Error {
        status: 502,
        message: format!("no dev-proxy route for {branch_label} / {target}"),
    }
}

fn no_route_for(host: &str, path: &str) -> Route {
    Route::Error {
        status: 404,
        message: format!("no dev-proxy route for {host}{path}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_follow_the_prefix_and_lose_what_breaks_on_localhost() {
        assert_eq!(
            rewrite_local_cookie("sid=1; Path=/; Secure; HttpOnly", "/_pom_dev/api/server"),
            "sid=1; Path=/_pom_dev/api/server/; HttpOnly"
        );
        assert_eq!(
            rewrite_local_cookie("sid=1; path=/app", ""),
            "sid=1; path=/app"
        );
        assert_eq!(
            rewrite_external_cookie(
                "sid=1; Domain=.example.com; Secure; SameSite=None",
                "/_pom_dev/api/server"
            ),
            "sid=1; SameSite=Lax; Path=/_pom_dev/api/server/"
        );
    }

    #[test]
    fn hosts_split_into_labels_and_ticket_labels_must_be_unique() {
        assert_eq!(
            host_labels("server.api.feat-login.localhost:8767"),
            Some(vec!["server".into(), "api".into(), "feat-login".into()])
        );
        assert_eq!(host_labels("example.com"), None);
        let labels = branch_labels(&[
            "PROJ-101-login".into(),
            "PROJ-101-signup".into(),
            "main".into(),
        ]);
        assert_eq!(labels.get("main").map(String::as_str), Some("main"));
        assert!(!labels.contains_key("proj-101"));
        assert_eq!(
            labels
                .get(&pom_env::branch_host("PROJ-101-login"))
                .map(String::as_str),
            Some("PROJ-101-login")
        );
        assert_eq!(pick_port(Some(4000), || Some(5000)), Some(4000));
        assert_eq!(pick_port(None, || Some(5000)), Some(5000));
        assert_eq!(
            strip_prefix("/_pom_dev/api/server", "/_pom_dev/api/server"),
            "/"
        );
        assert_eq!(
            strip_prefix("/_pom_dev/api/server/v1?x", "/_pom_dev/api/server"),
            "/v1?x"
        );
    }
}
