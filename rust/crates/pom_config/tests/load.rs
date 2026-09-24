use std::path::{Path, PathBuf};

use pom_config::{Config, LoadError};

struct Project {
    dir: tempfile::TempDir,
}

impl Project {
    fn new(root: &str) -> Project {
        let project = Project {
            dir: tempfile::tempdir().expect("temp dir"),
        };
        project.write("pom.yml", root);
        project
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.dir.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create dirs");
        }
        std::fs::write(path, contents).expect("write file");
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("pom.yml")
    }

    fn try_load(&self) -> Result<Config, LoadError> {
        Config::load(&self.root())
    }

    fn load(&self) -> Config {
        match self.try_load() {
            Ok(config) => config,
            Err(error) => panic!("load failed: {error}"),
        }
    }
}

fn load(yaml: &str) -> Config {
    Project::new(yaml).load()
}

fn keys<V>(map: &indexmap::IndexMap<String, V>) -> Vec<&str> {
    map.keys().map(String::as_str).collect()
}

#[test]
fn loads_full_schema_in_source_order() {
    let config = load(
        r#"
session: test-project
default_branch: main
repos:
  api:
    alias: api
    shell_env: .env
    services:
      server:
        cmd: "go run ./cmd/server"
      worker:
        cmd: "go run ./cmd/worker"
  client:
    alias: web
    services:
      dev:
        cmd: "npm run dev"
combinations:
  full:
    - api/server
    - api/worker
    - web/dev
shared_services:
  postgres:
    image: postgres:16
    host: 127.0.0.1
    ports:
      - "5432:5432"
    db_user: admin
    db_password: secret
"#,
    );
    assert_eq!(config.session, "test-project");
    assert_eq!(config.global_default_branch(), "main");
    assert_eq!(keys(&config.repos), ["api", "client"]);
    assert_eq!(config.repos["api"].alias, "api");
    assert_eq!(keys(&config.repos["api"].services), ["server", "worker"]);
    assert_eq!(config.combinations["full"].len(), 3);
    let postgres = &config.shared_services["postgres"];
    assert_eq!(
        (postgres.db_user.as_str(), postgres.db_password.as_str()),
        ("admin", "secret")
    );
    assert_eq!(postgres.ports, ["5432:5432"]);
}

#[test]
fn empty_file_gets_default_session() {
    let config = load("");
    assert_eq!(config.session, "pomelo");
    assert!(config.repos.is_empty());
}

#[test]
fn worktree_fields_env_files_and_shared_refs() {
    let config = load(
        r#"
session: test
repos:
  api:
    services:
      server:
        cmd: "go run ."
    copy:
      - .env.local
    databases:
      main: mydb
    env:
      "*":
        DB_BASE: shared
      .env: {}
      .env.local:
        DB_HOST: localhost
    shared_services:
      - postgres
      - redis:
          db_name: cache_db
    setup:
      - npm install
presets:
  node:
    setup:
      - yarn install
"#,
    );
    let api = &config.repos["api"];
    assert!(api.has_worktree_config());
    assert_eq!(api.copy, [".env.local"]);
    assert_eq!(api.databases["main"], "mydb");
    assert_eq!(api.env["DB_BASE"], "shared");
    let files: Vec<&str> = api.env_output.iter().map(|e| e.file.as_str()).collect();
    assert_eq!(files, [".env", ".env.local"]);
    assert_eq!(api.env_output[1].env["DB_HOST"], "localhost");
    assert_eq!(api.shared_refs.len(), 2);
    assert_eq!(api.shared_refs[0].name, "postgres");
    assert_eq!(api.shared_refs[0].db_name, "");
    assert_eq!(api.shared_refs[1].name, "redis");
    assert_eq!(api.shared_refs[1].db_name, "cache_db");
    assert_eq!(api.setup, ["npm install"]);
}

#[test]
fn flat_env_writes_default_file() {
    let config = load("repos:\n  api:\n    env:\n      A: '1'\n      B: 2\n");
    let api = &config.repos["api"];
    assert_eq!(api.env["B"], "2");
    let entries = api.env_file_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].file, ".env.local");
}

