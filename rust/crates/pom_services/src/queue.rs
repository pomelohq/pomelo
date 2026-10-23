//! How much work a worker service's job queue still holds, in the workspace's Redis slot, and waiting until it
//! holds none.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pom_config::{Config, Dir, ServiceQueue, SharedServiceDef};
use serde::Serialize;

use crate::control::{ServiceRunner, ServiceTarget};
use crate::shared::output_text;

pub const DEFAULT_BULL_PREFIX: &str = "bull";
const REDIS_IMAGE: &str = "redis:7-alpine";
/// Idle must hold this long, so a job between two queues does not look like the end.
const SETTLE: Duration = Duration::from_millis(200);
const POLL: Duration = Duration::from_millis(250);

/// The key suffixes that mark a Bull, Bull v4 or BullMQ queue; Bull v4 writes no `meta`.
const BULL_QUEUE_KEYS: &[&str] = &[
    "id",
    "wait",
    "active",
    "delayed",
    "completed",
    "failed",
    "repeat",
    "meta",
    "paused",
    "prioritized",
    "waiting-children",
    "priority",
    "events",
    "marker",
    "stalled",
];

/// Counts one Bull or BullMQ queue (KEYS[1] is `<prefix>:<queue>`, ARGV[1] now in ms) in one round trip, as
/// one line: wait active paused prioritized waiting-children delayed-due delayed-future repeat-due
/// repeat-future repeatable. BullMQ scores delayed jobs as `ms * 0x1000 + counter`, older Bull as plain ms,
/// so a score past 1e14 is decoded first; a `0:` entry in `wait` is a BullMQ 4 marker, not a job.
const BULL_COUNT_SCRIPT: &str = "local k = KEYS[1] local now = tonumber(ARGV[1]) \
local wait = 0 for _, id in ipairs(redis.call('LRANGE', k .. ':wait', 0, -1)) do \
if string.sub(id, 1, 2) ~= '0:' and id ~= '0' then wait = wait + 1 end end \
local d = {0, 0, 0, 0} local items = redis.call('ZRANGE', k .. ':delayed', 0, -1, 'WITHSCORES') \
for i = 1, #items, 2 do local at = tonumber(items[i + 1]) \
if at >= 1e14 then at = math.floor(at / 4096) end \
local slot = 2 if at <= now then slot = 1 end \
if string.sub(items[i], 1, 7) == 'repeat:' then slot = slot + 2 end d[slot] = d[slot] + 1 end \
return table.concat({wait, redis.call('LLEN', k .. ':active'), redis.call('LLEN', k .. ':paused'), \
redis.call('ZCARD', k .. ':prioritized'), redis.call('ZCARD', k .. ':waiting-children'), \
d[1], d[2], d[3], d[4], redis.call('ZCARD', k .. ':repeat')}, ' ')";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QueueKind {
    Sidekiq,
    Bullmq,
}

impl QueueKind {
    pub fn parse(text: &str) -> Result<QueueKind, String> {
        match text.to_ascii_lowercase().as_str() {
            "sidekiq" => Ok(QueueKind::Sidekiq),
            "bullmq" | "bull" => Ok(QueueKind::Bullmq),
            other => Err(format!("queue kind {other:?}: use sidekiq or bullmq")),
        }
    }
}

/// What a queue holds, by category; only waiting, active, due delayed and prioritized jobs keep it busy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct QueueCounts {
    /// Jobs waiting per queue.
    pub waiting: BTreeMap<String, u64>,
    /// Jobs being worked on now.
    pub active: u64,
    /// Delayed, scheduled or retried jobs whose time has come.
    pub delayed_due: u64,
    /// Delayed, scheduled or retried jobs not due yet.
    pub delayed_future: u64,
    /// BullMQ jobs waiting by priority.
    pub prioritized: u64,
    /// The next runs of repeatable (cron) jobs, not due yet.
    pub repeat_scheduled: u64,
    /// Repeatable job definitions.
    pub repeatable: u64,
    /// Jobs held in a paused queue.
    pub paused: u64,
    /// BullMQ parents waiting for their children.
    pub waiting_children: u64,
}

