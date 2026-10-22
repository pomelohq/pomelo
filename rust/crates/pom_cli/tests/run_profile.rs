//! `pom run` under the test profile, through the real `pom` binary: the repo's `.env.test` replaces dev
//! values, while connection values keep the workspace's.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const POM: &str = env!("CARGO_BIN_EXE_pom");

const DUMP: &str = "env | grep -E '^(DATABASE_URL|REDIS_URL|SOME_ALLOWED_DOMAINS|API_TOKEN|GREETING|ONLY_TEST|EXTRA)=' | sort > env.txt";

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new(test_block: &str) -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomrun")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        for repo in ["workspace--main/api", "workspace--feat/api"] {
            std::fs::create_dir_all(root.join(repo).join(".git")).expect("worktree");
        }
        std::fs::create_dir_all(temp.path().join("zdot")).expect("zdot");
        let config = format!(
            r#"session: demo
default_branch: main
shared_services:
  postgres: {{ image: "postgres:16" }}
  redis: {{ image: "redis:7" }}
repos:
  api:
    databases:
      main: "{{{{branch.safe}}}}"
    shared_services: [postgres, redis]
    env:
      DATABASE_URL: "{{{{db.main.url}}}}"
      REDIS_URL: "redis://{{{{shared.redis.host}}}}:{{{{shared.redis.port}}}}/{{{{slot.redis}}}}"
      SOME_ALLOWED_DOMAINS: "*"
      API_TOKEN: "{{{{secret.API_TOKEN}}}}"
      GREETING: hello
      SIGNING_KEY: "{{{{secret.SIGNING_KEY}}}}"
      LOG_LEVEL: debug
    commands:
      dump: "{DUMP}"
{test_block}"#
        );
        std::fs::write(root.join("pom.yml"), config).expect("pom.yml");
        let api = root.join("workspace--feat/api");
        std::fs::write(api.join(".env"), "SOME_ALLOWED_DOMAINS=*\n").expect(".env");
        std::fs::write(
            api.join(".env.test"),
            "SOME_ALLOWED_DOMAINS=example.test\nDATABASE_URL=postgres://ci/test\nREDIS_URL=redis://ci:6379/0\nAPI_TOKEN=test-token\nONLY_TEST=\"yes $SOME_ALLOWED_DOMAINS\"\nGREETING=from-test\n",
        )
        .expect(".env.test");
        std::fs::write(api.join(".env.test.local"), "GREETING=local-wins\n")
            .expect(".env.test.local");
        Fixture { temp }
    }

    fn api(&self) -> PathBuf {
        self.temp.path().join("project/workspace--feat/api")
    }

    fn pom(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(POM)
            .args(args)
            .current_dir(cwd)
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", "1")
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env("ZDOTDIR", self.temp.path().join("zdot"))
            .output()
            .expect("pom")
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.pom(&self.api(), args);
        assert!(
            output.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Runs `args`, then reads the variables the command saw.
    fn seen(&self, args: &[&str]) -> Vec<(String, String)> {
        self.ok(args);
        std::fs::read_to_string(self.api().join("env.txt"))
            .expect("env.txt")
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }
}

fn value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

#[test]
fn the_test_profile_takes_env_test_values_but_keeps_connections() {
    let fixture = Fixture::new("");
    let dev = fixture.seen(&["run", "dump"]);
    let test = fixture.seen(&["run", "--profile", "test", "dump"]);

    assert_eq!(value(&test, "SOME_ALLOWED_DOMAINS"), Some("example.test"));
    let database = value(&dev, "DATABASE_URL").expect("dev DATABASE_URL");
    assert!(database.ends_with("/demo_feat"), "{database}");
    assert_eq!(value(&test, "DATABASE_URL"), Some(database));
    let redis = value(&dev, "REDIS_URL").expect("dev REDIS_URL");
    assert!(redis.starts_with("redis://127.0.0.1:"), "{redis}");
    assert_eq!(value(&test, "REDIS_URL"), Some(redis));
    assert_eq!(value(&test, "API_TOKEN"), Some("test-token"));
    assert_eq!(value(&test, "ONLY_TEST"), Some("yes example.test"));
    assert_eq!(value(&test, "GREETING"), Some("local-wins"));
}

#[test]
fn without_a_profile_the_dev_env_is_unchanged() {
    let fixture = Fixture::new("");
    let dev = fixture.seen(&["run", "dump"]);
    assert_eq!(value(&dev, "SOME_ALLOWED_DOMAINS"), Some("*"));
    assert_eq!(value(&dev, "GREETING"), Some("hello"));
    assert_eq!(value(&dev, "API_TOKEN"), Some("{{secret.API_TOKEN}}"));
    assert_eq!(value(&dev, "ONLY_TEST"), None);
    let keys: Vec<&str> = dev.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "API_TOKEN",
            "DATABASE_URL",
            "GREETING",
            "REDIS_URL",
            "SOME_ALLOWED_DOMAINS"
        ]
    );
    let explained = fixture.ok(&["run", "--explain-env", "--json", "dump"]);
    let explained: serde_json::Value = serde_json::from_str(&explained).expect("json");
    assert_eq!(explained["profile"], "dev");
    assert_eq!(explained["env_files"], serde_json::json!([]));
}

