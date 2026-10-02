//! How much work a worker service's job queue still holds, in the workspace's Redis slot, and waiting until it
//! holds none.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pom_config::{Config, ServiceQueue};
use serde::Serialize;

use crate::control::{ServiceRunner, ServiceTarget};
use crate::shared::output_text;

const DEFAULT_BULL_PREFIX: &str = "bull";
const REDIS_IMAGE: &str = "redis:7-alpine";
/// Idle must hold this long, so a job between two queues does not look like the end.
const SETTLE: Duration = Duration::from_millis(200);
const POLL: Duration = Duration::from_millis(250);

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

/// What is still waiting or running.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct QueueCounts {
    /// Jobs waiting per queue.
    pub waiting: BTreeMap<String, u64>,
    /// Jobs being worked on now.
    pub active: u64,
    /// Scheduled or retried jobs already due.
    pub due: u64,
    /// BullMQ jobs delayed or waiting by priority.
    pub delayed: u64,
}

impl QueueCounts {
    pub fn total(&self) -> u64 {
        self.waiting.values().sum::<u64>() + self.active + self.due + self.delayed
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Redis commands that count a queue's work, one reply line each, with how each line is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Count {
    Waiting(String),
    Active,
    Due,
    Delayed,
}

/// The counting commands for `queues`; `workers` are Sidekiq process identities whose busy count is read.
pub fn count_commands(
    kind: QueueKind,
    prefix: &str,
    queues: &[String],
    workers: &[String],
    now: u64,
) -> Vec<(String, Count)> {
    let mut commands = Vec::new();
    match kind {
        QueueKind::Sidekiq => {
            for queue in queues {
                commands.push((format!("LLEN queue:{queue}"), Count::Waiting(queue.clone())));
            }
            for set in ["schedule", "retry"] {
                commands.push((format!("ZCOUNT {set} -inf {now}"), Count::Due));
            }
            for worker in workers {
                commands.push((format!("HGET {worker} busy"), Count::Active));
            }
        }
        QueueKind::Bullmq => {
            for queue in queues {
                let key = format!("{prefix}:{queue}");
                commands.push((format!("LLEN {key}:wait"), Count::Waiting(queue.clone())));
                commands.push((format!("LLEN {key}:active"), Count::Active));
                commands.push((format!("ZCARD {key}:delayed"), Count::Delayed));
                commands.push((format!("ZCARD {key}:prioritized"), Count::Delayed));
            }
        }
    }
    commands
}

/// Adds up the replies of `count_commands`, one line each in order (an empty line is nil, so zero).
pub fn tally(commands: &[(String, Count)], replies: &str) -> QueueCounts {
    let mut counts = QueueCounts::default();
    for ((_, count), reply) in commands.iter().zip(replies.lines()) {
        let value: u64 = reply.trim().parse().unwrap_or(0);
        match count {
            Count::Waiting(queue) => *counts.waiting.entry(queue.clone()).or_default() += value,
            Count::Active => counts.active += value,
            Count::Due => counts.due += value,
            Count::Delayed => counts.delayed += value,
        }
    }
    counts
}

/// BullMQ queue names from `<prefix>:<queue>:meta` keys.
pub fn bull_queues_from_keys(keys: &str, prefix: &str) -> Vec<String> {
    let mut queues: Vec<String> = keys
        .lines()
        .filter_map(|key| {
            key.trim()
                .strip_prefix(&format!("{prefix}:"))?
                .strip_suffix(":meta")
        })
        .map(str::to_string)
        .collect();
    queues.sort();
    queues.dedup();
    queues
}

/// Where a workspace's queue lives: Redis host port and database number.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RedisSlot {
    port: u16,
    database: u32,
}

impl ServiceRunner {
    fn queue_of(config: &Config, target: &ServiceTarget) -> Result<(ServiceQueue, String), String> {
        let dir = config
            .repos
            .get(&target.repo)
            .ok_or_else(|| format!("no repo {}", target.repo))?;
        let queue = dir
            .services
            .get(&target.service)
            .and_then(|service| service.queue.clone())
            .ok_or_else(|| {
                format!(
                    "{}/{} has no queue: add `queue: {{ kind: sidekiq }}` (or bullmq) to the service in pom.yml",
                    target.repo, target.service
                )
            })?;
        let redis = if queue.redis.is_empty() {
            dir.shared_refs
                .iter()
                .map(|shared| shared.name.clone())
                .find(|name| {
                    config.shared_services.get(name).is_some_and(|def| {
                        def.kind == "redis" || name == "redis" || def.image.starts_with("redis")
                    })
                })
                .ok_or_else(|| format!("{} uses no shared Redis; set queue.redis", target.repo))?
        } else {
            queue.redis.clone()
        };
        Ok((queue, redis))
    }