impl QueueCounts {
    /// The work still to do: what `wait-idle` waits on.
    pub fn total(&self) -> u64 {
        self.waiting.values().sum::<u64>() + self.active + self.delayed_due + self.prioritized
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Redis commands that count a queue's work, one reply line each, with how each line is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Count {
    Waiting(String),
    Active,
    Due,
    Future,
    Bull(String),
}

/// The counting commands for `queues`; `workers` are Sidekiq process identities whose busy count is read.
pub fn count_commands(
    kind: QueueKind,
    prefix: &str,
    queues: &[String],
    workers: &[String],
    now_ms: u64,
) -> Vec<(String, Count)> {
    let mut commands = Vec::new();
    match kind {
        QueueKind::Sidekiq => {
            let now = now_ms as f64 / 1000.0;
            for queue in queues {
                commands.push((format!("LLEN queue:{queue}"), Count::Waiting(queue.clone())));
            }
            for set in ["schedule", "retry"] {
                commands.push((format!("ZCOUNT {set} -inf {now}"), Count::Due));
                commands.push((format!("ZCOUNT {set} ({now} +inf"), Count::Future));
            }
            for worker in workers {
                commands.push((format!("HGET {worker} busy"), Count::Active));
            }
        }
        QueueKind::Bullmq => {
            for queue in queues {
                commands.push((
                    format!("EVAL \"{BULL_COUNT_SCRIPT}\" 1 {prefix}:{queue} {now_ms}"),
                    Count::Bull(queue.clone()),
                ));
            }
        }
    }
    commands
}

fn number(reply: &str) -> Result<u64, String> {
    let reply = reply.trim();
    if reply.is_empty() {
        return Ok(0);
    }
    reply
        .parse()
        .map_err(|_| format!("redis answered {reply:?}"))
}

/// Adds up the replies of `count_commands`, one line each in order (an empty line is nil, so zero).
pub fn tally(commands: &[(String, Count)], replies: &str) -> Result<QueueCounts, String> {
    let mut counts = QueueCounts::default();
    let replies: Vec<&str> = replies.lines().collect();
    if replies.len() < commands.len() {
        return Err(format!(
            "redis answered {} of {} commands: {}",
            replies.len(),
            commands.len(),
            replies.join(" / ")
        ));
    }
    for ((_, count), reply) in commands.iter().zip(replies) {
        match count {
            Count::Waiting(queue) => {
                *counts.waiting.entry(queue.clone()).or_default() += number(reply)?
            }
            Count::Active => counts.active += number(reply)?,
            Count::Due => counts.delayed_due += number(reply)?,
            Count::Future => counts.delayed_future += number(reply)?,
            Count::Bull(queue) => {
                let values = reply
                    .split_whitespace()
                    .map(number)
                    .collect::<Result<Vec<u64>, String>>()?;
                let [wait, active, paused, prioritized, children, due, future, repeat_due, repeat_future, repeatable] =
                    values[..]
                else {
                    return Err(format!("redis answered {reply:?} for queue {queue}"));
                };
                *counts.waiting.entry(queue.clone()).or_default() += wait;
                counts.active += active;
                counts.paused += paused;
                counts.prioritized += prioritized;
                counts.waiting_children += children;
                counts.delayed_due += due + repeat_due;
                counts.delayed_future += future;
                counts.repeat_scheduled += repeat_future;
                counts.repeatable += repeatable;
            }
        }
    }
    Ok(counts)
}

/// Queue names from the keys under `<prefix>:`, one per line; any of a queue's own keys names it.
pub fn bull_queues_from_keys(keys: &str, prefix: &str) -> Vec<String> {
    let head = format!("{prefix}:");
    let mut queues: Vec<String> = keys
        .lines()
        .filter_map(|key| {
            let (queue, suffix) = key.trim().strip_prefix(&head)?.rsplit_once(':')?;
            (!queue.is_empty() && BULL_QUEUE_KEYS.contains(&suffix)).then(|| queue.to_string())
        })
        .collect();
    queues.sort();
    queues.dedup();
    queues
}

/// Runs redis-cli with extra arguments (before the commands) and one command per stdin line.
pub type RedisCli<'a> = dyn FnMut(&[String], &[String]) -> Result<String, String> + 'a;

/// Counts what `queue` holds through `cli`, finding the queues (and Sidekiq workers) when none are named.
pub fn count_queue(
    kind: QueueKind,
    queue: &ServiceQueue,
    cli: &mut RedisCli<'_>,
    now_ms: u64,
) -> Result<QueueCounts, String> {
    let prefix = queue_prefix(queue);
    let mut queues = queue.queues.clone();
    let mut workers = Vec::new();
    let lines = |text: String| -> Vec<String> {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    };
    match kind {
        QueueKind::Sidekiq => {
            if queues.is_empty() {
                queues = lines(cli(&[], &["SMEMBERS queues".into()])?);
            }
            workers = lines(cli(&[], &["SMEMBERS processes".into()])?);
        }
        QueueKind::Bullmq if queues.is_empty() => {
            let scan = [
                "--scan".to_string(),
                "--pattern".into(),
                format!("{prefix}:*"),
            ];
            queues = bull_queues_from_keys(&cli(&scan, &[])?, prefix);
        }
        QueueKind::Bullmq => {}
    }
    let commands = count_commands(kind, prefix, &queues, &workers, now_ms);
    if commands.is_empty() {
        return Ok(QueueCounts::default());
    }
    let input: Vec<String> = commands
        .iter()
        .map(|(command, _)| command.clone())
        .collect();
    tally(&commands, &cli(&[], &input)?)
}

fn queue_prefix(queue: &ServiceQueue) -> &str {
    if queue.prefix.is_empty() {
        DEFAULT_BULL_PREFIX
    } else {
        queue.prefix.as_str()
    }
}

pub fn is_redis(name: &str, def: &SharedServiceDef) -> bool {
    def.kind == "redis"
        || name == "redis"
        || def.image.starts_with("redis")
        || def.image.starts_with("valkey")
}

/// The shared Redis a queue uses, and why that one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RedisChoice {
    pub name: String,
    /// The database the env names; none means the workspace's slot on the service.
    pub database: Option<u32>,
    pub reason: String,
}

