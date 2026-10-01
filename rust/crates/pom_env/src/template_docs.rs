//! What every `{{...}}` token means, next to the resolver that gives it a value. The docs site's templates
//! reference and the agents' `config_reference` tool both render this table; a test resolves each token.

#[derive(Clone, Copy, Debug)]
pub struct TemplateDoc {
    pub token: &'static str,
    pub doc: &'static str,
    /// A concrete key, resolved by the test in a workspace on `feat/login` of project `myproject`.
    pub sample: &'static str,
    /// What `sample` resolves to there; the test checks it.
    pub example: &'static str,
    /// Also works in a database name template (`databases:`, `db_name:`).
    pub in_db_names: bool,
}

const fn token(
    token: &'static str,
    sample: &'static str,
    example: &'static str,
    doc: &'static str,
) -> TemplateDoc {
    TemplateDoc {
        token,
        doc,
        sample,
        example,
        in_db_names: false,
    }
}

const fn branch_token(
    token: &'static str,
    sample: &'static str,
    example: &'static str,
    doc: &'static str,
) -> TemplateDoc {
    TemplateDoc {
        token,
        doc,
        sample,
        example,
        in_db_names: true,
    }
}

pub const TEMPLATES: &[TemplateDoc] = &[
    token("{{shared.<name>.url}}", "shared.postgres.url", "postgres:postgres@127.0.0.1:5432", "The shared service's connection: `user:pass@host:port`, or `host:port` for one with no login (Redis, for example). For a `cmd` shared service, `http://127.0.0.1:<port>`. `{{shared.<name>}}` is the same."),
    token("{{shared.<name>.host}}", "shared.postgres.host", "127.0.0.1", "Always `127.0.0.1`: `localhost` can resolve to `::1`, which Docker's published ports miss."),
    token("{{shared.<name>.port}}", "shared.postgres.port", "5432", "The shared service's leased port (plus the instance number when `capacity` runs several)."),
    token("{{shared.<name>.user}}", "shared.postgres.user", "postgres", "The login from `db_user` (`postgres` for a Postgres that sets none; empty otherwise)."),
    token("{{shared.<name>.pass}}", "shared.postgres.pass", "postgres", "The password from `db_password` (`postgres` for a Postgres that sets none; empty otherwise)."),
    token("{{shared.<name>.slot}}", "shared.redis.slot", "3", "This workspace's slot on a `capacity` service, from 0 (a Redis database number, for example); 0 when the service has none."),
    token("{{slot.<name>}}", "slot.redis", "3", "Same as `{{shared.<name>.slot}}`."),
    token("{{db.<name>}}", "db.main", "myproject_feat_login", "The name of one of the repo's `databases`: session prefix plus the resolved name template."),
    token("{{db.<name>.url}}", "db.main.url", "postgres://postgres:postgres@127.0.0.1:5432/myproject_feat_login", "`postgres://user:pass@host:port/<database>` through the shared Postgres."),
    token("{{<repo>.<service>.url}}", "api.server.url", "http://server.api.feat-login.localhost:8767", "The service's URL through the dev proxy, `http://<service>.<repo>.<branch>.localhost:<proxy port>`; or the remote URL when the active `environments` profile lists `<repo>.<service>`. `<repo>` is the alias or the folder name, and `{{<repo>.<service>}}` is the same. A branch that starts with a ticket key uses just the key in the host: `proj-101-login` gives `server.api.proj-101.localhost`."),
    token("{{<repo>.<service>.path}}", "api.server.path", "/_pom_dev/api/server", "The same-origin dev proxy path, `/_pom_dev/<repo>/<service>`, for a frontend calling its API without CORS."),
    token("{{<repo>.<service>.host}}", "api.server.host", "server.api.feat-login.localhost", "The host part of the service's URL."),
    token("{{<repo>.<service>.port}}", "api.server.port", "41000", "The service's leased port (the remote URL's port under a profile)."),
    token("{{<repo>.<service>.ws}}", "api.server.ws", "ws://server.api.feat-login.localhost:8767", "The service's URL with `ws://` (or `wss://`) for websockets."),
    token("{{secret.<NAME>}}", "secret.API_TOKEN", "sk_test_123", "A value from the project's secrets store, read when env is written. Never put a secret in `pom.yml`."),
    branch_token("{{branch}}", "branch", "feat/login", "The workspace's branch, as is."),
    branch_token("{{branch.safe}}", "branch.safe", "feat_login", "The branch with `/` turned into `_`; hyphens are kept: `feat/login-form` gives `feat_login-form`."),
    branch_token("{{branch.host}}", "branch.host", "feat-login", "The branch as a DNS label: lowercase, other characters turned into `-`, at most 63 bytes (a hash suffix keeps long names apart)."),
    branch_token("{{branch.hash}}", "branch.hash", "5d5c6df1", "The first 8 hex digits of the branch's SHA-1."),
    token("{{bind_ip}}", "bind_ip", "127.0.0.1", "Always `127.0.0.1`."),
];