#[test]
fn run_test_applies_the_profile_from_the_test_block() {
    let block = format!(
        "    test:\n      command: \"{DUMP}\"\n      env:\n        EXTRA: \"{{{{branch}}}}\"\n        DATABASE_URL: \"{{{{db.main.url}}}}_test\"\n"
    );
    let fixture = Fixture::new(&block);
    let test = fixture.seen(&["run", "test"]);
    assert_eq!(value(&test, "SOME_ALLOWED_DOMAINS"), Some("example.test"));
    assert_eq!(value(&test, "EXTRA"), Some("feat"));
    let database = value(&test, "DATABASE_URL").expect("DATABASE_URL");
    assert!(database.ends_with("/demo_feat_test"), "{database}");

    let opted_out = fixture.seen(&["run", "--profile", "dev", "test"]);
    assert_eq!(value(&opted_out, "SOME_ALLOWED_DOMAINS"), Some("*"));
    assert_eq!(value(&opted_out, "EXTRA"), None);

    let by_hand = fixture.seen(&["run", DUMP]);
    assert_eq!(
        value(&by_hand, "SOME_ALLOWED_DOMAINS"),
        Some("example.test")
    );
}

#[test]
fn run_test_without_a_command_says_so() {
    let fixture = Fixture::new("    test: { env_file: .env.test }\n");
    let output = fixture.pom(&fixture.api(), &["run", "test"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no test command"));
}

/// Shared service ports are leased per machine, so the snapshot holds a placeholder.
fn without_port(url: &str) -> String {
    let Some(colon) = url.rfind(':') else {
        return url.to_string();
    };
    let digits = url[colon + 1..]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    format!("{}:<port>{}", &url[..colon], &url[colon + 1 + digits..])
}

#[test]
fn explain_env_json_snapshot() {
    let fixture = Fixture::new("");
    let json = fixture.ok(&[
        "run",
        "--explain-env",
        "--profile",
        "test",
        "--json",
        "dump",
    ]);
    let mut actual: serde_json::Value = serde_json::from_str(&json).expect("json");
    for line in actual["env"].as_array_mut().expect("env lines") {
        if line["connection"] == true {
            line["value"] = without_port(line["value"].as_str().unwrap_or_default()).into();
        }
    }
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/run_explain_env.json");
    if std::env::var_os("POM_SNAPSHOT_BLESS").is_some() {
        let pretty = serde_json::to_string_pretty(&actual).expect("serialize");
        std::fs::create_dir_all(golden.parent().expect("parent")).expect("fixtures dir");
        std::fs::write(&golden, pretty + "\n").expect("write golden");
    }
    let expected: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&golden).unwrap_or_else(|error| {
            panic!(
                "{} ({error}); run with POM_SNAPSHOT_BLESS=1",
                golden.display()
            )
        }))
        .expect("golden json");
    assert_eq!(actual, expected);
    assert!(
        !fixture.api().join("env.txt").exists(),
        "explain must not run the command"
    );

    let text = fixture.ok(&["run", "--explain-env", "dump"]);
    assert!(text.contains("profile dev"), "{text}");
    assert!(text.contains("********"), "{text}");
}

#[test]
fn a_generated_env_file_is_not_read_as_the_test_env() {
    let fixture = Fixture::new("");
    std::fs::write(
        fixture.api().join(".env.test.local"),
        "# Auto-generated by pom. Do not commit.\nGREETING=generated\n",
    )
    .expect("generated file");
    let test = fixture.seen(&["run", "--profile", "test", "dump"]);
    assert_eq!(value(&test, "GREETING"), Some("from-test"));
}
