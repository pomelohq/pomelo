//! What every pom.yml key means, in one table next to the parser. The docs site's pom.yml reference and the
//! agents' `config_reference` tool both render it, and a test fails when the parser reads a key that has no
//! entry here, or an entry names a key the parser no longer reads.

/// Where a key sits in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Section {
    Root,
    Repo,
    Lifecycle,
    Service,
    Task,
    RepoSharedRef,
    Shared,
    HealthCheck,
    Preset,
    Sync,
    CodeAgents,
    Ui,
}

impl Section {
    pub const ALL: [Section; 12] = [
        Section::Root,
        Section::Repo,
        Section::Lifecycle,
        Section::Service,
        Section::Task,
        Section::RepoSharedRef,
        Section::Shared,
        Section::HealthCheck,
        Section::Preset,
        Section::Sync,
        Section::CodeAgents,
        Section::Ui,
    ];

    /// The path a key of this section is written under.
    pub fn prefix(self) -> &'static str {
        match self {
            Section::Root => "",
            Section::Repo => "repos.<repo>.",
            Section::Lifecycle => "repos.<repo>.lifecycle.",
            Section::Service => "repos.<repo>.services.<service>.",
            Section::Task => "tasks[].",
            Section::RepoSharedRef => "repos.<repo>.shared_services[].<name>.",
            Section::Shared => "shared_services.<name>.",
            Section::HealthCheck => "shared_services.<name>.healthcheck.",
            Section::Preset => "presets.<preset>.",
            Section::Sync => "sync.",
            Section::CodeAgents => "code_agents.",
            Section::Ui => "ui.",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Section::Root => "Top level",
            Section::Repo => "Repos",
            Section::Lifecycle => "Repo lifecycle",
            Section::Service => "Services",
            Section::Task => "Tasks",
            Section::RepoSharedRef => "A repo's shared services",
            Section::Shared => "Shared services",
            Section::HealthCheck => "Shared service healthcheck",
            Section::Preset => "Presets",
            Section::Sync => "Sync",
            Section::CodeAgents => "code_agents (ignored)",
            Section::Ui => "ui (ignored)",
        }
    }

    pub fn intro(self) -> &'static str {
        match self {
            Section::Root => "The keys at the top of `pom.yml`.",
            Section::Repo => "Each entry of `repos:` is one repository, keyed by its clone folder name.",
            Section::Lifecycle => "A repo's `lifecycle:` block groups how it is built and run. Every key here also works directly on the repo; when both are set, the `lifecycle:` value wins.",
            Section::Service => "Each entry of a repo's (or preset's) `services:` is one long-running process. `name: <cmd>` is short for `name: { cmd: <cmd> }`.",
            Section::Task => "A `tasks:` list (older name `shortcuts:`) on a repo, its lifecycle, a service or a preset adds quick commands to the app and the agents.",
            Section::RepoSharedRef => "A repo's `shared_services:` lists the shared services it uses: a name, or `name: { db_name: <template> }`.",
            Section::Shared => "Each entry of `shared_services:` runs once for every workspace: a Docker `image` or a `cmd`, never both. `postgres`, `redis`, `minio`, `opensearch` and `zincsearch` (by name or `type:`) get a working image, ports, credentials and healthcheck filled in.",
            Section::HealthCheck => "When the shared service counts as up.",
            Section::Preset => "Each entry of `presets:` is a reusable repo fragment. A repo (or the workspace, with the top-level `preset:`) names presets; a preset only fills what the repo left unset.",
            Section::Sync => "Keep Main Fresh: pulling the main workspace on a schedule.",
            Section::CodeAgents => "Read so older files load, but nothing uses it: agent behavior is in the app's Settings.",
            Section::Ui => "Read so older files load, but nothing uses it: the app's Settings hold this.",
        }
    }
}

/// Whether a key does anything today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Current,
    /// Parsed so an older file loads, but nothing uses it.
    Ignored,
    /// No longer parsed; `config_normalize` deletes it when it can.
    Removed,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldDoc {
    pub section: Section,
    pub key: &'static str,
    pub kind: &'static str,
    pub default: &'static str,
    pub doc: &'static str,
    pub example: &'static str,
    pub status: Status,
}

