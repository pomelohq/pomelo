//! The previous core's detection cases, over small repos written to a temp dir.

use pom_detect::{
    detect, detect_repo, emit, parse_compose, RepoDetection, ResolvedRun, ServiceKind, StackFacts,
};

fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp");
    for (name, content) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(path, content).expect("write");
    }
    dir
}

fn detected(files: &[(&str, &str)]) -> StackFacts {
    let dir = repo(files);
    detect(dir.path()).expect("a detected stack")
}

fn runs(facts: &StackFacts, needle: &str) -> bool {
    facts.run.iter().any(|run| run.cmd.contains(needle))
}

fn worker(facts: &StackFacts) -> bool {
    facts.run.iter().any(|run| run.kind == "worker")
}

fn by_dir<'a>(facts: &'a [StackFacts], dir: &str) -> Option<&'a StackFacts> {
    facts.iter().find(|facts| facts.dir == dir)
}

#[test]
fn python_stacks() {
    let django = detected(&[("manage.py", ""), ("requirements.txt", "django\ncelery\n")]);
    assert_eq!(
        (django.language.as_str(), django.framework.as_str()),
        ("python", "django")
    );
    assert_eq!(
        (django.package_manager.as_str(), django.port),
        ("pip", 8000)
    );
    assert!(runs(&django, "runserver") && worker(&django));
    let fastapi = detected(&[
        ("app.py", ""),
        ("requirements.txt", "fastapi\nuvicorn\n"),
        ("uv.lock", ""),
    ]);
    assert_eq!(
        (fastapi.framework.as_str(), fastapi.package_manager.as_str()),
        ("fastapi", "uv")
    );
    assert!(runs(&fastapi, "uvicorn") && fastapi.install.contains("uv "));
    let flask = detected(&[("app.py", ""), ("requirements.txt", "Flask\n")]);
    assert_eq!((flask.framework.as_str(), flask.port), ("flask", 5000));
    let base = detected(&[("requirements.txt", "requests\n")]);
    assert_eq!(
        (
            base.language.as_str(),
            base.framework.as_str(),
            base.package_manager.as_str()
        ),
        ("python", "", "pip")
    );
}

