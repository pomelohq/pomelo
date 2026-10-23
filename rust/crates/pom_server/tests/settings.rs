//! `settings.get` / `settings.set` against a real server on a socket, its settings.json in a temp folder: the
//! host keys go both ways, anything else is refused, values are checked, the rest of the file stays, and an
//! agent on the machine cannot change them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_remote::server_api::{method, HostSettings};
use pom_rpc::{error_code, Connection, Error};
use pom_server::{client, ServerHandle, ServerOptions, ServerPaths, SettingsService};
use serde_json::{json, Value};

struct Host {
    _temp: tempfile::TempDir,
    settings: PathBuf,
    handle: ServerHandle,
    thread: Option<JoinHandle<String>>,
    connection: Connection,
    asked_about: Arc<Mutex<Vec<i32>>>,
}

impl Host {
    fn start(agent: bool) -> Host {
        let temp = tempfile::Builder::new()
            .prefix("poms")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path();
        let settings = root.join("config/settings.json");
        std::fs::create_dir_all(settings.parent().expect("parent")).expect("config");
        std::fs::write(
            &settings,
            r#"{"theme": "One Dark", "from_a_newer_pom": {"keep": true}, "dev_proxy_port": 9100}"#,
        )
        .expect("settings.json");
        let asked_about = Arc::new(Mutex::new(Vec::new()));
        let asked = asked_about.clone();
        let service = SettingsService::new(
            Some(settings.clone()),
            Arc::new(move |pid| {
                if let Ok(mut asked) = asked.lock() {
                    asked.push(pid);
                }
                agent
            }),
        );
        let dispatcher = pom_server::Dispatcher::new(
            StateDir::new(root.join("state")),
            SocketDir::new(root.join("s")),
            PathBuf::from("/bin/false"),
            pom_server::OP_MEMORY,
        )
        .with_settings(service);
        let paths = ServerPaths::new(&root.join("run/pom-server"), &root.join("log"), "9.9.9");
        let mut options = ServerOptions::new(paths.clone(), "9.9.9", dispatcher);
        options.idle_exit = None;
        let server = pom_server::bind(options).expect("bind");
        let handle = server.handle();
        let thread = std::thread::spawn(move || server.run());
        let (connection, hello) = client::connect(&paths).expect("connect").expect("up");
        for name in [method::SETTINGS_GET, method::SETTINGS_SET] {
            assert!(hello.features.iter().any(|known| known == name), "{name}");
        }
        Host {
            _temp: temp,
            settings,
            handle,
            thread: Some(thread),
            connection,
            asked_about,
        }
    }

    fn ask(&self, name: &str, params: Value) -> Result<HostSettings, pom_rpc::RemoteError> {
        let answer = self
            .connection
            .start_request(name, &params)
            .and_then(|mut pending| pending.wait_timeout(std::time::Duration::from_secs(5)));
        match answer {
            Ok(value) => Ok(serde_json::from_value(value).expect("host settings")),
            Err(Error::Remote(error)) => Err(error),
            Err(other) => panic!("{name}: {other}"),
        }
    }

    fn file(&self) -> Value {
        read(&self.settings)
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.handle.stop();
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                eprintln!("server thread panicked");
            }
        }
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json")
}

#[test]
fn the_host_keys_go_both_ways_and_the_rest_of_the_file_stays() {
    let host = Host::start(false);
    let got = host.ask(method::SETTINGS_GET, Value::Null).expect("get");
    assert_eq!(got.values["dev_proxy_port"], json!(9100));
    assert_eq!(got.values["agent_command"], json!("claude"));
    assert_eq!(got.values["proxy"]["bind"], json!("127.0.0.1"));
    assert_eq!(got.values["server"]["idle_exit_minutes"], json!(30));
    assert!(got.values.get("theme").is_none(), "only host keys");
    assert!(got.keys.iter().any(|key| key == "proxy.trusted_proxies"));

    let set = host
        .ask(
            method::SETTINGS_SET,
            json!({"values": {"agent_command": "codex --full-auto", "terminal_shell": " /bin/bash ",
                              "proxy": {"bind": "[::]", "domain": "Dev.Example.NET"},
                              "server": {"reconnect_retention_minutes": 20}}}),
        )
        .expect("set");
    assert_eq!(set.values["agent_command"], json!("codex --full-auto"));
    assert_eq!(set.values["terminal_shell"], json!("/bin/bash"));
    assert_eq!(set.values["proxy"]["bind"], json!("::"));
    assert_eq!(set.values["proxy"]["domain"], json!("dev.example.net"));
    assert_eq!(set.values["dev_proxy_port"], json!(9100));
    assert_eq!(
        host.ask(method::SETTINGS_GET, Value::Null).expect("again"),
        set
    );
    let file = host.file();
    assert_eq!(file["theme"], json!("One Dark"));
    assert_eq!(file["from_a_newer_pom"], json!({"keep": true}));
    assert_eq!(file["server"], json!({"reconnect_retention_minutes": 20}));
    assert_eq!(
        host.asked_about.lock().expect("asked").as_slice(),
        [std::process::id() as i32],
        "the caller is told apart by its own process"
    );
}

#[test]
fn a_key_of_this_mac_or_a_bad_value_changes_nothing() {
    let host = Host::start(false);
    let before = host.file();
    for (values, wanted) in [
        (
            json!({"theme": "Pomelo Light"}),
            "not a setting of this machine: theme",
        ),
        (json!({"agent_command": "x", "vim_mode": true}), "vim_mode"),
        (json!({"dev_proxy_enabled": false}), "dev_proxy_enabled"),
        (
            json!({"server": {"unconfirmed_input_kib": 8}}),
            "server.unconfirmed_input_kib",
        ),
        (
            json!({"webhook_port": 80}),
            "webhook_port: 80 is not between 1024 and 65535",
        ),
        (
            json!({"modules_unused_days": 400}),
            "400 is not between 0 and 365",
        ),
        (json!({"proxy": {"public_scheme": "ftp"}}), "http or https"),
        (
            json!({"proxy": {"trusted_proxies": ["10.0.0.0/33"]}}),
            "not an address or range",
        ),
        (json!("agent_command"), "must be a JSON object"),
    ] {
        let error = host
            .ask(method::SETTINGS_SET, json!({ "values": values }))
            .expect_err("refused");
        assert_eq!(error.code, error_code::INVALID_PARAMS, "{values}");
        assert!(
            error.message.contains(wanted),
            "{values}: {}",
            error.message
        );
        assert_eq!(host.file(), before, "{values}");
    }
}

#[test]
fn an_agent_on_the_host_cannot_change_its_settings() {
    let host = Host::start(true);
    let before = host.file();
    assert!(host.ask(method::SETTINGS_GET, Value::Null).is_ok());
    let error = host
        .ask(
            method::SETTINGS_SET,
            json!({"values": {"agent_command": "sh -c 'curl example.com'"}}),
        )
        .expect_err("refused");
    assert!(error.message.contains("only a person"), "{}", error.message);
    assert_eq!(host.file(), before);
}
