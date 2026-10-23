//! Queue counting against a throwaway `redis-server` on a random port; skipped when none is installed, unless
//! `POM_REDIS_TEST` is set (the Linux CI job).

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pom_config::ServiceQueue;
use pom_services::{count_queue, QueueKind};

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Err(error) = self.child.kill() {
            eprintln!("redis-server: {error}");
        }
        if let Err(error) = self.child.wait() {
            eprintln!("redis-server: {error}");
        }
    }
}

fn tool(name: &str) -> Option<PathBuf> {
    pom_services::tool_path()
        .split(':')
        .map(|dir| PathBuf::from(dir).join(name))
        .find(|path| path.is_file())
}

fn start() -> Option<Server> {
    let (Some(server), Some(_)) = (tool("redis-server"), tool("redis-cli")) else {
        assert!(
            std::env::var_os("POM_REDIS_TEST").is_none(),
            "POM_REDIS_TEST is set but redis-server is not installed"
        );
        eprintln!("no redis-server here: skipped");
        return None;
    };
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .expect("free port");
    let child = Command::new(server)
        .args([
            "--port",
            &port.to_string(),
            "--bind",
            "127.0.0.1",
            "--save",
            "",
            "--appendonly",
            "no",
        ])
        .stdout(Stdio::null())
        .spawn()
        .expect("redis-server");
    let server = Server { child, port };
    let deadline = Instant::now() + Duration::from_secs(10);
    while cli(&server, 0, &[], &["PING".into()])
        .map(|reply| reply.trim() != "PONG")
        .unwrap_or(true)
    {
        assert!(Instant::now() < deadline, "redis-server did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    Some(server)
}

fn cli(
    server: &Server,
    database: u32,
    extra: &[String],
    commands: &[String],
) -> Result<String, String> {
    use std::io::Write as _;
    let mut child = Command::new(tool("redis-cli").ok_or("no redis-cli")?)
        .args(["-p", &server.port.to_string(), "-n", &database.to_string()])
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let input = commands
        .iter()
        .map(|command| format!("{command}\n"))
        .collect::<String>();
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(input.as_bytes())
        .map_err(|error| error.to_string())?;
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run(server: &Server, database: u32, commands: &[&str]) {
    let commands: Vec<String> = commands.iter().map(|command| command.to_string()).collect();
    let reply = cli(server, database, &[], &commands).expect("redis-cli");
    assert!(!reply.contains("ERR"), "{reply}");
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn queue(kind: &str, queues: &[&str]) -> ServiceQueue {
    ServiceQueue {
        kind: kind.into(),
        queues: queues.iter().map(|name| name.to_string()).collect(),
        ..ServiceQueue::default()
    }
}

#[test]
fn bull_repeat_jobs_and_future_delays_never_block_idle() {
    let Some(server) = start() else {
        return;
    };
    let now = now_ms();
    let next = now + 300_000;
    let past = now - 5_000;
    let soon = now + 60_000;
    run(
        &server,
        3,
        &[
            "HSET bull:emails:meta opts.maxLenEvents 10000 version bullmq:5.0.0",
            "SET bull:emails:id 2",
            &format!("ZADD bull:emails:repeat {next} daily::::0*/5*",),
            &format!(
                "ZADD bull:emails:delayed {} repeat:daily:{next}",
                next * 4096
            ),
            "HSET bull:emails:repeat:daily:1 name daily",
            "SET bull:legacy:id 4",
            "HSET bull:legacy:1 name x",
            &format!("ZADD bull:legacy:delayed {soon} 2"),
            "RPUSH bull:legacy:completed 3",
        ],
    );
    let mut redis = |extra: &[String], commands: &[String]| cli(&server, 3, extra, commands);
    let counts =
        count_queue(QueueKind::Bullmq, &queue("bullmq", &[]), &mut redis, now).expect("counts");
    assert_eq!(
        counts.waiting.keys().collect::<Vec<_>>(),
        ["emails", "legacy"],
        "{counts:?}"
    );
    assert_eq!(
        (counts.repeat_scheduled, counts.repeatable),
        (1, 1),
        "{counts:?}"
    );
    assert_eq!(
        (counts.delayed_future, counts.delayed_due),
        (1, 0),
        "{counts:?}"
    );
    assert_eq!(
        counts.total(),
        0,
        "a cron and a later delay are idle: {counts:?}"
    );

    run(
        &server,
        3,
        &[
            &format!("ZADD bull:legacy:delayed {past} 4"),
            &format!("ZADD bull:emails:delayed {} 7", (past << 12) + 1),
            "ZADD bull:emails:prioritized 1 8",
            "LPUSH bull:emails:wait 0:123 5",
            "LPUSH bull:legacy:active 6",
        ],
    );
    let mut redis = |extra: &[String], commands: &[String]| cli(&server, 3, extra, commands);
    let counts =
        count_queue(QueueKind::Bullmq, &queue("bull", &[]), &mut redis, now).expect("counts");
    assert_eq!(counts.delayed_due, 2, "{counts:?}");
    assert_eq!(counts.delayed_future, 1, "{counts:?}");
    assert_eq!(
        (counts.waiting["emails"], counts.active, counts.prioritized),
        (1, 1, 1),
        "{counts:?}"
    );
    assert_eq!(counts.total(), 5, "{counts:?}");

    let mut other = |extra: &[String], commands: &[String]| cli(&server, 0, extra, commands);
    let empty =
        count_queue(QueueKind::Bullmq, &queue("bullmq", &[]), &mut other, now).expect("counts");
    assert_eq!(empty.total(), 0, "another database holds nothing");
}

#[test]
fn sidekiq_counts_only_due_scheduled_work() {
    let Some(server) = start() else {
        return;
    };
    let now = now_ms();
    let seconds = now / 1000;
    run(
        &server,
        1,
        &[
            "SADD queues default",
            "SADD processes host:1",
            "HSET host:1 busy 0",
            &format!("ZADD schedule {} later", seconds + 3600),
            &format!("ZADD retry {} again", seconds + 60),
        ],
    );
    let mut redis = |extra: &[String], commands: &[String]| cli(&server, 1, extra, commands);
    let counts =
        count_queue(QueueKind::Sidekiq, &queue("sidekiq", &[]), &mut redis, now).expect("counts");
    assert_eq!(
        (counts.total(), counts.delayed_future),
        (0, 2),
        "{counts:?}"
    );

    run(
        &server,
        1,
        &[
            "LPUSH queue:default a b",
            &format!("ZADD schedule {} now", seconds - 10),
            "HSET host:1 busy 2",
        ],
    );
    let mut redis = |extra: &[String], commands: &[String]| cli(&server, 1, extra, commands);
    let counts =
        count_queue(QueueKind::Sidekiq, &queue("sidekiq", &[]), &mut redis, now).expect("counts");
    assert_eq!(counts.waiting["default"], 2, "{counts:?}");
    assert_eq!(
        (counts.delayed_due, counts.active, counts.total()),
        (1, 2, 5),
        "{counts:?}"
    );
}