/// A `{{...}}` key that points at a shared service: `shared.<name>[.<field>]`, `slot.<name>` or `slot:<name>`.
fn shared_name(key: &str) -> Option<&str> {
    if let Some(rest) = key.strip_prefix("shared.") {
        return rest.split('.').next();
    }
    key.strip_prefix("slot.")
        .or_else(|| key.strip_prefix("slot:"))
}

fn is_slot_key(key: &str, name: &str) -> bool {
    key == format!("shared.{name}.slot")
        || key == format!("slot.{name}")
        || key == format!("slot:{name}")
}

/// The database number a URL value names after its last template, as in `redis://{{shared.redis.url}}/2`.
fn url_database(value: &str) -> Option<u32> {
    let tail = value.rsplit_once("}}").map_or(value, |(_, tail)| tail);
    let path = tail.split('?').next()?.strip_prefix('/')?;
    path.parse().ok()
}

struct EnvRef<'a> {
    name: String,
    key: &'a str,
    value: &'a str,
    env: &'a indexmap::IndexMap<String, String>,
    from_service: bool,
}

fn env_refs<'a>(config: &Config, dir: &'a Dir, service: &str) -> Vec<EnvRef<'a>> {
    let service_env = dir
        .services
        .get(service)
        .map(|service| (&service.env, true));
    let repo_envs = std::iter::once(&dir.env)
        .chain(dir.env_output.iter().map(|file| &file.env))
        .map(|env| (env, false));
    let mut refs = Vec::new();
    for (env, from_service) in service_env.into_iter().chain(repo_envs) {
        for (key, value) in env {
            for token in pom_env::template::refs(value) {
                let Some(name) = shared_name(&token) else {
                    continue;
                };
                let redis = config
                    .shared_services
                    .get(name)
                    .is_some_and(|def| is_redis(name, def));
                if redis
                    && !refs
                        .iter()
                        .any(|known: &EnvRef| known.key == key && known.name == name)
                {
                    refs.push(EnvRef {
                        name: name.to_string(),
                        key,
                        value,
                        env,
                        from_service,
                    });
                }
            }
        }
    }
    refs
}

