use indexmap::IndexMap;

use crate::decode::Decoder;
use crate::yaml_node::{Node, NodeKind};

pub const DEFAULT_SESSION: &str = "pomelo";
pub const DEFAULT_ENV_FILE: &str = ".env.local";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub session: String,
    pub default_branch: String,
    pub repos: IndexMap<String, Dir>,
    pub presets: IndexMap<String, Preset>,
    pub shared_services: IndexMap<String, SharedServiceDef>,
    pub workspaces: IndexMap<String, Vec<String>>,
    pub combinations: IndexMap<String, Vec<String>>,
    pub environments: IndexMap<String, IndexMap<String, String>>,
    pub code_agents: Option<CodeAgentsConfig>,
    pub ui: Option<UiConfig>,
    pub sync: Option<SyncConfig>,
    pub seed: Vec<String>,
    pub prepare_main: Vec<String>,
    /// Preset names whose services run at workspace level rather than inside a repo.
    pub workspace_presets: Vec<String>,
    pub workspace_services: IndexMap<String, Service>,
    pub plugins: IndexMap<String, Node>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dir {
    pub alias: String,
    pub pre_start: String,
    pub shell_env: String,
    pub default_branch: String,
    pub shortcuts: Vec<Shortcut>,
    pub services: IndexMap<String, Service>,
    pub proxy_port: Option<u16>,
    pub profiles: Vec<String>,
    pub copy: Vec<String>,
    pub env: IndexMap<String, String>,
    /// The repo's own base env before presets fill in defaults.
    pub own_env: IndexMap<String, String>,
    pub env_output: Vec<EnvFileEntry>,
    pub shared_refs: Vec<SharedServiceRef>,
    pub databases: IndexMap<String, String>,
    pub presets: Vec<String>,
    pub setup: Vec<String>,
    pub migrate: Vec<String>,
    pub seed: Vec<String>,
    pub seed_from_main: bool,
    pub commands: IndexMap<String, String>,
    pub pre_delete: Vec<String>,
    pub plugins: IndexMap<String, Node>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preset {
    pub presets: Vec<String>,
    pub env: IndexMap<String, String>,
    pub setup: Vec<String>,
    pub seed: Vec<String>,
    pub pre_delete: Vec<String>,
    pub pre_start: String,
    pub copy: Vec<String>,
    pub seed_from_main: bool,
    pub shortcuts: Vec<Shortcut>,
    pub services: IndexMap<String, Service>,
    pub commands: IndexMap<String, String>,
    pub migrate: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Service {
    pub kind: String,
    pub cmd: String,
    pub dir: String,
    pub shell_env: String,
    pub env: IndexMap<String, String>,
    pub pre_start: String,
    pub proxy_port: Option<u16>,
    pub shortcuts: Vec<Shortcut>,
    pub depends_on: Vec<String>,
    pub port: Option<bool>,
    pub modes: IndexMap<String, String>,
    pub mode: String,
    pub profiles: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SharedServiceDef {
    pub kind: String,
    pub image: String,
    pub host: String,
    pub ports: Vec<String>,
    pub environment: IndexMap<String, String>,
    pub volumes: Vec<String>,
    pub command: String,
    pub healthcheck: Option<HealthCheck>,
    pub db_user: String,
    pub db_password: String,
    pub capacity: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HealthCheck {
    /// Either a shell string or a docker-style `[CMD, ...]` list, kept raw.
    pub test: Option<Node>,
    pub interval: String,
    pub timeout: String,
    pub retries: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shortcut {
    pub cmd: String,
    pub desc: String,
    pub key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvFileEntry {
    pub file: String,
    pub env: IndexMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SharedServiceRef {
    pub name: String,
    pub db_name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiConfig {
    pub editor: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncConfig {
    pub auto_push: bool,
    pub interval_sec: i64,
    pub refresh_main: bool,
    pub refresh_interval_sec: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodeAgentsConfig {
    pub disabled: bool,
    pub only: Vec<String>,
    pub notify_disabled: bool,
}

impl Config {
    pub(crate) fn decode(root: &Node, decoder: &mut Decoder) -> Config {
        let mut config = Config {
            session: DEFAULT_SESSION.to_string(),
            ..Config::default()
        };
        let Some(root) = decoder.fields(root, "Config") else {
            return config;
        };
        let field = |name: &str| root.get(name);
        let session = decoder.string(field("session"));
        if !session.is_empty() {
            config.session = session;
        }
        config.default_branch = decoder.string(field("default_branch"));
        config.repos = decoder.map(field("repos"), "map[string]Dir", Dir::decode);
        config.presets = decoder.map(field("presets"), "map[string]Preset", Preset::decode);
        config.shared_services = decoder.map(
            field("shared_services"),
            "map[string]SharedServiceDef",
            SharedServiceDef::decode,
        );
        config.workspaces = decoder.map(field("workspaces"), "map[string][]string", |d, n| {
            d.strings(Some(n))
        });
        config.combinations = decoder.map(field("combinations"), "map[string][]string", |d, n| {
            d.strings(Some(n))
        });
        config.environments = decoder.map(
            field("environments"),
            "map[string]map[string]string",
            |d, n| d.string_map(Some(n)),
        );
        config.code_agents = field("code_agents")
            .and_then(|node| decoder.fields(node, "CodeAgentsConfig"))
            .map(|node| CodeAgentsConfig {
                disabled: decoder.bool(node.get("disabled")),
                only: decoder.strings(node.get("only")),
                notify_disabled: decoder.bool(node.get("notify_disabled")),
            });
        config.ui = field("ui")
            .and_then(|node| decoder.fields(node, "UIConfig"))
            .map(|node| UiConfig {
                editor: decoder.string(node.get("editor")),
            });
        config.sync = field("sync")
            .and_then(|node| decoder.fields(node, "SyncConfig"))
            .map(|node| SyncConfig {
                auto_push: decoder.bool(node.get("auto_push")),
                interval_sec: decoder.int(node.get("interval_sec")),
                refresh_main: decoder.bool(node.get("refresh_main")),
                refresh_interval_sec: decoder.int(node.get("refresh_interval_sec")),
            });
        config.seed = decoder.strings(field("seed"));
        config.prepare_main = decoder.strings(field("prepare_main"));
        config.workspace_presets = preset_names(field("preset"));
        config.plugins = decoder.map(field("plugins"), "map[string]Node", |_, n| n.clone());
        config
    }
}

impl Dir {
    fn decode(decoder: &mut Decoder, node: &Node) -> Dir {
        let mut dir = Dir::default();
        let Some(node) = decoder.fields(node, "Dir") else {
            return dir;
        };
        let field = |name: &str| node.get(name);
        dir.alias = decoder.string(field("alias"));
        dir.pre_start = decoder.string(field("pre_start"));
        dir.shell_env = decoder.string(field("shell_env"));
        dir.default_branch = decoder.string(field("default_branch"));
        dir.shortcuts = decode_shortcuts(decoder, field("shortcuts"));
        dir.services = decoder.map(field("services"), "map[string]Service", Service::decode);
        dir.proxy_port = decoder.opt_u16(field("proxy_port"));
        dir.profiles = decoder.string_list(field("profiles"));
        dir.copy = decoder.strings(field("copy"));
        dir.databases = decoder.string_map(field("databases"));
        dir.setup = decoder.strings(field("setup"));
        dir.migrate = decoder.strings(field("migrate"));
        dir.seed = decoder.strings(field("seed"));
        dir.seed_from_main = decoder.bool(field("seed_from_main"));
        dir.commands = decoder.string_map(field("commands"));
        dir.pre_delete = decoder.strings(field("pre_delete"));
        dir.plugins = decoder.map(field("plugins"), "map[string]Node", |_, n| n.clone());
        if let Some(lifecycle) = field("lifecycle").and_then(|n| decoder.fields(n, "Lifecycle")) {
            dir.fold_lifecycle(decoder, lifecycle);
        }
        (dir.env, dir.env_output) = parse_env(field("env"));
        dir.own_env = dir.env.clone();
        dir.shared_refs = parse_shared_refs(field("shared_services"));
        dir.presets = preset_names(field("preset"));
        dir
    }

    /// A `lifecycle:` block is the grouped spelling of the flat fields; anything it sets wins.
    fn fold_lifecycle(&mut self, decoder: &mut Decoder, block: &Node) {
        let replace = |target: &mut Vec<String>, values: Vec<String>| {
            if !values.is_empty() {
                *target = values;
            }
        };
        replace(&mut self.copy, decoder.strings(block.get("copy")));
        replace(&mut self.setup, decoder.strings(block.get("setup")));
        replace(&mut self.migrate, decoder.strings(block.get("migrate")));
        replace(&mut self.seed, decoder.strings(block.get("seed")));
        replace(
            &mut self.pre_delete,
            decoder.strings(block.get("pre_delete")),
        );
        let pre_start = decoder.string(block.get("pre_start"));
        if !pre_start.is_empty() {
            self.pre_start = pre_start;
        }
        let shortcuts = decode_shortcuts(decoder, block.get("shortcuts"));
        if !shortcuts.is_empty() {
            self.shortcuts = shortcuts;
        }
        let commands = decoder.string_map(block.get("commands"));
        if !commands.is_empty() {
            self.commands = commands;
        }
    }
}

impl Preset {
    fn decode(decoder: &mut Decoder, node: &Node) -> Preset {
        let mut preset = Preset::default();
        let Some(node) = decoder.fields(node, "Preset") else {
            return preset;
        };
        let field = |name: &str| node.get(name);
        preset.presets = preset_names(field("preset"));
        preset.env = decoder.string_map(field("env"));
        preset.setup = decoder.strings(field("setup"));
        preset.seed = decoder.strings(field("seed"));
        preset.pre_delete = decoder.strings(field("pre_delete"));
        preset.pre_start = decoder.string(field("pre_start"));
        preset.copy = decoder.strings(field("copy"));
        preset.seed_from_main = decoder.bool(field("seed_from_main"));
        preset.shortcuts = decode_shortcuts(decoder, field("shortcuts"));
        preset.services = decoder.map(field("services"), "map[string]Service", Service::decode);
        preset.commands = decoder.string_map(field("commands"));
        preset.migrate = decoder.strings(field("migrate"));
        preset
    }
}

impl Service {
    fn decode(decoder: &mut Decoder, node: &Node) -> Service {
        let mut service = Service::default();
        if let Some(cmd) = node.scalar_value() {
            service.cmd = cmd.to_string();
            return service;
        }
        let Some(node) = decoder.fields(node, "Service") else {
            return service;
        };
        let field = |name: &str| node.get(name);
        service.kind = decoder.string(field("type"));
        service.cmd = decoder.string(field("cmd"));
        service.dir = decoder.string(field("dir"));
        service.shell_env = decoder.string(field("shell_env"));
        service.env = decoder.string_map(field("env"));
        service.pre_start = decoder.string(field("pre_start"));
        service.proxy_port = decoder.opt_u16(field("proxy_port"));
        service.shortcuts = decode_shortcuts(decoder, field("shortcuts"));
        service.depends_on = decoder.strings(field("depends_on"));
        service.port = decoder.opt_bool(field("port"));
        service.modes = decoder.string_map(field("modes"));
        service.mode = decoder.string(field("mode"));
        service.profiles = decoder.string_list(field("profiles"));
        service
    }

    pub fn active_cmd(&self, mode_override: &str) -> &str {
        let mode = if mode_override.is_empty() {
            self.mode.as_str()
        } else {
            mode_override
        };
        if !mode.is_empty() {
            if let Some(cmd) = self.modes.get(mode) {
                return cmd;
            }
        }
        &self.cmd
    }

    pub fn mode_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.modes.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn has_port(&self) -> bool {
        self.port
            .unwrap_or(self.kind == "backend" || self.kind == "frontend")
    }
}

impl SharedServiceDef {
    fn decode(decoder: &mut Decoder, node: &Node) -> SharedServiceDef {
        let mut def = SharedServiceDef::default();
        let Some(node) = decoder.fields(node, "SharedServiceDef") else {
            return def;
        };
        let field = |name: &str| node.get(name);
        def.kind = decoder.string(field("type"));
        def.image = decoder.string(field("image"));
        def.host = decoder.string(field("host"));
        def.ports = decoder.strings(field("ports"));
        def.environment = decoder.string_map(field("environment"));
        def.volumes = decoder.strings(field("volumes"));
        def.command = decoder.string(field("command"));
        def.healthcheck = field("healthcheck")
            .and_then(|n| decoder.fields(n, "HealthCheck"))
            .map(|n| HealthCheck {
                test: n.get("test").filter(|test| !test.is_null()).cloned(),
                interval: decoder.string(n.get("interval")),
                timeout: decoder.string(n.get("timeout")),
                retries: decoder.int(n.get("retries")),
            });
        def.db_user = decoder.string(field("db_user"));
        def.db_password = decoder.string(field("db_password"));
        def.capacity = decoder.opt_u16(field("capacity"));
        def
    }
}

fn decode_shortcuts(decoder: &mut Decoder, node: Option<&Node>) -> Vec<Shortcut> {
    let Some(node) = node.filter(|node| !node.is_null()) else {
        return Vec::new();
    };
    let Some(items) = node.items() else {
        decoder.mismatch(node, "[]Shortcut");
        return Vec::new();
    };
    let mut shortcuts = Vec::new();
    for item in items {
        if let Some(item) = decoder.fields(item, "Shortcut") {
            shortcuts.push(Shortcut {
                cmd: decoder.string(item.get("cmd")),
                desc: decoder.string(item.get("desc")),
                key: decoder.string(item.get("key")),
            });
        }
    }
    shortcuts
}

/// `preset: name` or `preset: [a, b]`; anything else names no preset.
pub(crate) fn preset_names(node: Option<&Node>) -> Vec<String> {
    let Some(node) = node else {
        return Vec::new();
    };
    if let Some(items) = node.items() {
        return items
            .iter()
            .filter(|item| matches!(item.kind, NodeKind::Scalar { .. }))
            .map(Node::raw_text)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
    }
    match &node.kind {
        NodeKind::Scalar { value, .. } if !value.is_empty() => vec![value.clone()],
        _ => Vec::new(),
    }
}

/// A flat `env:` map is the base env written to `.env.local`. If any value is itself a map, the
/// keys are file names instead, with `*` holding the base shared by every file.
fn parse_env(node: Option<&Node>) -> (IndexMap<String, String>, Vec<EnvFileEntry>) {
    let Some(entries) = node.and_then(Node::entries).filter(|e| !e.is_empty()) else {
        return (IndexMap::new(), Vec::new());
    };
    let flat = |node: &Node| -> IndexMap<String, String> {
        node.entries()
            .unwrap_or_default()
            .iter()
            .map(|(key, value)| (key.text().to_string(), value.raw_text().to_string()))
            .collect()
    };
    let file_keyed = entries.iter().any(|(_, value)| value.is_mapping());
    if !file_keyed {
        let base = entries
            .iter()
            .map(|(key, value)| (key.text().to_string(), value.raw_text().to_string()))
            .collect();
        let files = vec![EnvFileEntry {
            file: DEFAULT_ENV_FILE.to_string(),
            env: IndexMap::new(),
        }];
        return (base, files);
    }
    let mut base = IndexMap::new();
    let mut files = Vec::new();
    for (key, value) in entries {
        if !value.is_mapping() {
            continue;
        }
        if key.text() == "*" {
            base = flat(value);
        } else {
            files.push(EnvFileEntry {
                file: key.text().to_string(),
                env: flat(value),
            });
        }
    }
    (base, files)
}

fn parse_shared_refs(node: Option<&Node>) -> Vec<SharedServiceRef> {
    let Some(items) = node.and_then(Node::items) else {
        return Vec::new();
    };
    let mut refs = Vec::new();
    for item in items {
        if let Some(entries) = item.entries() {
            for (name, options) in entries {
                refs.push(SharedServiceRef {
                    name: name.text().to_string(),
                    db_name: options
                        .get("db_name")
                        .map(Node::raw_text)
                        .unwrap_or_default()
                        .to_string(),
                });
            }
        } else if let NodeKind::Scalar { value: name, .. } = &item.kind {
            refs.push(SharedServiceRef {
                name: name.to_string(),
                db_name: String::new(),
            });
        }
    }
    refs
}
