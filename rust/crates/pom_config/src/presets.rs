use std::collections::HashSet;

use indexmap::IndexMap;

use crate::schema::{Config, Dir, HealthCheck, Preset, SharedServiceDef};
use crate::yaml_node::Node;

impl Config {
    /// Presets only fill what the repo left unset; nested `preset:` lists expand depth-first so a
    /// composed preset's parts apply before the preset itself.
    pub(crate) fn apply_presets(&mut self) {
        let presets = &self.presets;
        for dir in self.repos.values_mut() {
            for name in flatten_presets(presets, &dir.presets) {
                if let Some(preset) = presets.get(&name) {
                    apply_preset(dir, preset);
                }
            }
        }
    }

    pub(crate) fn apply_workspace_presets(&mut self) {
        let mut services = IndexMap::new();
        for name in &self.workspace_presets {
            let Some(preset) = self.presets.get(name) else {
                continue;
            };
            for (service_name, service) in &preset.services {
                services
                    .entry(service_name.clone())
                    .or_insert_with(|| service.clone());
            }
        }
        self.workspace_services = services;
    }

    /// Well-known shared services (keyed by `type:` or else by name) get a working image, ports,
    /// volumes and credentials so a bare `postgres:` entry is enough.
    pub(crate) fn apply_well_known_defaults(&mut self) {
        for (name, def) in self.shared_services.iter_mut() {
            let kind = if def.kind.is_empty() {
                name.as_str()
            } else {
                def.kind.as_str()
            };
            if let Some(template) = well_known_service(kind) {
                fill_shared_defaults(def, template);
            }
        }
    }
}

fn flatten_presets(presets: &IndexMap<String, Preset>, names: &[String]) -> Vec<String> {
    fn visit(
        presets: &IndexMap<String, Preset>,
        names: &[String],
        seen: &mut HashSet<String>,
        out: &mut Vec<String>,
    ) {
        for name in names {
            if !seen.insert(name.clone()) {
                continue;
            }
            if let Some(preset) = presets.get(name) {
                visit(presets, &preset.presets, seen, out);
            }
            out.push(name.clone());
        }
    }
    let mut out = Vec::new();
    visit(presets, names, &mut HashSet::new(), &mut out);
    out
}

fn apply_preset(dir: &mut Dir, preset: &Preset) {
    fn fill<T: Clone>(target: &mut Vec<T>, source: &[T]) {
        if target.is_empty() {
            *target = source.to_vec();
        }
    }
    fn fill_map(target: &mut IndexMap<String, String>, source: &IndexMap<String, String>) {
        for (key, value) in source {
            target.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    fill_map(&mut dir.env, &preset.env);
    fill(&mut dir.setup, &preset.setup);
    fill(&mut dir.seed, &preset.seed);
    fill(&mut dir.pre_delete, &preset.pre_delete);
    if dir.pre_start.is_empty() {
        dir.pre_start = preset.pre_start.clone();
    }
    fill(&mut dir.copy, &preset.copy);
    dir.seed_from_main |= preset.seed_from_main;
    fill(&mut dir.shortcuts, &preset.shortcuts);
    fill(&mut dir.migrate, &preset.migrate);
    fill_map(&mut dir.commands, &preset.commands);
    for (name, service) in &preset.services {
        dir.services
            .entry(name.clone())
            .or_insert_with(|| service.clone());
    }
}

fn well_known_service(kind: &str) -> Option<SharedServiceDef> {
    let strings = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
    let map = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<IndexMap<_, _>>()
    };
    let def = match kind {
        "postgres" => SharedServiceDef {
            image: "postgres:16".into(),
            ports: strings(&["5432"]),
            command: "postgres -c shared_preload_libraries=pg_stat_statements".into(),
            environment: map(&[
                ("POSTGRES_USER", "postgres"),
                ("POSTGRES_PASSWORD", "postgres"),
                ("POSTGRES_DB", "postgres"),
            ]),
            volumes: strings(&["shared_postgres:/var/lib/postgresql/data"]),
            healthcheck: Some(HealthCheck {
                test: Some(Node::scalar("pg_isready -U postgres -h 127.0.0.1")),
                interval: "5s".into(),
                timeout: "3s".into(),
                retries: 3,
            }),
            db_user: "postgres".into(),
            db_password: "postgres".into(),
            ..SharedServiceDef::default()
        },
        "redis" => SharedServiceDef {
            image: "redis:7-alpine".into(),
            ports: strings(&["6379"]),
            command: "redis-server --appendonly yes".into(),
            volumes: strings(&["shared_redis:/data"]),
            capacity: Some(16),
            ..SharedServiceDef::default()
        },
        "minio" => SharedServiceDef {
            image: "minio/minio".into(),
            command: r#"server /data --console-address ":9001""#.into(),
            ports: strings(&["9000", "9001"]),
            environment: map(&[
                ("MINIO_ROOT_USER", "minioadmin"),
                ("MINIO_ROOT_PASSWORD", "minioadmin"),
            ]),
            volumes: strings(&["shared_minio:/data"]),
            db_user: "minioadmin".into(),
            db_password: "minioadmin".into(),
            ..SharedServiceDef::default()
        },
        "opensearch" => SharedServiceDef {
            image: "opensearchproject/opensearch:2.11.0".into(),
            ports: strings(&["9200", "9600"]),
            environment: map(&[
                ("discovery.type", "single-node"),
                ("DISABLE_SECURITY_PLUGIN", "true"),
                ("OPENSEARCH_JAVA_OPTS", "-Xms256m -Xmx256m"),
            ]),
            volumes: strings(&["shared_opensearch:/usr/share/opensearch/data"]),
            ..SharedServiceDef::default()
        },
        "zincsearch" => SharedServiceDef {
            image: "public.ecr.aws/zinclabs/zincsearch:latest".into(),
            ports: strings(&["4080"]),
            environment: map(&[
                ("ZINC_FIRST_ADMIN_USER", "admin"),
                ("ZINC_FIRST_ADMIN_PASSWORD", "admin"),
                ("ZINC_DATA_PATH", "/data"),
            ]),
            volumes: strings(&["shared_zincsearch:/data"]),
            db_user: "admin".into(),
            db_password: "admin".into(),
            ..SharedServiceDef::default()
        },
        _ => return None,
    };
    Some(def)
}

fn fill_shared_defaults(def: &mut SharedServiceDef, template: SharedServiceDef) {
    if def.image.is_empty() {
        def.image = template.image;
    }
    if def.ports.is_empty() {
        def.ports = template.ports;
    }
    if def.command.is_empty() {
        def.command = template.command;
    }
    if def.volumes.is_empty() {
        def.volumes = template.volumes;
    }
    if def.healthcheck.is_none() {
        def.healthcheck = template.healthcheck;
    }
    if def.db_user.is_empty() {
        def.db_user = template.db_user;
    }
    if def.db_password.is_empty() {
        def.db_password = template.db_password;
    }
    if def.capacity.is_none() {
        def.capacity = template.capacity;
    }
    if !template.environment.is_empty() {
        let mut merged = template.environment;
        merged.extend(std::mem::take(&mut def.environment));
        def.environment = merged;
    }
}
