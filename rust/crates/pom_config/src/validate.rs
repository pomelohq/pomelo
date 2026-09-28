use indexmap::IndexMap;

use crate::schema::Config;

impl Config {
    /// Load-time checks that turn a silent typo into a loud error: removed colon-form templates
    /// and profiles that name no defined environment. Every problem is reported, sorted.
    pub fn validate(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        for (repo, dir) in &self.repos {
            let context = format!("repo {repo:?}");
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
            self.check_profiles(&mut errors, &context, &dir.profiles);
            for (service_name, service) in &dir.services {
                self.check_profiles(
                    &mut errors,
                    &format!("{context} service {service_name}"),
                    &service.profiles,
                );
            }
        }
        if errors.is_empty() {
            return Ok(());
        }
        errors.sort();
        Err(format!("invalid pom.yml:\n  - {}", errors.join("\n  - ")))
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
}