fn distinct<'a>(names: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for name in names {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The database `name` is used with, from the env that points at it: its slot template, the number at the
/// end of a URL, or a `*REDIS_DB` sibling; a plain `redis://` URL is database 0.
fn env_database(refs: &[EnvRef], name: &str) -> Option<u32> {
    for found in refs.iter().filter(|found| found.name == name) {
        let tokens = pom_env::template::refs(found.value);
        if tokens.iter().any(|token| is_slot_key(token, name)) {
            return None;
        }
        if let Some(database) = url_database(found.value) {
            return Some(database);
        }
        let sibling = found.env.iter().find(|(key, _)| {
            let key = key.to_ascii_uppercase();
            key.contains("REDIS") && (key.ends_with("_DB") || key.ends_with("_DATABASE"))
        });
        if let Some((_, value)) = sibling {
            if pom_env::template::refs(value)
                .iter()
                .any(|token| is_slot_key(token, name))
            {
                return None;
            }
            if let Ok(database) = value.trim().parse() {
                return Some(database);
            }
        }
        let scheme = found.value.trim_start();
        if scheme.starts_with("redis://") || scheme.starts_with("rediss://") {
            return Some(0);
        }
    }
    None
}

/// Which shared Redis `repo/service`'s queue lives in: `explicit` (`--redis`), else `queue.redis`, else the
/// one its env points at, else the repo's only shared Redis.
pub fn pick_redis(
    config: &Config,
    repo: &str,
    service: &str,
    explicit: Option<&str>,
) -> Result<RedisChoice, String> {
    let dir = config
        .repos
        .get(repo)
        .ok_or_else(|| format!("no repo {repo}"))?;
    let refs = env_refs(config, dir, service);
    let named = explicit
        .map(|name| (name.to_string(), "--redis"))
        .or_else(|| {
            dir.services
                .get(service)
                .and_then(|service| service.queue.as_ref())
                .filter(|queue| !queue.redis.is_empty())
                .map(|queue| (queue.redis.clone(), "queue.redis"))
        });
    if let Some((name, reason)) = named {
        if !config.shared_services.contains_key(&name) {
            let known: Vec<&str> = config.shared_services.keys().map(String::as_str).collect();
            return Err(format!(
                "{reason} names {name}, which is no shared service (have: {})",
                known.join(", ")
            ));
        }
        return Ok(RedisChoice {
            database: env_database(&refs, &name),
            name,
            reason: reason.into(),
        });
    }
    let choose = |name: &str, reason: String| RedisChoice {
        name: name.to_string(),
        database: env_database(&refs, name),
        reason,
    };
    let from_env = distinct(refs.iter().map(|found| found.name.as_str()));
    let reason_for = |name: &str| {
        refs.iter()
            .find(|found| found.name == name)
            .map_or_else(String::new, |found| format!("env {}", found.key))
    };
    if let [only] = from_env[..] {
        return Ok(choose(only, reason_for(only)));
    }
    if from_env.len() > 1 {
        let queue_words = ["SIDEKIQ", "BULL", "QUEUE", "JOB", "WORKER"];
        let narrowed = [
            distinct(
                refs.iter()
                    .filter(|found| found.from_service)
                    .map(|found| found.name.as_str()),
            ),
            distinct(
                refs.iter()
                    .filter(|found| {
                        let key = found.key.to_ascii_uppercase();
                        queue_words.iter().any(|word| key.contains(word))
                    })
                    .map(|found| found.name.as_str()),
            ),
            distinct(
                refs.iter()
                    .filter(|found| found.key.eq_ignore_ascii_case("REDIS_URL"))
                    .map(|found| found.name.as_str()),
            ),
        ];
        if let Some([only]) = narrowed
            .iter()
            .map(Vec::as_slice)
            .find(|names| names.len() == 1)
        {
            return Ok(choose(only, reason_for(only)));
        }
        return Err(format!(
            "{repo}/{service} uses several shared Redis ({}): pick one with --redis <name> or set queue.redis",
            from_env.join(", ")
        ));
    }
    let listed = distinct(
        dir.shared_refs
            .iter()
            .map(|shared| shared.name.as_str())
            .filter(|name| {
                config
                    .shared_services
                    .get(*name)
                    .is_some_and(|def| is_redis(name, def))
            }),
    );
    match listed[..] {
        [only] => Ok(choose(only, "shared_services".into())),
        [] => Err(format!(
            "{repo} uses no shared Redis: point its env at one with {{{{shared.<name>.url}}}}, list it under shared_services, or set queue.redis"
        )),
        _ => Err(format!(
            "{repo} lists several shared Redis ({}): pick one with --redis <name> or set queue.redis",
            listed.join(", ")
        )),
    }
}

