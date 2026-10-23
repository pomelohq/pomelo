//! Pushes for a remote app: each `watch.subscribe` gets a thread that reads the project here every second, or
//! as soon as an agent reports, and publishes a topic's full value only when it changed.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use pom_agent::{AgentWatcher, SessionView};
use pom_paths::StateDir;
use pom_remote::server_api::{
    topic, AgentsUpdate, ServicesUpdate, StatusUpdate, TurnUpdate, WatchBranchParams, WatchChannel,
    WatchParams, WorkspacesUpdate,
};
use pom_remote::{LocalSource, ProjectSource};
use pom_rpc::{error_code, Publisher, RemoteError};
use serde::Serialize;
use serde_json::Value;

use crate::dispatch::{check_branch, check_name};
use crate::pty::Caller;

pub const READ_EVERY: Duration = Duration::from_secs(1);

fn invalid(message: impl Into<String>) -> RemoteError {
    RemoteError::new(error_code::INVALID_PARAMS, message)
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct Wanted {
    branch: String,
    turn: Option<String>,
    stopped: bool,
    /// Read again now: the branch changed or an agent reported.
    poked: bool,
}

#[derive(Default)]
struct Entry {
    wanted: Mutex<Wanted>,
    changed: Condvar,
}

impl Entry {
    fn change(&self, edit: impl FnOnce(&mut Wanted)) {
        let mut wanted = locked(&self.wanted);
        edit(&mut wanted);
        wanted.poked = true;
        drop(wanted);
        self.changed.notify_all();
    }

    /// Waits up to `timeout` for a poke; the branch and turn to read next, or `None` once stopped.
    fn next(&self, timeout: Duration) -> Option<(String, Option<String>)> {
        let mut wanted = locked(&self.wanted);
        if !wanted.poked && !wanted.stopped {
            wanted = match self.changed.wait_timeout(wanted, timeout) {
                Ok((guard, _)) => guard,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        wanted.poked = false;
        (!wanted.stopped).then(|| (wanted.branch.clone(), wanted.turn.clone()))
    }
}

type Entries = Arc<Mutex<HashMap<(u64, u32), Arc<Entry>>>>;

/// Every watch of every connection.
pub struct Watches {
    state: StateDir,
    read_every: Mutex<Duration>,
    entries: Entries,
    agents: Mutex<Option<AgentWatcher>>,
}

impl Watches {
    pub fn new(state: StateDir) -> Watches {
        Watches {
            state,
            read_every: Mutex::new(READ_EVERY),
            entries: Arc::new(Mutex::new(HashMap::new())),
            agents: Mutex::new(None),
        }
    }

    pub fn set_read_every(&self, every: Duration) {
        *locked(&self.read_every) = every;
    }

    pub fn count(&self) -> usize {
        locked(&self.entries).len()
    }

    /// Agents report into the state dir; watching it wakes every watch at once instead of a second later.
    fn watch_agents(&self) {
        let mut agents = locked(&self.agents);
        if agents.is_some() {
            return;
        }
        let entries = self.entries.clone();
        let wake = Arc::new(move || {
            let all: Vec<Arc<Entry>> = locked(&entries).values().cloned().collect();
            for entry in all {
                entry.change(|_| {});
            }
        });
        match AgentWatcher::new(&self.state, wake) {
            Ok(watcher) => *agents = Some(watcher),
            Err(error) => eprintln!("pom server: watching agents: {error}"),
        }
    }

    pub fn subscribe(
        &self,
        source: LocalSource,
        caller: &Caller<'_>,
        params: WatchParams,
    ) -> Result<Value, RemoteError> {
        if let Some(unknown) = params
            .topics
            .iter()
            .find(|name| !topic::ALL.contains(&name.as_str()))
        {
            return Err(invalid(format!("no topic {unknown}")));
        }
        check_followed(&params.branch, params.turn.as_deref())?;
        let publisher = caller
            .context
            .connection()
            .publisher(params.channel)
            .map_err(|error| invalid(format!("channel {}: {error}", params.channel)))?;
        let entry = Arc::new(Entry {
            wanted: Mutex::new(Wanted {
                branch: params.branch,
                turn: params.turn,
                ..Wanted::default()
            }),
            changed: Condvar::new(),
        });
        let key = (caller.connection, params.channel);
        locked(&self.entries).insert(key, entry.clone());
        self.watch_agents();
        let topics: Vec<&'static str> = topic::ALL
            .iter()
            .copied()
            .filter(|name| params.topics.iter().any(|wanted| wanted == name))
            .collect();
        let entries = self.entries.clone();
        let every = *locked(&self.read_every);
        let spawned = std::thread::Builder::new()
            .name("pom-server-watch".into())
            .spawn(move || {
                let reason = publish(&entry, &publisher, &source, &topics, every);
                publisher.finish(reason);
                let mut entries = locked(&entries);
                if entries
                    .get(&key)
                    .is_some_and(|known| Arc::ptr_eq(known, &entry))
                {
                    entries.remove(&key);
                }
            });
        if let Err(error) = spawned {
            locked(&self.entries).remove(&key);
            return Err(RemoteError::new(error_code::INTERNAL, error.to_string()));
        }
        Ok(Value::Null)
    }

    fn entry(&self, caller: &Caller<'_>, channel: u32) -> Result<Arc<Entry>, RemoteError> {
        locked(&self.entries)
            .get(&(caller.connection, channel))
            .cloned()
            .ok_or_else(|| invalid(format!("no watch on channel {channel}")))
    }

    pub fn set_branch(
        &self,
        caller: &Caller<'_>,
        params: WatchBranchParams,
    ) -> Result<Value, RemoteError> {
        check_followed(&params.branch, params.turn.as_deref())?;
        self.entry(caller, params.channel)?.change(|wanted| {
            wanted.branch = params.branch;
            wanted.turn = params.turn;
        });
        Ok(Value::Null)
    }

    pub fn unsubscribe(
        &self,
        caller: &Caller<'_>,
        params: WatchChannel,
    ) -> Result<Value, RemoteError> {
        self.entry(caller, params.channel)?
            .change(|wanted| wanted.stopped = true);
        Ok(Value::Null)
    }

    /// A client went away: its watches stop.
    pub fn disconnect(&self, connection: u64) {
        let gone: Vec<Arc<Entry>> = locked(&self.entries)
            .iter()
            .filter(|((owner, _), _)| *owner == connection)
            .map(|(_, entry)| entry.clone())
            .collect();
        for entry in gone {
            entry.change(|wanted| wanted.stopped = true);
        }
    }
}

fn check_followed(branch: &str, turn: Option<&str>) -> Result<(), RemoteError> {
    if !branch.is_empty() {
        check_branch(branch)?;
    }
    if let Some(role) = turn {
        check_name("agent role", role)?;
    }
    Ok(())
}

/// What was last pushed per topic, by hash, so an unchanged value is never sent again.
#[derive(Default)]
struct Sent {
    hashes: HashMap<&'static str, u64>,
}

impl Sent {
    fn publish(
        &mut self,
        publisher: &Publisher,
        name: &'static str,
        value: &impl Serialize,
        hashed: &impl Serialize,
    ) -> Result<(), pom_rpc::Error> {
        let bytes = serde_json::to_vec(hashed).unwrap_or_default();
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        let hash = hasher.finish();
        if self.hashes.get(name) == Some(&hash) {
            return Ok(());
        }
        publisher.publish(name, value)?;
        self.hashes.insert(name, hash);
        Ok(())
    }
}

/// A session's age ticks every second; it alone is no change worth a push.
fn steady(sessions: &[SessionView]) -> Vec<SessionView> {
    sessions
        .iter()
        .map(|view| SessionView {
            last_event_age_s: 0,
            ..view.clone()
        })
        .collect()
}

fn publish(
    entry: &Entry,
    publisher: &Publisher,
    source: &LocalSource,
    topics: &[&'static str],
    every: Duration,
) -> &'static str {
    let wants = |name: &str| topics.contains(&name);
    let mut sent = Sent::default();
    let mut followed: Option<(String, Option<String>)> = None;
    let mut turn_of: Option<SessionView> = None;
    let mut next = {
        let wanted = locked(&entry.wanted);
        (!wanted.stopped).then(|| (wanted.branch.clone(), wanted.turn.clone()))
    };
    while let Some((branch, turn)) = next {
        if publisher.is_ended() {
            return "unsubscribed";
        }
        if followed.as_ref() != Some(&(branch.clone(), turn.clone())) {
            sent.hashes.retain(|name, _| *name == topic::WORKSPACES);
            turn_of = None;
            followed = Some((branch.clone(), turn.clone()));
        }
        let published = read_once(
            &mut sent,
            publisher,
            source,
            &wants,
            &branch,
            turn.as_deref(),
            &mut turn_of,
        );
        if published.is_err() {
            return "unsubscribed";
        }
        next = entry.next(every);
    }
    "unsubscribed"
}

fn read_once(
    sent: &mut Sent,
    publisher: &Publisher,
    source: &LocalSource,
    wants: &dyn Fn(&str) -> bool,
    branch: &str,
    turn: Option<&str>,
    turn_of: &mut Option<SessionView>,
) -> Result<(), pom_rpc::Error> {
    if wants(topic::WORKSPACES) {
        let update = match source.workspaces() {
            Ok(list) => WorkspacesUpdate {
                list: Some(list),
                error: None,
            },
            Err(error) => WorkspacesUpdate {
                list: None,
                error: Some(error.to_string()),
            },
        };
        sent.publish(publisher, topic::WORKSPACES, &update, &update)?;
    }
    if branch.is_empty() {
        return Ok(());
    }
    if wants(topic::STATUS) {
        let mut update = StatusUpdate {
            branch: branch.to_string(),
            ..StatusUpdate::default()
        };
        match source.status(branch) {
            Ok(report) => update.report = Some(report),
            Err(error) => update.error = Some(error.to_string()),
        }
        sent.publish(publisher, topic::STATUS, &update, &update)?;
    }
    if wants(topic::SERVICES) {
        let mut update = ServicesUpdate {
            branch: branch.to_string(),
            ..ServicesUpdate::default()
        };
        match source.services_facts(branch) {
            Ok(facts) => update.facts = Some(facts),
            Err(error) => update.error = Some(error.to_string()),
        }
        sent.publish(publisher, topic::SERVICES, &update, &update)?;
    }
    if !wants(topic::AGENTS) && !wants(topic::TURN) {
        return Ok(());
    }
    let mut update = AgentsUpdate {
        branch: branch.to_string(),
        ..AgentsUpdate::default()
    };
    match source.agents(branch) {
        Ok(sessions) => {
            update.presence = source.agent_presence(&sessions);
            update.sessions = sessions;
        }
        Err(error) => update.error = Some(error.to_string()),
    }
    if wants(topic::AGENTS) {
        let hashed = AgentsUpdate {
            sessions: steady(&update.sessions),
            ..update.clone()
        };
        sent.publish(publisher, topic::AGENTS, &update, &hashed)?;
    }
    let Some(role) = turn.filter(|_| wants(topic::TURN)) else {
        return Ok(());
    };
    let view = update
        .sessions
        .iter()
        .find(|view| view.role == role)
        .map(|view| SessionView {
            last_event_age_s: 0,
            ..view.clone()
        });
    // The transcript is read again only when the session moved on.
    if sent.hashes.contains_key(topic::TURN) && *turn_of == view {
        return Ok(());
    }
    let mut turn_update = TurnUpdate {
        branch: branch.to_string(),
        role: role.to_string(),
        ..TurnUpdate::default()
    };
    match source.last_turn_document(branch, role) {
        Ok(document) => turn_update.document = Some(document),
        Err(error) => turn_update.error = Some(error.to_string()),
    }
    sent.publish(publisher, topic::TURN, &turn_update, &turn_update)?;
    *turn_of = view;
    Ok(())
}