#[test]
fn profiles_and_raw_templates() {
    let config = load(
        r#"
session: demo
environments:
  staging:
    backend.api: https://api.staging.example
repos:
  backend:
    alias: api
    databases:
      main: "{{branch.safe}}"
    services:
      api:
        cmd: "go run ."
        port: true
  web:
    profiles: [local, staging]
    services:
      app:
        cmd: "vite"
        port: true
        dir: app
        env:
          VITE_API: "{{backend.api.url}}"
          VITE_WS:  "{{backend.api.ws}}"
"#,
    );
    assert_eq!(config.repos["backend"].databases["main"], "{{branch.safe}}");
    let web = &config.repos["web"];
    assert_eq!(
        web.env_profiles(web.services.get("app")),
        ["local", "staging"]
    );
    let backend = &config.repos["backend"];
    assert_eq!(backend.env_profiles(backend.services.get("api")), ["local"]);
    assert!(web.services["app"].has_port());
    assert!(config.validate().is_ok());
}

#[test]
fn scalar_service_is_its_command_and_profile_scalar_is_a_list() {
    let config =
        load("repos:\n  api:\n    profiles: staging\n    services:\n      dev: npm run dev\n");
    let api = &config.repos["api"];
    assert_eq!(api.services["dev"].cmd, "npm run dev");
    assert_eq!(api.profiles, ["staging"]);
}

#[test]
fn null_repo_and_service_bodies_are_empty_defaults() {
    let config = load("repos:\n  api:\n  web:\n    services:\n      dev:\n");
    assert_eq!(keys(&config.repos), ["api", "web"]);
    assert_eq!(config.repos["web"].services["dev"].cmd, "");
}

#[test]
fn validate_rejects_colon_templates_and_unknown_profiles() {
    let cases = [
        "repos:\n  r:\n    services:\n      s: { cmd: x, env: { A: '{{var:NOPE}}' } }\n",
        "repos:\n  r:\n    databases: { main: '{{branch.safe}}' }\n    services:\n      s: { cmd: x, env: { A: '{{db:other}}' } }\n",
        "repos:\n  r:\n    services:\n      s: { cmd: x, env: { A: '{{conn:postgres}}' } }\n",
        "repos:\n  r:\n    services:\n      s: { cmd: x, env: { A: '{{url:r}}' } }\n",
        "repos:\n  r:\n    profiles: [local, ghost]\n    services:\n      s: { cmd: x }\n",
    ];
    for yaml in cases {
        assert!(
            load(yaml).validate().is_err(),
            "expected validation error for {yaml}"
        );
    }
}

#[test]
fn validate_reports_every_problem_sorted() {
    let config = load(
        "repos:\n  r:\n    profiles: [ghost]\n    env:\n      A: '{{db:x}}'\n      B: '{{conn:pg}}'\n",
    );
    let message = config.validate().err().unwrap_or_default();
    let lines: Vec<&str> = message.lines().skip(1).collect();
    assert_eq!(lines.len(), 3, "{message}");
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted);
}

#[test]
fn type_mismatches_are_all_reported_with_lines() {
    let error = Project::new("session: [a]\nrepos:\n  api:\n    setup: npm i\n")
        .try_load()
        .err()
        .map(|e| e.message)
        .unwrap_or_default();
    assert!(
        error.contains("line 1: cannot unmarshal !!seq into string"),
        "{error}"
    );
    assert!(
        error.contains("line 4: cannot unmarshal !!str `npm i` into []string"),
        "{error}"
    );
}

#[test]
fn syntax_error_names_the_file() {
    let error = Project::new("repos: [\n").try_load().err();
    assert!(error.is_some_and(|e| e.path.ends_with("pom.yml") && e.message.contains("parse")));
}

#[test]
fn fragments_merge_in_order() {
    let project = Project::new(
        "session: demo\ndefault_branch: main\nrepos:\n  api:\n    alias: api\n    services:\n      s:\n        cmd: go run .\n",
    );
    project.write(
        "pom.d/10-web.yml",
        "repos:\n  web:\n    alias: web\n    services:\n      dev:\n        cmd: vite\n",
    );
    project.write(
        "pom.d/20-shared.yml",
        "shared_services:\n  postgres:\n    image: postgres:16\n",
    );
    let config = project.load();
    assert_eq!(keys(&config.repos), ["api", "web"]);
    assert_eq!(config.shared_services["postgres"].image, "postgres:16");
}