/// Where a workspace's queue lives: the shared Redis, its host port and database number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QueueRedis {
    pub name: String,
    pub port: u16,
    pub database: u32,
    pub reason: String,
}

/// One look at a queue: where it lives and what it holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QueueSnapshot {
    pub redis: QueueRedis,
    pub counts: QueueCounts,
}

impl ServiceRunner {
    fn queue_of(
        config: &Config,
        target: &ServiceTarget,
        redis: Option<&str>,
    ) -> Result<(ServiceQueue, RedisChoice), String> {
        let queue = config
            .repos
            .get(&target.repo)
            .ok_or_else(|| format!("no repo {}", target.repo))?
            .services
            .get(&target.service)
            .and_then(|service| service.queue.clone())
            .ok_or_else(|| {
                format!(
                    "{}/{} has no queue: add `queue: {{ kind: sidekiq }}` (or bullmq) to the service in pom.yml",
                    target.repo, target.service
                )
            })?;
        let choice = pick_redis(config, &target.repo, &target.service, redis)?;
        Ok((queue, choice))
    }

    fn queue_redis(&self, choice: RedisChoice, branch: &str) -> QueueRedis {
        let slot = self.slots.get(&choice.name, &pom_env::port_ws_key(branch));
        QueueRedis {
            port: self.shared_host_port(&choice.name) + slot.map_or(0, |slot| slot.instance),
            database: choice
                .database
                .unwrap_or_else(|| slot.map_or(0, |slot| slot.slot)),
            name: choice.name,
            reason: choice.reason,
        }
    }