const fn field(
    section: Section,
    key: &'static str,
    kind: &'static str,
    default: &'static str,
    doc: &'static str,
    example: &'static str,
) -> FieldDoc {
    FieldDoc {
        section,
        key,
        kind,
        default,
        doc,
        example,
        status: Status::Current,
    }
}

const fn ignored(
    section: Section,
    key: &'static str,
    kind: &'static str,
    doc: &'static str,
) -> FieldDoc {
    FieldDoc {
        section,
        key,
        kind,
        default: "",
        doc,
        example: "",
        status: Status::Ignored,
    }
}

const fn removed(section: Section, key: &'static str, doc: &'static str) -> FieldDoc {
    FieldDoc {
        section,
        key,
        kind: "",
        default: "",
        doc,
        example: "",
        status: Status::Removed,
    }
}

use Section::*;

pub const FIELDS: &[FieldDoc] = &[
    field(Root, "session", "string", "pomelo", "The project's namespace: database names and hostnames start with it.", "session: myproject"),
    field(Root, "default_branch", "string", "", "The branch the main workspace follows.", "default_branch: main"),
    field(Root, "repos", "map", "", "The project's repositories; see Repos.", ""),
    field(Root, "shared_services", "map", "", "Services one instance of serves every workspace; see Shared services.", ""),
    field(Root, "presets", "map", "", "Reusable repo fragments; see Presets.", ""),
    field(Root, "environments", "map of maps", "", "Profiles that switch services to remote URLs: `<profile>: { <repo>.<service>: <url> }`. Only the listed services switch; `local` is the profile that switches none.", "environments:\n  staging:\n    api.server: https://api.staging.example.com"),
    field(Root, "preset", "string or list", "", "Presets whose services run once per workspace, outside any repo.", "preset: [gateway]"),
    field(Root, "seed", "list of commands", "", "Runs once in the workspace folder when a workspace is created, before each repo's seed.", "seed: [./scripts/seed-all.sh]"),
    field(Root, "prepare_main", "list", "reset, migrate, seed", "The phases Prepare Main runs, in order: `reset`, `migrate` and `seed`. Other names are skipped; a list with none of them runs only `reset`.", "prepare_main: [migrate, seed]"),
    field(Root, "sync", "map", "", "Keep Main Fresh; see Sync.", ""),
    ignored(Root, "workspaces", "map", "Workspace groups; nothing reads them now."),
    ignored(Root, "combinations", "map", "Repo combinations; `config_normalize` deletes them."),
    ignored(Root, "code_agents", "map", "Agent switches; the app's Settings hold these now."),
    ignored(Root, "ui", "map", "UI preferences; the app's Settings hold these now."),
    ignored(Root, "plugins", "map", "Plugin settings; `config_normalize` deletes them."),
    removed(Root, "schema_version", "Version marker of an older format."),
    removed(Root, "proxy", "Routing is automatic: `/_pom_dev/<repo>/<service>` and `<service>.<repo>.<branch>.localhost`."),
    removed(Root, "webhook", "Webhooks are routed automatically at `/<repo>/<service>`."),
    removed(Root, "global_services", "Replaced by `shared_services`."),
    removed(Root, "shared_stable_ports", "Shared service ports are leased automatically."),
    removed(Root, "e2e", "Removed with `exposes:` and `{{var:}}`."),
    removed(Root, "jira", "The app's Settings hold the Jira connection."),
    removed(Root, "archive", "Removed."),
    field(Repo, "alias", "string", "the repo's key", "The name templates and hostnames use for this repo.", "alias: api"),
    field(Repo, "default_branch", "string", "the top-level default_branch", "This repo's main branch, when it differs from the project's.", ""),
    field(Repo, "preset", "string or list", "", "Presets to apply; they only fill what the repo left unset, in order.", "preset: [rails]"),
    field(Repo, "databases", "map", "", "Databases each workspace gets: `<name>: <name template>`; the session prefix is added. Reach one with `{{db.<name>}}`. The template takes `{{branch}}`, `{{branch.safe}}`, `{{branch.host}}` and `{{branch.hash}}`.", "databases:\n  main: \"{{branch.safe}}\"\n  test: \"{{branch.safe}}_test\""),
    field(Repo, "shared_services", "list", "", "The shared services this repo uses; see A repo's shared services. A declared shared service no repo lists is flagged by `config_doctor` (`shared.unwired`).", "shared_services: [postgres, redis]"),
    field(Repo, "seed_from_main", "bool", "false", "New workspaces copy this repo's databases from the main workspace instead of starting empty, and skip its seed.", "seed_from_main: true"),
    field(Repo, "profiles", "list", "", "The `environments` profiles this repo's services may switch between; `local` is always offered. Each must be defined in `environments`.", "profiles: [staging]"),
    ignored(Repo, "proxy_port", "port", "Only a fallback port for a template that names no service; service ports are leased."),
    field(Repo, "shell_env", "string", "", "`KEY=value ...` put before every service command of the repo.", "shell_env: RAILS_LOG_TO_STDOUT=1"),
    field(Repo, "env", "map", "", "The repo's environment, written to `.env.local` with templates resolved. If any value is a map, the keys are file names instead and `*` holds the base every file starts from. Edit it here, never in the generated file.", "env:\n  DATABASE_URL: postgresql://{{shared.postgres.url}}/{{db.main}}"),
    field(Repo, "services", "map", "", "The repo's long-running processes; see Services.", ""),
    field(Repo, "lifecycle", "map", "", "How the repo is built and run; see Repo lifecycle.", ""),
    field(Repo, "pre_start", "command", "", "Same as `lifecycle.pre_start`.", ""),
    field(Repo, "commands", "map", "", "Same as `lifecycle.commands`.", ""),
    field(Repo, "setup", "list of commands", "", "Same as `lifecycle.setup`.", ""),
    field(Repo, "migrate", "list of commands", "", "Same as `lifecycle.migrate`.", ""),
    field(Repo, "seed", "list of commands", "", "Same as `lifecycle.seed`.", ""),
    field(Repo, "pre_delete", "list of commands", "", "Same as `lifecycle.pre_delete`.", ""),
    field(Repo, "copy", "list of globs", "", "Same as `lifecycle.copy`.", ""),
    field(Repo, "tasks", "list", "", "Same as `lifecycle.tasks`.", ""),
    field(Repo, "shortcuts", "list", "", "Older name of `tasks`.", ""),
    ignored(Repo, "plugins", "map", "Plugin settings; `config_normalize` deletes them."),
    removed(Repo, "exposes", "Published a `{{var:}}` variable; use a service ref such as `{{<repo>.<service>.url}}`."),
    removed(Repo, "env_switch", "Profiles switch services through `environments`."),
    field(Lifecycle, "pre_start", "command", "", "Runs before every service of the repo starts, in the same shell (for example `nvm use`).", "pre_start: nvm use"),
    field(Lifecycle, "commands", "map", "", "Named commands. Each becomes a task; `install`, `generate` and `migrate`, in that order, are the setup a new workspace runs unless `setup` is set, and `migrate` is what Prepare Main and refreshes run unless `migrate` is set.", "commands:\n  install: bundle install\n  migrate: bin/rails db:migrate\n  test: bin/rspec"),
    field(Lifecycle, "setup", "list of commands", "commands install, generate, migrate", "What a new workspace runs in this repo after checkout, joined with `&&`.", "setup: [npm ci, npm run build]"),
    field(Lifecycle, "migrate", "list of commands", "commands.migrate", "How the repo migrates its databases.", ""),
    field(Lifecycle, "seed", "list of commands", "", "Runs after setup when a workspace is created; skipped when `seed_from_main` copies the databases.", "seed: [bin/rails db:seed]"),
    field(Lifecycle, "pre_delete", "list of commands", "", "Runs in the repo before its workspace is deleted.", ""),
    field(Lifecycle, "copy", "list of globs", "", "Files copied from the main checkout into a new workspace (untracked config, for example).", "copy: [config/master.key]"),
    field(Lifecycle, "tasks", "list", "", "Quick commands; see Tasks.", ""),
    field(Lifecycle, "shortcuts", "list", "", "Older name of `tasks`.", ""),
    removed(Lifecycle, "create", "Workspace create runs `setup` (or the install, generate and migrate commands)."),
    removed(Lifecycle, "refresh", "Refreshes run `migrate`."),
    field(Service, "cmd", "command", "", "The command that runs the service. It gets `$PORT` and `$BIND_IP`.", "cmd: bin/rails s -p $PORT -b $BIND_IP"),
    field(Service, "type", "backend, frontend or worker", "", "A backend or frontend gets a `$PORT` unless `port: false`; a worker gets none unless `port: true`.", "type: backend"),
    field(Service, "dir", "path", "the repo folder", "The folder, inside the repo, the service runs in (a monorepo app).", "dir: apps/web"),
    field(Service, "port", "bool", "true for backend and frontend", "Whether the service gets a leased `$PORT`.", "port: false"),
    field(Service, "depends_on", "list", "", "Services of the same repo that start first.", "depends_on: [server]"),
    field(Service, "env", "map", "", "Environment for this service only, on top of the repo's.", ""),
    field(Service, "pre_start", "command", "the repo's pre_start", "Replaces the repo's `pre_start` for this service.", ""),
    field(Service, "shell_env", "string", "the repo's shell_env", "Replaces the repo's `shell_env` for this service.", ""),
    ignored(Service, "proxy_port", "port", "Nothing reads it; the dev proxy forwards to the leased port."),
    field(Service, "profiles", "list", "the repo's profiles", "Replaces the repo's `profiles` for this service.", ""),
    field(Service, "modes", "map", "", "Named alternative commands, switched in the app without editing the config.", "modes:\n  dev: npm run dev -- --port $PORT\n  prod: npm run start -- -p $PORT"),
    field(Service, "mode", "string", "", "The mode used when none is picked in the app; `cmd` runs when there is none.", "mode: dev"),
    field(Service, "tasks", "list", "", "Quick commands for this service; see Tasks.", ""),
    field(Service, "shortcuts", "list", "", "Older name of `tasks`.", ""),
    removed(Service, "exposes", "Published a `{{var:}}` variable; use a service ref such as `{{<repo>.<service>.url}}`."),
    field(Task, "key", "string", "", "A short name for the task.", "key: migrate"),
    field(Task, "desc", "string", "", "What the task does, shown next to it.", "desc: Run migrations"),
    field(Task, "cmd", "command", "", "The command the task runs, in the repo.", "cmd: bin/rails db:migrate"),
    field(RepoSharedRef, "db_name", "template", "", "The database this repo uses on that shared service, as a name template taking the `branch` tokens.", "db_name: \"{{branch.safe}}_reports\""),
    field(Shared, "type", "string", "the service's name", "The well-known service to fill defaults from, when the name differs: `postgres`, `redis`, `minio`, `opensearch` or `zincsearch`.", "type: postgres"),
    field(Shared, "image", "string", "filled for well-known services", "The Docker image to run. Either this or `cmd`.", "image: postgres:16"),
    field(Shared, "cmd", "command", "", "A command run once for every workspace instead of a container. It gets `$PORT` and `$BIND_IP`; `{{shared.<name>.url}}` is `http://127.0.0.1:<port>`. Either this or `image`.", "cmd: node scripts/mock-as.js"),
    field(Shared, "repo", "repo key", "the project folder", "For a `cmd`: the repo whose main-workspace checkout it runs in. Must be in `repos`.", "repo: api"),
    field(Shared, "port", "port", "leased", "For a `cmd`: the port it is told in `$PORT`. No other shared service may want it.", "port: 4010"),
    field(Shared, "ports", "list", "filled for well-known services", "For an `image`: the container ports to publish (`\"container\"` or `\"host:container\"`); the host side is leased.", "ports: [\"5432\"]"),
    field(Shared, "environment", "map", "filled for well-known services", "Environment for the container or command. In a `cmd` service, `$PORT` and `${PORT}` in a value become its port.", ""),
    field(Shared, "volumes", "list", "filled for well-known services", "For an `image`: Docker volumes.", ""),
    field(Shared, "command", "string", "", "For an `image`: the container's command.", ""),
    field(Shared, "healthcheck", "map", "filled for well-known services", "When the service counts as up; see Shared service healthcheck.", ""),
    field(Shared, "db_user", "string", "postgres for a Postgres", "For an `image`: the login `{{shared.<name>.url}}` and `.user` carry.", ""),
    field(Shared, "db_password", "string", "postgres for a Postgres", "For an `image`: the password `{{shared.<name>.url}}` and `.pass` carry.", ""),
    field(Shared, "capacity", "number", "", "For an `image`: how many workspaces share one instance; each gets a slot, `{{shared.<name>.slot}}` (a Redis database number, for example), and more instances start at base port + instance.", "capacity: 16"),
    field(Shared, "host", "string", "localhost", "The host the app's database and storage browsers connect to. Templates always use `127.0.0.1`.", ""),
    field(HealthCheck, "test", "command or list", "", "A shell command, or a Docker-style `[CMD, ...]` list, that succeeds once the service is up. A `cmd` service waits up to 30 seconds for it before the services that use it start.", "test: curl -sf http://127.0.0.1:$PORT/health"),
    field(HealthCheck, "interval", "duration", "", "For an `image`: time between checks.", "interval: 5s"),
    field(HealthCheck, "timeout", "duration", "", "For an `image`: how long one check may take.", "timeout: 3s"),
    field(HealthCheck, "retries", "number", "", "For an `image`: failed checks before the container counts as unhealthy.", "retries: 10"),
    field(Preset, "preset", "string or list", "", "Presets this one builds on; they apply first.", ""),
    field(Preset, "services", "map", "", "Services the preset adds; same fields as Services.", ""),
    field(Preset, "env", "map", "", "Environment keys the repo left unset.", ""),
    field(Preset, "pre_start", "command", "", "As on a repo.", ""),
    field(Preset, "commands", "map", "", "As on a repo.", ""),
    field(Preset, "setup", "list of commands", "", "As on a repo.", ""),
    field(Preset, "migrate", "list of commands", "", "As on a repo.", ""),
    field(Preset, "seed", "list of commands", "", "As on a repo.", ""),
    field(Preset, "pre_delete", "list of commands", "", "As on a repo.", ""),
    field(Preset, "copy", "list of globs", "", "As on a repo.", ""),
    field(Preset, "seed_from_main", "bool", "false", "As on a repo.", ""),
    field(Preset, "tasks", "list", "", "As on a repo.", ""),
    field(Preset, "shortcuts", "list", "", "Older name of `tasks`.", ""),
    field(Sync, "auto_push", "bool", "false", "Push new commits of the main workspace after a sync.", ""),
    field(Sync, "interval_sec", "number", "", "Seconds between syncs.", ""),
    field(Sync, "refresh_main", "bool", "false", "Keep the main workspace pulled and migrated on a schedule.", "refresh_main: true"),
    field(Sync, "refresh_interval_sec", "number", "", "Seconds between refreshes of the main workspace.", "refresh_interval_sec: 1800"),
    ignored(CodeAgents, "disabled", "bool", "Nothing reads it."),
    ignored(CodeAgents, "only", "list", "Nothing reads it."),
    ignored(CodeAgents, "notify_disabled", "bool", "Nothing reads it."),
    ignored(Ui, "editor", "string", "Nothing reads it; Settings > Editor > External Editor replaced it."),
];