#[test]
fn js_stacks() {
    let next = detected(&[
        ("package.json", r#"{"dependencies":{"next":"14"}}"#),
        ("next.config.js", ""),
        ("package-lock.json", ""),
    ]);
    assert_eq!(
        (
            next.framework.as_str(),
            next.package_manager.as_str(),
            next.port
        ),
        ("next", "npm", 3000)
    );
    assert!(runs(&next, "npm run dev"));
    let pnpm = detected(&[
        ("package.json", r#"{"dependencies":{"next":"14"}}"#),
        ("next.config.js", ""),
        ("pnpm-lock.yaml", ""),
    ]);
    assert!(pnpm.package_manager == "pnpm" && runs(&pnpm, "pnpm run dev"));
    let vite = detected(&[("package.json", "{}"), ("vite.config.ts", "")]);
    assert_eq!((vite.framework.as_str(), vite.port), ("vite", 5173));
    assert_eq!(
        detected(&[("package.json", "{}"), ("vite.config.mts", "")]).framework,
        "vite"
    );
    assert_eq!(
        detected(&[("package.json", "{}"), ("nest-cli.json", "")]).framework,
        "nest"
    );
    let angular = detected(&[
        ("package.json", r#"{"dependencies":{"@angular/core":"18"}}"#),
        ("angular.json", "{}"),
    ]);
    assert_eq!(
        (angular.framework.as_str(), angular.port),
        ("angular", 4200)
    );
    assert_eq!(
        detected(&[("package.json", r#"{"dependencies":{"express":"4"}}"#)]).framework,
        "express"
    );
    assert_eq!(
        detected(&[(
            "package.json",
            r#"{"dependencies":{"@remix-run/react":"2"}}"#
        )])
        .framework,
        "remix"
    );
}

#[test]
fn java_go_and_ruby_stacks() {
    let spring = detected(&[("pom.xml", "<project>spring-boot-starter-web</project>")]);
    assert_eq!(
        (
            spring.framework.as_str(),
            spring.package_manager.as_str(),
            spring.port
        ),
        ("spring-boot", "maven", 8080)
    );
    assert!(runs(&spring, "spring-boot:run"));
    let gradle = detected(&[("build.gradle", "org.springframework.boot")]);
    assert!(gradle.package_manager == "gradle" && runs(&gradle, "bootRun"));
    let quarkus = detected(&[("pom.xml", "<project>quarkus-maven-plugin</project>")]);
    assert!(quarkus.framework == "quarkus" && runs(&quarkus, "quarkus:dev"));
    let plain = detected(&[("pom.xml", "<project></project>")]);
    assert_eq!(
        (plain.framework.as_str(), plain.package_manager.as_str()),
        ("", "maven")
    );

    let gin = detected(&[(
        "go.mod",
        "module x\n\nrequire github.com/gin-gonic/gin v1.9.1\n",
    )]);
    assert!(gin.framework == "gin" && gin.port == 8080 && runs(&gin, "go run"));
    assert_eq!(
        detected(&[(
            "go.mod",
            "module x\n\nrequire github.com/labstack/echo/v4 v4.11.0\n"
        )])
        .port,
        1323
    );
    let go = detected(&[("go.mod", "module x\n")]);
    assert!(go.framework.is_empty() && go.install.contains("go mod download"));

    let rails = detected(&[
        ("Gemfile", "gem 'rails'\ngem 'sidekiq'\n"),
        ("bin/rails", ""),
    ]);
    assert_eq!((rails.framework.as_str(), rails.port), ("rails", 3000));
    assert!(runs(&rails, "rails server") && worker(&rails));
    assert!(rails.setup.join(" ").contains("db:migrate"));
    assert_eq!(
        detected(&[("Gemfile", "gem 'sinatra'\n")]).framework,
        "sinatra"
    );
    let ruby = detected(&[("Gemfile", "gem 'rake'\n")]);
    assert_eq!(
        (ruby.framework.as_str(), ruby.package_manager.as_str()),
        ("", "bundler")
    );

    assert!(detect(repo(&[("README.md", "hi")]).path()).is_none());
}

#[test]
fn monorepos() {
    let turbo = repo(&[
        ("turbo.json", "{}"),
        ("pnpm-workspace.yaml", "packages:\n  - 'apps/*'\n"),
        ("package.json", "{}"),
        ("apps/web/package.json", r#"{"dependencies":{"next":"14"}}"#),
        ("apps/web/next.config.js", ""),
        (
            "apps/api/package.json",
            r#"{"dependencies":{"express":"4"}}"#,
        ),
    ]);
    let facts = detect_repo(turbo.path());
    assert_eq!(facts.len(), 2, "{facts:?}");
    assert_eq!(
        by_dir(&facts, "apps/web").map(|facts| facts.framework.as_str()),
        Some("next")
    );
    assert_eq!(
        by_dir(&facts, "apps/api").map(|facts| facts.framework.as_str()),
        Some("express")
    );

    let go_work = repo(&[
        ("go.work", "go 1.22\n\nuse ./svc-a\nuse ./svc-b\n"),
        (
            "svc-a/go.mod",
            "module a\n\nrequire github.com/gin-gonic/gin v1.9.1\n",
        ),
        (
            "svc-b/go.mod",
            "module b\n\nrequire github.com/labstack/echo/v4 v4.11.0\n",
        ),
    ]);
    let facts = detect_repo(go_work.path());
    assert_eq!(
        by_dir(&facts, "svc-a").map(|facts| facts.framework.as_str()),
        Some("gin")
    );
    assert_eq!(
        by_dir(&facts, "svc-b").map(|facts| facts.framework.as_str()),
        Some("echo")
    );

    let polyglot = repo(&[
        ("package.json", r#"{"workspaces":["frontend"]}"#),
        ("frontend/package.json", "{}"),
        ("frontend/vite.config.ts", ""),
        (
            "backend/pyproject.toml",
            "[project]\ndependencies = [\"fastapi\"]\n",
        ),
        ("backend/app/main.py", ""),
    ]);
    let facts = detect_repo(polyglot.path());
    assert_eq!(
        by_dir(&facts, "frontend").map(|facts| facts.framework.as_str()),
        Some("vite")
    );
    assert_eq!(
        by_dir(&facts, "backend").map(|facts| facts.framework.as_str()),
        Some("fastapi")
    );

    let rails_plus = repo(&[
        ("Gemfile", "gem 'rails'\ngem 'sidekiq'\n"),
        ("bin/rails", ""),
        ("package.json", r#"{"workspaces":["streaming"]}"#),
        (
            "streaming/package.json",
            r#"{"dependencies":{"express":"4"}}"#,
        ),
    ]);
    let facts = detect_repo(rails_plus.path());
    assert_eq!(
        by_dir(&facts, "").map(|facts| facts.framework.as_str()),
        Some("rails")
    );
    assert_eq!(
        by_dir(&facts, "streaming").map(|facts| facts.framework.as_str()),
        Some("express")
    );

    let lock_at_root = repo(&[
        ("package.json", r#"{"workspaces":["frontend"]}"#),
        ("bun.lock", ""),
        ("frontend/package.json", r#"{"dependencies":{"next":"14"}}"#),
        ("frontend/next.config.js", ""),
    ]);
    assert_eq!(
        by_dir(&detect_repo(lock_at_root.path()), "frontend")
            .map(|facts| facts.package_manager.as_str()),
        Some("bun")
    );

    let maven = repo(&[
        (
            "pom.xml",
            "<project><modules><module>svc-a</module><module>svc-b</module></modules></project>",
        ),
        ("svc-a/pom.xml", "<project>quarkus-maven-plugin</project>"),
        (
            "svc-b/pom.xml",
            "<project>spring-boot-starter-web</project>",
        ),
    ]);
    let facts = detect_repo(maven.path());
    assert_eq!(
        by_dir(&facts, "svc-a").map(|facts| facts.framework.as_str()),
        Some("quarkus")
    );
    assert_eq!(
        by_dir(&facts, "svc-b").map(|facts| facts.framework.as_str()),
        Some("spring-boot")
    );

    let libraries = repo(&[
        ("package.json", r#"{"workspaces":["packages/*"]}"#),
        (
            "packages/app/package.json",
            r#"{"dependencies":{"next":"14"}}"#,
        ),
        ("packages/app/next.config.js", ""),
        ("packages/ui/package.json", r#"{"name":"ui"}"#),
    ]);
    let facts = detect_repo(libraries.path());
    assert!(by_dir(&facts, "packages/ui").is_none());
    assert_eq!(
        by_dir(&facts, "packages/app").map(|facts| facts.framework.as_str()),
        Some("next")
    );

    let single = repo(&[
        ("package.json", r#"{"dependencies":{"next":"14"}}"#),
        ("next.config.js", ""),
    ]);
    let facts = detect_repo(single.path());
    assert!(facts.len() == 1 && facts[0].framework == "next" && facts[0].dir.is_empty());
}

#[test]
fn nx_workspaces() {
    let angular = repo(&[
        ("nx.json", "{}"),
        ("package.json", r#"{"dependencies":{"@angular/core":"21"}}"#),
        ("yarn.lock", ""),
        (
            "apps/cart/project.json",
            r#"{"name":"cart","projectType":"application"}"#,
        ),
        ("apps/cart/package.json", r#"{"name":"cart"}"#),
        (
            "apps/cart-e2e/project.json",
            r#"{"name":"cart-e2e","projectType":"application","tags":["type:e2e"]}"#,
        ),
        (
            "libs/shared/ui/project.json",
            r#"{"name":"shared-ui","projectType":"library"}"#,
        ),
    ]);
    let facts = detect_repo(angular.path());
    let cart = by_dir(&facts, "apps/cart").expect("cart");
    assert_eq!(cart.framework, "angular");
    assert_eq!(cart.run[0].cmd, "yarn nx serve cart");
    assert!(
        by_dir(&facts, "apps/cart-e2e").is_none() && by_dir(&facts, "libs/shared/ui").is_none()
    );

    let root_app = repo(&[
        ("nx.json", "{}"),
        ("package.json", r#"{"dependencies":{"express":"4"}}"#),
        (
            "project.json",
            r#"{"name":"api","projectType":"application","targets":{"serve":{}}}"#,
        ),
        (
            "e2e/project.json",
            r#"{"name":"e2e","projectType":"application","targets":{"e2e":{}}}"#,
        ),
        ("e2e/package.json", r#"{"name":"e2e"}"#),
    ]);
    let facts = detect_repo(root_app.path());
    let api = by_dir(&facts, "").expect("root app");
    assert_eq!(
        (api.framework.as_str(), api.run[0].cmd.as_str()),
        ("express", "npx nx serve api")
    );
    assert!(by_dir(&facts, "e2e").is_none());
}

#[test]
fn compose_services_are_classified() {
    let dir = repo(&[(
        "compose.yml",
        "services:\n  db:\n    image: postgres:16-alpine\n  cache:\n    image: redis:7\n  search:\n    image: docker.elastic.co/elasticsearch/elasticsearch:8.13.0\n  storage:\n    image: minio/minio\n  queue:\n    image: rabbitmq:3-management\n  mail:\n    image: axllent/mailpit\n  web:\n    build: .\n  proxy:\n    image: nginx:alpine\n  weird:\n    image: acme/some-internal-thing:1.2\n  api:\n    image: myapp-api:latest\n  zk:\n    image: zookeeper:3.9\n  base:\n    image: mysql:8.4\n  replica:\n    extends:\n      service: base\n",
    )]);
    let services = parse_compose(dir.path());
    let get = |name: &str| {
        services
            .iter()
            .find(|service| service.name == name)
            .expect(name)
    };
    let shared = |name: &str, kind: &str, strategy: &str| {
        let service = get(name);
        assert_eq!(
            (
                service.kind,
                service.kind_name.as_str(),
                service.strategy.as_str()
            ),
            (ServiceKind::Shared, kind, strategy),
            "{name}"
        );
    };
    shared("db", "postgres", "database-per-branch");
    shared("cache", "redis", "dbindex-slot");
    shared("search", "elasticsearch", "namespace-prefix");
    shared("storage", "minio", "bucket-per-branch");
    shared("queue", "rabbitmq", "vhost-per-branch");
    shared("mail", "mail", "shared-stateless");
    shared("weird", "custom", "");
    shared("zk", "custom", "");
    assert_eq!(get("web").kind, ServiceKind::App);
    assert_eq!(get("api").kind, ServiceKind::App);
    assert_eq!(get("proxy").kind, ServiceKind::Proxy);
    assert_eq!(get("db").clone, "template");
    assert_eq!(
        (
            get("replica").kind_name.as_str(),
            get("replica").clone.as_str()
        ),
        ("mysql", "dump-restore")
    );
    assert!(parse_compose(repo(&[("README.md", "hi")]).path()).is_empty());

    let files = repo(&[
        ("base.yml", "services:\n  pg:\n    image: postgres:16\n"),
        (
            "compose.yml",
            "services:\n  db:\n    extends:\n      file: base.yml\n      service: pg\n",
        ),
    ]);
    assert_eq!(parse_compose(files.path())[0].kind_name, "postgres");
}

fn load(yaml: &str) -> pom_config::Config {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("pom.yml");
    std::fs::write(&path, yaml).expect("write");
    let config = pom_config::Config::load(&path)
        .unwrap_or_else(|error| panic!("load:\n{yaml}\n{}", error.message));
    if let Err(error) = config.validate() {
        panic!("validate:\n{yaml}\n{error}");
    }
    config
}

#[test]
fn the_draft_config_loads_and_validates() {
    let services = parse_compose(
        repo(&[("compose.yml", "services:\n  db:\n    image: postgres:16\n  cache:\n    image: redis:7\n  web:\n    build: .\n")])
            .path(),
    );
    let yaml = emit(
        "proj",
        &[
            RepoDetection {
                name: "api".into(),
                alias: String::new(),
                apps: vec![StackFacts {
                    language: "go".into(),
                    framework: "gin".into(),
                    install: "go mod download".into(),
                    port: 8080,
                    run: vec![ResolvedRun {
                        kind: "server".into(),
                        cmd: "go run .".into(),
                    }],
                    ..StackFacts::default()
                }],
                shared: services,
            },
            RepoDetection {
                name: "frontend".into(),
                alias: String::new(),
                apps: vec![StackFacts {
                    dir: "apps/web".into(),
                    language: "js".into(),
                    framework: "next".into(),
                    install: "npm install".into(),
                    port: 3000,
                    run: vec![ResolvedRun {
                        kind: "server".into(),
                        cmd: "npm run dev".into(),
                    }],
                    ..StackFacts::default()
                }],
                shared: Vec::new(),
            },
            RepoDetection {
                name: "app".into(),
                alias: String::new(),
                apps: vec![StackFacts {
                    language: "ruby".into(),
                    framework: "rails".into(),
                    install: "bundle install".into(),
                    port: 3000,
                    setup: vec!["bin/rails db:migrate".into()],
                    run: vec![
                        ResolvedRun {
                            kind: "server".into(),
                            cmd: "bin/rails server -p 3000".into(),
                        },
                        ResolvedRun {
                            kind: "worker".into(),
                            cmd: "bundle exec sidekiq".into(),
                        },
                    ],
                    ..StackFacts::default()
                }],
                shared: Vec::new(),
            },
        ],
    );
    let config = load(&yaml);
    assert_eq!(config.repos.len(), 3, "{yaml}");
    let api = &config.repos["api"];
    assert_eq!(api.services["gin"].cmd, "go run .");
    assert_eq!(config.repos["frontend"].services["web"].dir, "apps/web");
    assert!(config.repos["app"].services.contains_key("rails-worker"));
    assert!(config.repos["app"].setup.join(" ").contains("db:migrate"));
    assert!(
        config.shared_services.contains_key("postgres")
            && config.shared_services.contains_key("redis")
    );
    assert!(!yaml.contains("custom"));
}

/// Detection over every repo under `POM_DETECT_REAL_ROOT` (opt-in; skipped without it): prints one line per
/// repo in the previous core's format, and checks each repo's draft config loads and validates.
#[test]
fn real_corpus() {
    let Some(root) = std::env::var_os("POM_DETECT_REAL_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .expect("corpus")
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        let repo = root.join(&name);
        let facts = detect_repo(&repo);
        if facts.is_empty() {
            println!("{name:<28} NO MATCH");
            continue;
        }
        let parts: Vec<String> = facts
            .iter()
            .map(|facts| {
                let dir = if facts.dir.is_empty() {
                    "."
                } else {
                    &facts.dir
                };
                format!(
                    "{dir}:{}/{}/{}",
                    facts.language, facts.framework, facts.package_manager
                )
            })
            .collect();
        println!("{name:<28} {}", parts.join("  "));
        let key: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        load(&emit(
            "corpus",
            &[RepoDetection {
                name: key,
                alias: String::new(),
                apps: facts,
                shared: parse_compose(&repo),
            }],
        ));
    }
}
