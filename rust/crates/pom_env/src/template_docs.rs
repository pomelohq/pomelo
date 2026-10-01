//! What every `{{...}}` token means, next to the resolver that gives it a value. The docs site's templates
//! reference and the agents' `config_reference` tool both render this table; a test resolves each token.

#[derive(Clone, Copy, Debug)]
pub struct TemplateDoc {
    pub token: &'static str,
    pub doc: &'static str,
    /// A concrete key the resolver must answer, for the test.
    pub sample: &'static str,
    /// Also works in a database name template (`databases:`, `db_name:`).
    pub in_db_names: bool,
}

const fn token(token: &'static str, sample: &'static str, doc: &'static str) -> TemplateDoc {
    TemplateDoc {
        token,
        doc,
        sample,
        in_db_names: false,
    }
}

const fn branch_token(token: &'static str, sample: &'static str, doc: &'static str) -> TemplateDoc {
    TemplateDoc {
        token,
        doc,
        sample,
        in_db_names: true,
    }
}

pub const TEMPLATES: &[TemplateDoc] = &[
    token("{{shared.<name>.url}}", "shared.postgres.url", "The shared service's connection: `user:pass@host:port`, or `host:port` for one with no login (Redis, for example). For a `cmd` shared service, `http://127.0.0.1:<port>`."),
    token("{{shared.<name>.host}}", "shared.postgres.host", "Always `127.0.0.1`: `localhost` can resolve to `::1`, which Docker's published ports miss."),
    token("{{shared.<name>.port}}", "shared.postgres.port", "The shared service's leased port (plus the instance number when `capacity` runs several)."),
    token("{{shared.<name>.user}}", "shared.postgres.user", "The login from `db_user` (`postgres` for a Postgres that sets none; empty otherwise)."),
    token("{{shared.<name>.pass}}", "shared.postgres.pass", "The password from `db_password` (`postgres` for a Postgres that sets none; empty otherwise)."),
    token("{{shared.<name>.slot}}", "shared.redis.slot", "This workspace's slot on a `capacity` service, from 0 (a Redis database number, for example)."),
    token("{{slot.<name>}}", "slot.redis", "Same as `{{shared.<name>.slot}}`."),
    token("{{db.<name>}}", "db.main", "The name of one of the repo's `databases`: session prefix plus the resolved name template."),
    token("{{db.<name>.url}}", "db.main.url", "`postgres://user:pass@host:port/<database>` through the shared Postgres."),
    token("{{<repo>.<service>.url}}", "api.server.url", "The service's URL through the dev proxy, `http://<service>.<repo>.<branch>.localhost:<proxy port>`; or the remote URL when the active `environments` profile lists `<repo>.<service>`. `<repo>` is the alias or the folder name."),
    token("{{<repo>.<service>.path}}", "api.server.path", "The same-origin dev proxy path, `/_pom_dev/<repo>/<service>`, for a frontend calling its API without CORS."),
    token("{{<repo>.<service>.host}}", "api.server.host", "The host part of the service's URL."),
    token("{{<repo>.<service>.port}}", "api.server.port", "The service's leased port (the remote URL's port under a profile)."),
    token("{{<repo>.<service>.ws}}", "api.server.ws", "The service's URL with `ws://` (or `wss://`) for websockets."),
    token("{{secret.<NAME>}}", "secret.API_TOKEN", "A value from the project's secrets store, read when env is written. Never put a secret in `pom.yml`."),
    branch_token("{{branch}}", "branch", "The workspace's branch, as is: `feat/login-form`."),
    branch_token("{{branch.safe}}", "branch.safe", "The branch with `/` turned into `_`; hyphens are kept: `feat/login-form` gives `feat_login-form`."),
    branch_token("{{branch.host}}", "branch.host", "The branch as a DNS label: lowercase, other characters turned into `-`, at most 63 bytes (a hash suffix keeps long names apart)."),
    branch_token("{{branch.hash}}", "branch.hash", "The first 8 hex digits of the branch's SHA-1."),
    token("{{bind_ip}}", "bind_ip", "Always `127.0.0.1`."),
];

/// The rules every template follows, as Markdown.
pub const GRAMMAR: &str = "Templates are written `{{<source>.<name>.<field>}}` (dot notation) and work in any `env:` value, \
service `env:` and `cmd`, and shared service `environment:`. A token the resolver does not know stays in the output as \
written, so a typo shows up in the env file instead of becoming an empty value. A `| filter` after the key is accepted \
but changes nothing.\n\nDatabase name templates (`databases:` and a repo's `shared_services: [{ <name>: { db_name: ... } }]`) \
take only the `branch` tokens; they also accept the older spellings `{{branch_safe}}` and `{{branch|safe}}` (and the same \
for `hash` and `host`), but write `{{branch.safe}}`.\n\nService commands also get `$PORT` (their leased port) and `$BIND_IP` in their \
environment; a `cmd` shared service gets the same.";