#[test]
fn later_fragment_overrides_leaf_and_merges_maps() {
    let project = Project::new("repos:\n  api:\n    setup: [a]\n    commands:\n      test: t\n");
    project.write(
        "pom.d/01.yml",
        "repos:\n  api:\n    setup: [b]\n    commands:\n      lint: l\n",
    );
    let api = &project.load().repos["api"];
    assert_eq!(api.setup, ["b"]);
    assert_eq!(keys(&api.commands), ["test", "lint"]);
}

#[test]
fn fragment_syntax_error_names_the_fragment() {
    let project = Project::new("session: x\n");
    project.write("pom.d/bad.yml", "a: [\n");
    let error = project.try_load().err();
    assert!(error.is_some_and(|e| e.path.ends_with(Path::new("pom.d/bad.yml"))));
}

#[test]
fn well_known_shared_defaults() {
    let config = load(
        "repos:\n  api:\n    services:\n      s:\n        cmd: x\nshared_services:\n  postgres:\n  redis:\n    capacity: 8\n  cache:\n    type: redis\n",
    );
    let postgres = &config.shared_services["postgres"];
    assert_eq!(postgres.image, "postgres:16");
    assert_eq!(postgres.ports, ["5432"]);
    assert_eq!(postgres.environment["POSTGRES_USER"], "postgres");
    assert!(postgres.healthcheck.is_some());
    assert_eq!(postgres.db_user, "postgres");
    let redis = &config.shared_services["redis"];
    assert_eq!(redis.image, "redis:7-alpine");
    assert_eq!(redis.capacity, Some(8));
    let cache = &config.shared_services["cache"];
    assert_eq!(cache.image, "redis:7-alpine");
    assert_eq!(cache.command, "redis-server --appendonly yes");
}

#[test]
fn user_environment_overrides_template_environment() {
    let config =
        load("shared_services:\n  postgres:\n    environment:\n      POSTGRES_PASSWORD: other\n");
    let environment = &config.shared_services["postgres"].environment;
    assert_eq!(environment["POSTGRES_PASSWORD"], "other");
    assert_eq!(environment["POSTGRES_USER"], "postgres");
}

#[test]
fn commands_derive_setup_migrate_and_shortcuts() {
    let config = load(
        "repos:\n  api:\n    commands:\n      install: npm ci\n      generate: npm run gen\n      migrate: npm run migrate\n      test: npm test\n",
    );
    let api = &config.repos["api"];
    assert_eq!(
        api.effective_setup(),
        ["npm ci", "npm run gen", "npm run migrate"]
    );
    assert_eq!(api.effective_migrate(), ["npm run migrate"]);
    let commands: Vec<String> = api
        .effective_shortcuts()
        .into_iter()
        .map(|s| s.cmd)
        .collect();
    assert_eq!(
        commands,
        ["npm ci", "npm run gen", "npm run migrate", "npm test"]
    );
}

#[test]
fn custom_commands_follow_known_ones_sorted() {
    let config = load(
        "repos:\n  api:\n    shortcuts:\n      - { cmd: npm test, desc: tests }\n    commands:\n      zeta: z\n      test: npm test\n      alpha: a\n      build: b\n",
    );
    let shortcuts = config.repos["api"].effective_shortcuts();
    let labels: Vec<(&str, &str)> = shortcuts
        .iter()
        .map(|s| (s.cmd.as_str(), s.desc.as_str()))
        .collect();
    assert_eq!(
        labels,
        [
            ("npm test", "tests"),
            ("b", "Build"),
            ("a", "Alpha"),
            ("z", "Zeta")
        ]
    );
}

