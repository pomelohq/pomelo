use indexmap::IndexMap;

use crate::schema::Config;

impl Config {
    /// Load-time checks that turn a silent typo into a loud error: removed colon-form templates
    /// and profiles that name no defined environment. Every problem is reported, sorted.
    pub fn validate(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        for (repo, dir) in &self.repos {
            let context = format!("repo {repo:?}");
            // The webhook relay keeps `/_<name>` paths for itself (`/_pom/github`).
            for (what, name) in [("name", repo), ("alias", &dir.alias)] {
                if name.starts_with('_') {
                    errors.push(format!(
                        "{context}: the {what} {name:?} may not start with `_` (the webhook relay keeps those paths)"
                    ));
                }
            }
            let mut scan = |place: &str, values: &IndexMap<String, String>| {
                for (key, value) in values {
                    for (namespace, name) in colon_template_refs(value) {
                        errors.push(colon_ref_error(
                            &format!("{context} {place}.{key}"),
                            &namespace,
                            &name,
                        ));
                    }
                }
            };
            scan("env", &dir.env);
            for entry in &dir.env_output {
                scan(&entry.file, &entry.env);
            }
            for (service_name, service) in &dir.services {
                scan(&format!("service:{service_name}"), &service.env);
            }
            if let Some(test) = &dir.test {
                scan("test.env", &test.env);
                let path = std::path::Path::new(&test.env_file);
                if path.is_absolute()
                    || path
                        .components()
                        .any(|part| matches!(part, std::path::Component::ParentDir))
                {
                    errors.push(format!(
                        "{context} test.env_file: {:?} must be a path inside the repo",
                        test.env_file
                    ));
                }
            }
            self.check_profiles(&mut errors, &context, &dir.profiles);
            for (service_name, service) in &dir.services {
                self.check_profiles(
                    &mut errors,
                    &format!("{context} service {service_name}"),
                    &service.profiles,
                );
            }
        }
        self.check_shared_services(&mut errors);
        self.limits.check(&mut errors);
        if let Some(sync) = &self.sync {
            if crate::OnPush::parse(&sync.on_push).is_none() {
                errors.push(format!(
                    "sync.on_push: {:?} is not one of {}",
                    sync.on_push,
                    crate::OnPush::NAMES.join(", ")
                ));
            }
        }
        if errors.is_empty() {
            return Ok(());
        }
        errors.sort();
        Err(format!("invalid pom.yml:\n  - {}", errors.join("\n  - ")))
    }

    /// A shared service is a Docker `image` or a `cmd`, each with only its own fields, on a port of its own.
    fn check_shared_services(&self, errors: &mut Vec<String>) {
        let mut owners: IndexMap<u16, Vec<(String, bool)>> = IndexMap::new();
        for (name, def) in &self.shared_services {
            let context = format!("shared service {name:?}");
            def.limit.check(&context, errors);
            match (def.image.is_empty(), def.cmd.is_empty()) {
                (false, false) => errors.push(format!(
                    "{context}: set either image (a Docker container) or cmd (a command), not both"
                )),
                (true, true) => errors.push(format!(
                    "{context}: needs an image (a Docker container) or a cmd (a command)"
                )),
                _ => {}
            }
            if def.is_command() {
                let docker_only = [
                    ("ports", !def.ports.is_empty()),
                    ("volumes", !def.volumes.is_empty()),
                    ("command", !def.command.is_empty()),
                    ("capacity", def.capacity.is_some()),
                    ("slot_reset", !def.slot_reset.is_empty()),
                    ("db_user", !def.db_user.is_empty()),
                    ("db_password", !def.db_password.is_empty()),
                    ("cpu and memory", !def.limit.is_empty()),
                ];
                for (field, set) in docker_only {
                    if set {
                        errors.push(format!(
                            "{context}: {field} is only for an image; a cmd service uses port"
                        ));
                    }
                }
                if !def.repo.is_empty() && !self.repos.contains_key(&def.repo) {
                    errors.push(format!("{context}: repo {:?} is not in repos", def.repo));
                }
            } else {
                for (field, set) in [("repo", !def.repo.is_empty()), ("port", def.port.is_some())] {
                    if set {
                        errors.push(format!(
                            "{context}: {field} is only for a cmd; an image service uses ports"
                        ));
                    }
                }
            }
            for port in def
                .port
                .into_iter()
                .chain(def.ports.iter().filter_map(|mapping| {
                    mapping
                        .split([':', '/'])
                        .next()
                        .and_then(|port| port.trim().parse::<u16>().ok())
                }))
            {
                owners
                    .entry(port)
                    .or_default()
                    .push((name.clone(), def.is_command()));
            }
        }
        // Docker services that ask for the same port are leased apart; a cmd's port is the one it is told.
        for (port, names) in owners {
            if names.len() > 1 && names.iter().any(|(_, command)| *command) {
                let names: Vec<String> = names
                    .into_iter()
                    .map(|(name, _)| format!("{name:?}"))
                    .collect();
                errors.push(format!(
                    "shared services {} all want port {port}",
                    names.join(", ")
                ));
            }
        }
    }