/// The rules every template follows, as Markdown.
pub const GRAMMAR: &str = "Templates are written `{{<source>.<name>.<field>}}` (dot notation). They are resolved in a \
repo's `env:` and a service's `env:`, when Pomelo writes the env files and starts the service; every other value \
(`cmd`, `environments` URLs, shared service `environment:`) is used as written. A token the resolver does not know stays \
in the output as written, so a typo shows up in the env file instead of becoming an empty value. A `| filter` after the \
key is accepted but changes nothing.\n\nDatabase name templates (`databases:` and a repo's `shared_services: [{ <name>: \
{ db_name: ... } }]`) take only the `branch` tokens; they also accept the older spellings `{{branch_safe}}` and \
`{{branch|safe}}` (and the same for `hash` and `host`), but write `{{branch.safe}}`.\n\nIn a `cmd`, use `$PORT` (the \
service's leased port), `$BIND_IP` or any env var the service gets; a `cmd` shared service gets `$PORT` and `$BIND_IP` too.";

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
        "{GRAMMAR}\n\n## Tokens\n\nExamples are for branch `feat/login` of project `myproject`.\n\n| Token | Resolves to | Example | In database names |\n|---|---|---|---|\n"
    );
    for doc in TEMPLATES {
        out.push_str(&format!(
            "| `{}` | {} | `{}` | {} |\n",
            cell(doc.token),
            cell(doc.doc),
            cell(doc.example),
            if doc.in_db_names { "yes" } else { "-" }
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
            Some(5432)
        }
        fn service_port(&self, _: &str, _: &str) -> Option<u16> {
            Some(41000)
        }
        fn slot(&self, _: &str, _: &str) -> Option<SlotAllocation> {
            Some(SlotAllocation {
                instance: 0,
                slot: 3,
            })
        }
        fn secret(&self, _: &str, _: &str) -> Option<String> {
            Some("sk_test_123".to_string())
        }
    }

    fn config() -> Config {
        let mut config = Config {
            session: "myproject".into(),
            ..Config::default()
        };
        let mut api = Dir::default();
        api.services.insert("server".into(), Service::default());
        api.databases
            .insert("main".into(), "{{branch.safe}}".into());
        config.repos.insert("api".into(), api);
        for name in ["postgres", "redis"] {
            config.shared_services.insert(
                name.into(),
                SharedServiceDef {
                    kind: name.into(),
                    ..SharedServiceDef::default()
                },
            );
        }
        config
    }

    #[test]
    fn every_documented_token_resolves_to_its_example() {
        const BRANCH: &str = "feat/login";
        let config = config();
        let db_names: IndexMap<String, String> = config.repos["api"]
            .databases
            .iter()
            .map(|(name, template)| {
                let resolved = resolve_branch_tokens(template, BRANCH);
                (name.clone(), format!("{}_{resolved}", config.session))
            })
            .collect();
        let context = ResolveContext {
            config: &config,
            branch: BRANCH,
            ws_key: "ws",
            env_name: "",
            db_names: &db_names,
            sources: &Sources,
        };
        for doc in TEMPLATES {
            assert_eq!(
                context.lookup(doc.sample).as_deref(),
                Some(doc.example),
                "{}",
                doc.token
            );
            if doc.in_db_names {
                let template = format!("{{{{{}}}}}", doc.sample);
                assert_eq!(
                    resolve_branch_tokens(&template, BRANCH),
                    doc.example,
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