#[test]
fn explicit_setup_and_migrate_win_over_commands() {
    let config = load(
        "repos:\n  api:\n    setup: [explicit setup]\n    migrate: [explicit migrate]\n    commands:\n      install: npm ci\n      migrate: npm run migrate\n",
    );
    let api = &config.repos["api"];
    assert_eq!(api.effective_setup(), ["explicit setup"]);
    assert_eq!(api.effective_migrate(), ["explicit migrate"]);
}

#[test]
fn composed_presets_fill_defaults() {
    let config = load(concat!(
        "session: t\n",
        "presets:\n",
        "  infra:\n    env:\n      REDIS_URL: redis://x\n",
        "  pg:\n    env:\n      DATABASE_URL: postgres://y\n",
        "  nest:\n    preset: [infra, pg]\n    seed_from_main: true\n    pre_start: source .env.local\n",
        "    commands:\n      install: npm ci\n",
        "    services:\n      worker: node worker\n",
        "repos:\n  ai:\n    preset: nest\n    services:\n      api:\n        port: true\n        cmd: node main\n",
    ));
    let ai = &config.repos["ai"];
    assert_eq!(ai.env["REDIS_URL"], "redis://x");
    assert_eq!(ai.env["DATABASE_URL"], "postgres://y");
    assert!(ai.own_env.is_empty());
    assert!(ai.seed_from_main);
    assert_eq!(ai.pre_start, "source .env.local");
    assert_eq!(ai.commands["install"], "npm ci");
    assert_eq!(keys(&ai.services), ["api", "worker"]);
}

#[test]
fn repo_lifecycle_wins_over_composed_presets() {
    let config = load(concat!(
        "presets:\n",
        "  base:\n    pre_start: base start\n",
        "  nest:\n    preset: [base]\n    pre_start: nest start\n",
        "repos:\n  a:\n    preset: nest\n    lifecycle:\n      pre_start: repo start\n",
    ));
    assert_eq!(config.repos["a"].pre_start, "repo start");
}

#[test]
fn preset_cycle_terminates() {
    let config = load(
        "presets:\n  a:\n    preset: b\n    setup: [x]\n  b:\n    preset: a\nrepos:\n  r:\n    preset: a\n",
    );
    assert_eq!(config.repos["r"].setup, ["x"]);
}

#[test]
fn commands_from_preset() {
    let config = load(
        "presets:\n  node:\n    commands:\n      install: npm ci\n      migrate: npm run migrate\nrepos:\n  api:\n    preset: [node]\n",
    );
    let api = &config.repos["api"];
    assert_eq!(api.effective_migrate(), ["npm run migrate"]);
    assert_eq!(api.commands["install"], "npm ci");
}

#[test]
fn lifecycle_block_equals_flat_and_wins() {
    let flat = load("repos:\n  api:\n    setup: [bundle install]\n    seed: [rake seed]\n    pre_start: source .env\n");
    let block = load(
        "repos:\n  api:\n    lifecycle:\n      setup: [bundle install]\n      seed: [rake seed]\n      pre_start: source .env\n",
    );
    assert_eq!(flat.repos["api"], block.repos["api"]);
    let both = load("repos:\n  api:\n    setup: [old]\n    lifecycle:\n      setup: [new]\n");
    assert_eq!(both.repos["api"].setup, ["new"]);
}

#[test]
fn workspace_presets_become_workspace_services() {
    let config =
        load("preset: [tools]\npresets:\n  tools:\n    services:\n      docs: mkdocs serve\n");
    assert_eq!(keys(&config.workspace_services), ["docs"]);
}

#[test]
fn yaml_anchors_share_service_blocks() {
    let config = load(
        "x-node: &node\n  type: frontend\n  cmd: npm start\nrepos:\n  web:\n    services:\n      a: *node\n      b:\n        <<: *node\n        cmd: npm run b\n",
    );
    let services = &config.repos["web"].services;
    assert_eq!(services["a"].cmd, "npm start");
    assert_eq!(services["b"].cmd, "npm run b");
    assert!(services["b"].has_port());
}

#[test]
fn finds_config_walking_up() {
    let project = Project::new("session: x\n");
    let nested = project.dir.path().join("a/b");
    std::fs::create_dir_all(&nested).expect("dirs");
    assert_eq!(pom_config::find_config_from(&nested), Some(project.root()));
}