    fn check_profiles(&self, errors: &mut Vec<String>, context: &str, profiles: &[String]) {
        for profile in profiles {
            if profile != "local" && !self.environments.contains_key(profile) {
                errors.push(format!("{context}: environment {profile:?} not defined"));
            }
        }
    }
}

fn colon_template_refs(mut text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while let Some(start) = text.find("{{") {
        let Some(length) = text[start..].find("}}") else {
            break;
        };
        let inner = text[start + 2..start + length].trim();
        text = &text[start + length + 2..];
        let Some((namespace, name)) = inner.split_once(':') else {
            continue;
        };
        let name = name.split('|').next().unwrap_or_default();
        out.push((namespace.trim().to_string(), name.trim().to_string()));
    }
    out
}

fn colon_ref_error(context: &str, namespace: &str, name: &str) -> String {
    let replacement = match namespace {
        "conn" => Some("{{shared.NAME.url}}"),
        "host" => Some("{{shared.NAME.host}}"),
        "port" => Some("{{shared.NAME.port}} (or {{<repo>.<svc>.port}})"),
        "user" => Some("{{shared.NAME.user}}"),
        "pass" => Some("{{shared.NAME.pass}}"),
        "slot" => Some("{{shared.NAME.slot}} (or {{slot.NAME}})"),
        "db" => Some("{{db.NAME}}"),
        "var" | "url" | "ws" => None,
        _ => {
            return format!(
                "{context}: {{{{{namespace}:{name}}}}} - colon-form templates are removed; use dot notation"
            );
        }
    };
    match replacement {
        Some(replacement) => {
            format!("{context}: {{{{{namespace}:{name}}}}} is removed - use {replacement}")
        }
        None => format!(
            "{context}: {{{{{namespace}:{name}}}}} is removed with no replacement - use a dot-notation ref (e.g. {{{{<repo>.<svc>.url}}}})"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_colon_refs_and_strips_filters() {
        assert_eq!(
            colon_template_refs("a {{ db:main | upper }} b {{shared.pg.url}} {{conn:pg}}"),
            vec![
                ("db".to_string(), "main".to_string()),
                ("conn".to_string(), "pg".to_string())
            ]
        );
        assert!(colon_template_refs("{{unterminated").is_empty());
    }

    #[test]
    fn error_messages_name_the_replacement() {
        assert_eq!(
            colon_ref_error("repo \"r\" env.A", "db", "x"),
            "repo \"r\" env.A: {{db:x}} is removed - use {{db.NAME}}"
        );
        assert!(colon_ref_error("c", "var", "x").contains("no replacement"));
        assert!(colon_ref_error("c", "weird", "x").contains("colon-form templates are removed"));
    }

    #[test]
    fn relay_paths_and_on_push_values_are_checked() {
        let mut config = Config::default();
        config
            .repos
            .insert("_internal".into(), crate::Dir::default());
        config.repos.insert(
            "api".into(),
            crate::Dir {
                alias: "_api".into(),
                ..crate::Dir::default()
            },
        );
        config.sync = Some(crate::SyncConfig {
            on_push: "sometimes".into(),
            ..crate::SyncConfig::default()
        });
        let errors = config.validate().expect_err("invalid");
        assert!(errors.contains("the name \"_internal\""), "{errors}");
        assert!(errors.contains("the alias \"_api\""), "{errors}");
        assert!(errors.contains("sync.on_push"), "{errors}");

        config.repos.clear();
        for (text, mode) in [
            ("pull", crate::OnPush::Pull),
            ("", crate::OnPush::Notify),
            ("off", crate::OnPush::Off),
        ] {
            config.sync = Some(crate::SyncConfig {
                on_push: text.into(),
                ..crate::SyncConfig::default()
            });
            assert_eq!(config.validate(), Ok(()));
            assert_eq!(config.on_push(), mode);
        }
        assert_eq!(Config::default().on_push(), crate::OnPush::Notify);
    }
}