/// The colon forms older configs used. Loading a config that has one fails; `config_normalize` (or
/// `pom config normalize`) rewrites the ones with a replacement.
pub const REMOVED_FORMS: &[(&str, &str)] = &[
    ("`{{conn:<name>}}`", "`{{shared.<name>.url}}`"),
    (
        "`{{host:<name>}}`, `{{port:<name>}}`",
        "`{{shared.<name>.host}}`, `{{shared.<name>.port}}`",
    ),
    (
        "`{{user:<name>}}`, `{{pass:<name>}}`",
        "`{{shared.<name>.user}}`, `{{shared.<name>.pass}}`",
    ),
    ("`{{db:<name>}}`", "`{{db.<name>}}`"),
    ("`{{slot:<name>}}`", "`{{shared.<name>.slot}}`"),
    (
        "`{{var:<NAME>}}`, `{{url:...}}`, `{{ws:...}}`",
        "No replacement: use a service ref such as `{{<repo>.<service>.url}}`.",
    ),
];

use pom_config::field_docs::cell;

/// One line per token, for an agent's system prompt.
pub fn token_lines() -> String {
    TEMPLATES
        .iter()
        .map(|doc| format!("- {} - {}", doc.token, doc.doc.replace('`', "")))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn markdown() -> String {
    let mut out = format!(
        "{GRAMMAR}\n\n## Tokens\n\n| Token | In database names | Resolves to |\n|---|---|---|\n"
    );
    for doc in TEMPLATES {
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            cell(doc.token),
            if doc.in_db_names { "yes" } else { "-" },
            cell(doc.doc)
        ));
    }
    out.push_str("\n## Removed colon forms\n\nLoading a config that still has one of these fails. `config_normalize` (or `pom config normalize`) rewrites the ones that have a replacement.\n\n| Removed | Write instead |\n|---|---|\n");
    for (removed, instead) in REMOVED_FORMS {
        out.push_str(&format!("| {} | {} |\n", cell(removed), cell(instead)));
    }
    out
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use pom_config::{Config, Dir, Service, SharedServiceDef};

    use super::*;
    use crate::{resolve_branch_tokens, EnvSources, ResolveContext, SlotAllocation};

    struct Sources;

    impl EnvSources for Sources {
        fn shared_port(&self, _: &str) -> Option<u16> {
            Some(21000)
        }
        fn service_port(&self, _: &str, _: &str) -> Option<u16> {
            Some(4000)
        }
        fn slot(&self, _: &str, _: &str) -> Option<SlotAllocation> {
            Some(SlotAllocation::default())
        }
        fn secret(&self, _: &str, name: &str) -> Option<String> {
            Some(format!("value-of-{name}"))
        }
    }

    fn config() -> Config {
        let mut config = Config::default();
        let mut api = Dir::default();
        api.services.insert("server".into(), Service::default());
        config.repos.insert("api".into(), api);
        for (name, kind) in [("postgres", "postgres"), ("redis", "redis")] {
            config.shared_services.insert(
                name.into(),
                SharedServiceDef {
                    kind: kind.into(),
                    ..SharedServiceDef::default()
                },
            );
        }
        config
    }

    #[test]
    fn every_documented_token_resolves() {
        let config = config();
        let db_names: IndexMap<String, String> =
            [("main".to_string(), "myproject_feat".to_string())].into();
        let context = ResolveContext {
            config: &config,
            branch: "feat/login-form",
            ws_key: "ws",
            env_name: "",
            db_names: &db_names,
            sources: &Sources,
        };
        for doc in TEMPLATES {
            assert!(
                context.lookup(doc.sample).is_some(),
                "{} does not resolve",
                doc.token
            );
            if doc.in_db_names {
                let template = format!("{{{{{}}}}}", doc.sample);
                assert_ne!(
                    resolve_branch_tokens(&template, "feat/login-form"),
                    template,
                    "{}",
                    doc.token
                );
            }
        }
    }

    #[test]
    fn the_documented_branch_example_is_what_branch_safe_gives() {
        assert_eq!(crate::branch_safe("feat/login-form"), "feat_login-form");
    }
}