    /// Runs `redis-cli <extra>` against the database, with one command per stdin line.
    fn redis_cli(
        &self,
        redis: &QueueRedis,
        extra: &[String],
        commands: &[String],
    ) -> Result<String, String> {
        let database = redis.database.to_string();
        let mut args: Vec<String> = match self.published_container(redis.port) {
            Some(container) => ["exec", "-i", &container, "redis-cli", "-n", &database]
                .map(str::to_string)
                .to_vec(),
            None => [
                "run",
                "--rm",
                "-i",
                "--add-host=host.docker.internal:host-gateway",
                REDIS_IMAGE,
                "redis-cli",
                "-h",
                "host.docker.internal",
                "-p",
                &redis.port.to_string(),
                "-n",
                &database,
            ]
            .map(str::to_string)
            .to_vec(),
        };
        args.extend_from_slice(extra);
        let input = if commands.is_empty() {
            String::new()
        } else {
            commands.join("\n") + "\n"
        };
        let output = self
            .docker_input(&args, input.as_bytes())
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(format!("redis-cli: {}", output_text(&output).trim()));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// The work `target`'s queue still holds, in the Redis `redis` names (or the one its config and env point at).
    pub fn queue_snapshot(
        &self,
        config: &Config,
        target: &ServiceTarget,
        redis: Option<&str>,
    ) -> Result<QueueSnapshot, String> {
        let (queue, choice) = Self::queue_of(config, target, redis)?;
        let kind = QueueKind::parse(&queue.kind)?;
        let redis = self.queue_redis(choice, &target.branch);
        let counts = count_queue(
            kind,
            &queue,
            &mut |extra, commands| self.redis_cli(&redis, extra, commands),
            now_millis(),
        )?;
        Ok(QueueSnapshot { redis, counts })
    }

    /// Polls `target`'s queue until it holds no work twice in a row, or `timeout` passes (then the last look).
    pub fn wait_queue_idle(
        &self,
        config: &Config,
        target: &ServiceTarget,
        redis: Option<&str>,
        timeout: Duration,
    ) -> Result<Result<QueueSnapshot, QueueSnapshot>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let snapshot = self.queue_snapshot(config, target, redis)?;
            if snapshot.counts.total() == 0 {
                std::thread::sleep(SETTLE);
                let again = self.queue_snapshot(config, target, redis)?;
                if again.counts.total() == 0 {
                    return Ok(Ok(again));
                }
            }
            if Instant::now() >= deadline {
                return Ok(Err(snapshot));
            }
            std::thread::sleep(POLL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidekiq_counts_waiting_due_and_busy_work() {
        let commands = count_commands(
            QueueKind::Sidekiq,
            "",
            &["default".into(), "mailers".into()],
            &["host:1:abc".into()],
            100_000,
        );
        let lines: Vec<&str> = commands
            .iter()
            .map(|(command, _)| command.as_str())
            .collect();
        assert_eq!(
            lines,
            [
                "LLEN queue:default",
                "LLEN queue:mailers",
                "ZCOUNT schedule -inf 100",
                "ZCOUNT schedule (100 +inf",
                "ZCOUNT retry -inf 100",
                "ZCOUNT retry (100 +inf",
                "HGET host:1:abc busy",
            ]
        );
        let counts = tally(&commands, "3\n0\n1\n5\n0\n0\n2\n").expect("tally");
        assert_eq!(counts.waiting["default"], 3);
        assert_eq!(
            (
                counts.delayed_due,
                counts.delayed_future,
                counts.active,
                counts.total()
            ),
            (1, 5, 2, 6)
        );
        assert_eq!(
            tally(&commands, "0\n0\n0\n0\n0\n0\n\n").map(|counts| counts.total()),
            Ok(0),
            "a nil busy is zero"
        );
        assert!(tally(&commands, "0\n").is_err(), "a short answer");
        assert!(tally(&commands, "WRONGTYPE x\n0\n0\n0\n0\n0\n0\n").is_err());
    }

    #[test]
    fn bull_counts_only_due_work_and_keeps_repeats_apart() {
        let commands = count_commands(QueueKind::Bullmq, "bull", &["emails".into()], &[], 7);
        assert!(commands[0].0.starts_with("EVAL \""));
        assert!(commands[0].0.ends_with("\" 1 bull:emails 7"));
        assert!(!BULL_COUNT_SCRIPT.contains('"') && !BULL_COUNT_SCRIPT.contains('\\'));
        let counts = tally(&commands, "2 1 0 3 0 1 4 0 1 1\n").expect("tally");
        assert_eq!(
            (
                counts.waiting["emails"],
                counts.active,
                counts.prioritized,
                counts.delayed_due,
                counts.delayed_future,
                counts.repeat_scheduled,
                counts.repeatable,
            ),
            (2, 1, 3, 1, 4, 1, 1)
        );
        assert_eq!(counts.total(), 7);
        assert!(tally(&commands, "2 1\n").is_err());
        assert!(QueueKind::parse("resque").is_err());
    }

    #[test]
    fn bull_queues_come_from_any_of_their_keys() {
        assert_eq!(
            bull_queues_from_keys(
                "bull:emails:meta\nbull:reports:id\nbull:reports:wait\nbull:legacy:delayed\nbull:emails:42\nbull:emails:repeat:abc:1700000000000\nother:x:meta\nbull:a:b:active\n",
                "bull"
            ),
            ["a:b", "emails", "legacy", "reports"]
        );
        assert_eq!(bull_queues_from_keys("jobs:mail:wait\n", "jobs"), ["mail"]);
    }

    fn config(yaml: &str) -> Config {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("pom.yml");
        std::fs::write(&path, yaml).expect("write");
        Config::load(&path).expect("config")
    }

    const SHARED: &str = "session: shop\nshared_services:\n  cache:\n    image: redis:7-alpine\n  jobs:\n    type: redis\n    image: valkey/valkey:8\n  postgres:\n    image: postgres:16\n";

    #[test]
    fn the_redis_comes_from_the_env_without_queue_redis() {
        let config = config(&format!(
            "{SHARED}repos:\n  api:\n    env:\n      DATABASE_URL: \"postgres://{{{{shared.postgres.url}}}}/x\"\n      REDIS_URL: \"redis://{{{{shared.jobs.host}}}}:{{{{shared.jobs.port}}}}/{{{{shared.jobs.slot}}}}\"\n    services:\n      worker:\n        cmd: run\n        queue: {{ kind: sidekiq }}\n"
        ));
        let choice = pick_redis(&config, "api", "worker", None).expect("pick");
        assert_eq!(
            choice,
            RedisChoice {
                name: "jobs".into(),
                database: None,
                reason: "env REDIS_URL".into()
            }
        );
    }

    #[test]
    fn several_redis_are_narrowed_by_queue_env_or_asked_for() {
        let config = config(&format!(
            "{SHARED}repos:\n  api:\n    env:\n      CACHE_URL: \"redis://{{{{shared.cache.url}}}}/1\"\n      BULL_REDIS_URL: \"redis://{{{{shared.jobs.url}}}}/4\"\n    services:\n      worker:\n        cmd: run\n        queue: {{ kind: bullmq }}\n  web:\n    env:\n      A_URL: \"redis://{{{{shared.cache.url}}}}\"\n      B_URL: \"{{{{shared.jobs.url}}}}\"\n    services:\n      worker:\n        cmd: run\n        queue: {{ kind: bullmq }}\n"
        ));
        let choice = pick_redis(&config, "api", "worker", None).expect("pick");
        assert_eq!((choice.name.as_str(), choice.database), ("jobs", Some(4)));
        let ambiguous = pick_redis(&config, "web", "worker", None).expect_err("ambiguous");
        assert!(
            ambiguous.contains("cache, jobs") && ambiguous.contains("--redis"),
            "{ambiguous}"
        );
        let chosen = pick_redis(&config, "web", "worker", Some("cache")).expect("explicit");
        assert_eq!((chosen.name.as_str(), chosen.database), ("cache", Some(0)));
        assert!(pick_redis(&config, "web", "worker", Some("nope")).is_err());
    }

    #[test]
    fn queue_redis_overrides_and_shared_refs_still_work() {
        let config = config(&format!(
            "{SHARED}repos:\n  api:\n    shared_services: [postgres, cache]\n    services:\n      worker:\n        cmd: run\n        queue: {{ kind: sidekiq }}\n      mailer:\n        cmd: run\n        env:\n          REDIS_DB: \"7\"\n          REDIS_HOST: \"{{{{shared.jobs.host}}}}\"\n        queue: {{ kind: sidekiq, redis: cache }}\n  bare:\n    services:\n      worker:\n        cmd: run\n        queue: {{ kind: sidekiq }}\n"
        ));
        let listed = pick_redis(&config, "api", "worker", None).expect("listed");
        assert_eq!(
            (listed.name.as_str(), listed.reason.as_str()),
            ("cache", "shared_services")
        );
        let named = pick_redis(&config, "api", "mailer", None).expect("named");
        assert_eq!(
            (named.name.as_str(), named.reason.as_str()),
            ("cache", "queue.redis")
        );
        assert!(pick_redis(&config, "bare", "worker", None).is_err());
    }

    #[test]
    fn database_numbers_from_env_values() {
        assert_eq!(url_database("redis://{{shared.r.url}}/2"), Some(2));
        assert_eq!(
            url_database("redis://{{shared.r.host}}:{{shared.r.port}}/11?x=1"),
            Some(11)
        );
        assert_eq!(url_database("{{shared.r.url}}"), None);
        assert_eq!(shared_name("shared.redis.slot"), Some("redis"));
        assert_eq!(shared_name("slot:cache"), Some("cache"));
        assert_eq!(shared_name("db.main"), None);
    }
}