/// The table as Markdown: one section per part of the file, then the keys that no longer do anything.
pub fn markdown() -> String {
    let mut out = String::new();
    for section in Section::ALL {
        let rows: Vec<&FieldDoc> = FIELDS
            .iter()
            .filter(|doc| doc.section == section && doc.status != Status::Removed)
            .collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "## {}\n\n{}\n\n",
            section.title(),
            section.intro()
        ));
        out.push_str("| Key | Type | Default | What it does |\n|---|---|---|---|\n");
        for doc in &rows {
            let what = match doc.status {
                Status::Ignored => format!("Ignored. {}", doc.doc),
                _ => doc.doc.to_string(),
            };
            let default = if doc.default.is_empty() {
                "-".to_string()
            } else if doc.default.contains(' ') {
                doc.default.to_string()
            } else {
                format!("`{}`", doc.default)
            };
            out.push_str(&format!(
                "| `{}{}` | {} | {} | {} |\n",
                section.prefix(),
                doc.key,
                cell(doc.kind),
                cell(&default),
                cell(&what)
            ));
        }
        out.push('\n');
        let examples: Vec<&str> = rows
            .iter()
            .map(|doc| doc.example)
            .filter(|example| !example.is_empty())
            .collect();
        if !examples.is_empty() {
            let under = match section.prefix().trim_end_matches('.') {
                "" => String::new(),
                path => format!(", each written under `{path}`"),
            };
            out.push_str(&format!(
                "Examples{under}:\n\n```yaml\n{}\n```\n\n",
                examples.join("\n\n")
            ));
        }
    }
    out.push_str("## Removed keys\n\nThese keys do nothing any more. `config_normalize` (or `pom config normalize`) deletes the ones it can.\n\n| Key | Instead |\n|---|---|\n");
    for doc in FIELDS.iter().filter(|doc| doc.status == Status::Removed) {
        out.push_str(&format!(
            "| `{}{}` | {} |\n",
            doc.section.prefix(),
            doc.key,
            cell(doc.doc)
        ));
    }
    out
}