    fn redis_slot(&self, redis: &str, branch: &str) -> RedisSlot {
        let slot = self.slots.get(redis, &pom_env::port_ws_key(branch));
        RedisSlot {
            port: self.shared_host_port(redis) + slot.map_or(0, |slot| slot.instance),
            database: slot.map_or(0, |slot| slot.slot),
        }
    }

    /// Runs `commands` through `redis-cli` against the slot, one reply line each.
    fn redis_cli(&self, slot: &RedisSlot, commands: &[String]) -> Result<String, String> {
        let database = slot.database.to_string();
        let args: Vec<String> = match self.published_container(slot.port) {
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
                &slot.port.to_string(),
                "-n",
                &database,
            ]
            .map(str::to_string)
            .to_vec(),
        };
        let input = commands.join("\n") + "\n";
        let output = self
            .docker_input(&args, input.as_bytes())
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(format!("redis-cli: {}", output_text(&output).trim()));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// The work `target`'s queue still holds.
    pub fn queue_counts(
        &self,
        config: &Config,
        target: &ServiceTarget,
    ) -> Result<QueueCounts, String> {
        let (queue, redis) = Self::queue_of(config, target)?;
        let kind = QueueKind::parse(&queue.kind)?;
        let prefix = if queue.prefix.is_empty() {
            DEFAULT_BULL_PREFIX
        } else {
            queue.prefix.as_str()
        };
        let slot = self.redis_slot(&redis, &target.branch);
        let mut queues = queue.queues.clone();
        let mut workers = Vec::new();
        match kind {
            QueueKind::Sidekiq => {
                let found = self.redis_cli(&slot, &["SMEMBERS queues".into()])?;
                if queues.is_empty() {
                    queues = found
                        .lines()
                        .map(str::trim)
                        .filter(|q| !q.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                workers = self
                    .redis_cli(&slot, &["SMEMBERS processes".into()])?
                    .lines()
                    .map(str::trim)
                    .filter(|worker| !worker.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            QueueKind::Bullmq if queues.is_empty() => {
                let keys = self.redis_cli(&slot, &[format!("KEYS {prefix}:*:meta")])?;
                queues = bull_queues_from_keys(&keys, prefix);
            }
            QueueKind::Bullmq => {}
        }
        let commands = count_commands(kind, prefix, &queues, &workers, now_secs());
        if commands.is_empty() {
            return Ok(QueueCounts::default());
        }
        let lines: Vec<String> = commands
            .iter()
            .map(|(command, _)| command.clone())
            .collect();
        let replies = self.redis_cli(&slot, &lines)?;
        Ok(tally(&commands, &replies))
    }

    /// Polls `target`'s queue until it holds no work twice in a row, or `timeout` passes (then the last counts).
    pub fn wait_queue_idle(
        &self,
        config: &Config,
        target: &ServiceTarget,
        timeout: Duration,
    ) -> Result<Result<QueueCounts, QueueCounts>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let counts = self.queue_counts(config, target)?;
            if counts.total() == 0 {
                std::thread::sleep(SETTLE);
                let again = self.queue_counts(config, target)?;
                if again.total() == 0 {
                    return Ok(Ok(again));
                }
            }
            if Instant::now() >= deadline {
                return Ok(Err(counts));
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
            100,
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
                "ZCOUNT retry -inf 100",
                "HGET host:1:abc busy",
            ]
        );
        let counts = tally(&commands, "3\n0\n1\n0\n2\n");
        assert_eq!(counts.waiting["default"], 3);
        assert_eq!((counts.due, counts.active, counts.total()), (1, 2, 6));
        assert_eq!(
            tally(&commands, "0\n0\n0\n0\n\n").total(),
            0,
            "a nil busy is zero"
        );
    }

    #[test]
    fn bullmq_counts_wait_active_delayed_and_prioritized() {
        let commands = count_commands(QueueKind::Bullmq, "bull", &["emails".into()], &[], 0);
        assert_eq!(commands[0].0, "LLEN bull:emails:wait");
        assert_eq!(commands[3].0, "ZCARD bull:emails:prioritized");
        let counts = tally(&commands, "2\n1\n4\n0\n");
        assert_eq!(
            (counts.waiting["emails"], counts.active, counts.delayed),
            (2, 1, 4)
        );
        assert_eq!(
            bull_queues_from_keys(
                "bull:emails:meta\nbull:reports:meta\nother:x:meta\n",
                "bull"
            ),
            ["emails", "reports"]
        );
        assert!(QueueKind::parse("resque").is_err());
    }
}