/// A pipe ends a Markdown table cell even inside a code span.
pub fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

#[cfg(test)]
pub(crate) mod seen {
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    use super::Section;

    thread_local! {
        static SEEN: RefCell<BTreeSet<(Section, &'static str)>> = const { RefCell::new(BTreeSet::new()) };
    }

    pub(crate) fn record(section: Section, key: &'static str) {
        SEEN.with(|seen| seen.borrow_mut().insert((section, key)));
    }

    pub(crate) fn take() -> BTreeSet<(Section, &'static str)> {
        SEEN.with(|seen| std::mem::take(&mut *seen.borrow_mut()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// One of everything, so the parser looks up every key it knows.
    const EVERYTHING: &str = r#"
session: myproject
sync: { auto_push: false }
code_agents: { disabled: false }
ui: { editor: code }
repos:
  api:
    lifecycle: { tasks: [{ key: k, desc: d, cmd: c }] }
    shared_services: [{ postgres: { db_name: x } }]
    services:
      server: { cmd: run }
presets:
  base: { env: { A: b } }
shared_services:
  postgres: { image: postgres:16, healthcheck: { test: "true" } }
"#;

    #[test]
    fn every_key_the_parser_reads_is_documented_and_every_documented_key_is_read() {
        seen::take();
        let root = crate::yaml_node::parse(EVERYTHING)
            .expect("parses")
            .expect("a document");
        let mut decoder = crate::decode::Decoder::default();
        crate::Config::decode(&root, &mut decoder);
        assert!(decoder.errors.is_empty(), "{:?}", decoder.errors);
        let read = seen::take();
        let documented: BTreeSet<(Section, &str)> = FIELDS
            .iter()
            .filter(|doc| doc.status != Status::Removed)
            .map(|doc| (doc.section, doc.key))
            .collect();
        let undocumented: Vec<_> = read.difference(&documented).collect();
        assert!(
            undocumented.is_empty(),
            "add these to FIELDS: {undocumented:?}"
        );
        let stale: Vec<_> = documented.difference(&read).collect();
        assert!(
            stale.is_empty(),
            "the parser no longer reads these: {stale:?}"
        );
    }

    #[test]
    fn keys_are_documented_once() {
        let mut keys = BTreeSet::new();
        for doc in FIELDS {
            assert!(
                keys.insert((doc.section, doc.key)),
                "{:?} {}",
                doc.section,
                doc.key
            );
            assert!(doc.doc.is_ascii() && doc.example.is_ascii(), "{}", doc.key);
        }
    }

    #[test]
    fn the_markdown_lists_every_current_key() {
        let text = markdown();
        for doc in FIELDS.iter().filter(|doc| doc.status == Status::Current) {
            assert!(
                text.contains(&format!("`{}{}`", doc.section.prefix(), doc.key)),
                "{}",
                doc.key
            );
        }
    }
}
