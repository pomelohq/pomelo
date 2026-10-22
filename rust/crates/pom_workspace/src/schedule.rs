//! Keeping a machine's running workspaces few: idle ones go to sleep after `workspaces.idle_sleep_minutes`, and
//! with `workspaces.max_running` one more starting first puts the least recently used idle one to sleep, or waits
//! as `queued` until a slot frees. A workspace with a working agent is never put to sleep.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pom_config::Config;
use pom_layout::{RunState, SleepRecord, Workspace, WorkspaceState};
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner, ServiceTarget};

use crate::sleep::{Agents, DriveAgents};
use crate::WorkspaceContext;

/// Where the dev proxy keeps each workspace's last request (`pom.proxy_activity/v1`).
pub const ACTIVITY_FILE: &str = "proxy/activity.json";

/// The `workspaces` choices of settings.json that keep running workspaces few.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerSettings {
    /// 0: never.
    pub idle_sleep_minutes: u64,
    /// 0: any number.
    pub max_running: usize,
}

impl PowerSettings {
    pub fn from_settings_file() -> PowerSettings {
        pom_paths::config_dir()
            .map(|dir| dir.join("settings.json"))
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .map_or_else(PowerSettings::default, |value| {
                PowerSettings::from_value(&value)
            })
    }

    pub fn from_value(value: &serde_json::Value) -> PowerSettings {
        let number = |key: &str| value["workspaces"][key].as_u64().unwrap_or_default();
        PowerSettings {
            idle_sleep_minutes: number("idle_sleep_minutes"),
            max_running: number("max_running") as usize,
        }
    }

    pub fn is_off(&self) -> bool {
        self.idle_sleep_minutes == 0 && self.max_running == 0
    }
}

/// One workspace as scheduling sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    pub root: PathBuf,
    pub branch: String,
    pub state: RunState,
    pub queued_ms: Option<u64>,
    /// A service or an agent session of it runs.
    pub up: bool,
    /// An agent of it is in a turn or waits for an approval.
    pub busy: bool,
    /// Its last dev proxy request, agent event or service start.
    pub last_used_ms: u64,
}

impl Seen {
    fn counts(&self) -> bool {
        self.state == RunState::Running && self.up
    }

    fn is(&self, root: &Path, branch: &str) -> bool {
        self.root == root && self.branch == branch
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Sleep(usize),
    Wake(usize),
}

/// What one tick does, by index into `seen`: idle workspaces sleep, then queued ones start into the free slots,
/// longest waiting first.
pub fn plan(seen: &[Seen], settings: &PowerSettings, now_ms: u64) -> Vec<Step> {
    let mut steps = Vec::new();
    if settings.idle_sleep_minutes > 0 {
        let idle_ms = settings.idle_sleep_minutes * 60_000;
        for (index, workspace) in seen.iter().enumerate() {
            if workspace.counts()
                && !workspace.busy
                && now_ms.saturating_sub(workspace.last_used_ms) >= idle_ms
            {
                steps.push(Step::Sleep(index));
            }
        }
    }
    let slept = |index: usize| steps.contains(&Step::Sleep(index));
    let running = seen
        .iter()
        .enumerate()
        .filter(|(index, workspace)| workspace.counts() && !slept(*index))
        .count();
    let mut queued: Vec<usize> = (0..seen.len())
        .filter(|index| seen[*index].state == RunState::Queued)
        .collect();
    queued.sort_by_key(|index| seen[*index].queued_ms.unwrap_or_default());
    let free = match settings.max_running {
        0 => queued.len(),
        max => max.saturating_sub(running),
    };
    steps.extend(queued.into_iter().take(free).map(Step::Wake));
    steps
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Run,
    /// Put this one (an index into `seen`) to sleep first.
    SleepFirst(usize),
    Queue,
}

/// Whether workspace `branch` of `root` may start now under `max_running`.
pub fn admission(seen: &[Seen], root: &Path, branch: &str, settings: &PowerSettings) -> Admission {
    if settings.max_running == 0 {
        return Admission::Run;
    }
    let others: Vec<usize> = (0..seen.len())
        .filter(|index| seen[*index].counts() && !seen[*index].is(root, branch))
        .collect();
    if others.len() < settings.max_running {
        return Admission::Run;
    }
    others
        .into_iter()
        .filter(|index| !seen[*index].busy)
        .min_by_key(|index| seen[*index].last_used_ms)
        .map_or(Admission::Queue, Admission::SleepFirst)
}

/// Every registered project on this machine, opened.
pub struct Machine {
    pub state: StateDir,
    pub holders: SocketDir,
    pub binary: PathBuf,
}

struct Opened {
    root: PathBuf,
    config: Config,
    runner: ServiceRunner,
}

fn modified_ms(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

impl Machine {
    fn open(&self) -> Vec<Opened> {
        pom_sessions::Projects::load(&self.state)
            .projects
            .into_values()
            .filter_map(|root| {
                let root = PathBuf::from(root);
                let config = Config::load(&root.join(pom_config::CONFIG_FILE_NAME)).ok()?;
                let runner = ServiceRunner::new(RunnerOptions {
                    project_root: root.clone(),
                    session: config.session.clone(),
                    state: self.state.clone(),
                    holders: self.holders.clone(),
                    binary: self.binary.clone(),
                    docker: "docker".into(),
                });
                Some(Opened {
                    root,
                    config,
                    runner,
                })
            })
            .collect()
    }

    fn activity(&self) -> HashMap<(PathBuf, String), u64> {
        let Ok(bytes) = std::fs::read(self.state.path(ACTIVITY_FILE)) else {
            return HashMap::new();
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return HashMap::new();
        };
        value["workspaces"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                Some((
                    (
                        PathBuf::from(entry["root"].as_str()?),
                        entry["branch"].as_str()?.to_string(),
                    ),
                    entry["last_request_ms"].as_u64()?,
                ))
            })
            .collect()
    }

    /// Every workspace of every project, with where it lives so a step can act on it.
    fn survey(&self, opened: &[Opened]) -> Vec<(Seen, usize, Workspace)> {
        let activity = self.activity();
        let mut out = Vec::new();
        for (project, open) in opened.iter().enumerate() {
            let context = self.context(open);
            let agents = DriveAgents::new(&context);
            let known: std::collections::HashSet<String> =
                open.config.repos.keys().cloned().collect();
            for workspace in pom_layout::scan(
                &open.root,
                open.config.global_default_branch(),
                Some(&known),
            ) {
                let state = WorkspaceState::load(&workspace.path);
                let sessions = agents.sessions(&workspace.branch);
                let running: Vec<ServiceTarget> = ServiceRunner::service_targets(
                    &open.config,
                    &workspace.branch,
                    workspace.is_main,
                )
                .into_iter()
                .filter(|target| open.runner.is_running(target))
                .collect();
                let started = running
                    .iter()
                    .map(|target| {
                        modified_ms(
                            &open
                                .runner
                                .holders()
                                .pidfile(&open.runner.holder_name(target)),
                        )
                    })
                    .max()
                    .unwrap_or_default();
                let agent_active = sessions
                    .iter()
                    .filter(|session| session.alive)
                    .map(|session| session.active_ms)
                    .max()
                    .unwrap_or_default();
                let request = activity
                    .get(&(open.root.clone(), workspace.branch.clone()))
                    .copied()
                    .unwrap_or_default();
                let seen = Seen {
                    root: open.root.clone(),
                    branch: workspace.branch.clone(),
                    state: state.state,
                    queued_ms: state.queued_ms,
                    up: !running.is_empty() || sessions.iter().any(|session| session.alive),
                    busy: sessions.iter().any(|session| session.alive && session.busy),
                    last_used_ms: started.max(agent_active).max(request),
                };
                out.push((seen, project, workspace));
            }
        }
        out
    }

    fn context<'a>(&'a self, open: &'a Opened) -> WorkspaceContext<'a> {
        WorkspaceContext {
            config: &open.config,
            runner: &open.runner,
            state: &self.state,
        }
    }

    fn sleep_one(&self, open: &Opened, workspace: &Workspace) -> Result<(), String> {
        let context = self.context(open);
        crate::sleep::sleep(&context, &DriveAgents::new(&context), workspace, false)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// One pass: idle workspaces sleep, queued ones start into free slots. Returns what it did, for a log.
    pub fn tick(&self, settings: &PowerSettings, now_ms: u64) -> Vec<String> {
        if settings.is_off() {
            return Vec::new();
        }
        let opened = self.open();
        let surveyed = self.survey(&opened);
        let seen: Vec<Seen> = surveyed.iter().map(|(seen, _, _)| seen.clone()).collect();
        let mut done = Vec::new();
        for step in plan(&seen, settings, now_ms) {
            let (Step::Sleep(index) | Step::Wake(index)) = step;
            let Some((seen, project, workspace)) = surveyed.get(index) else {
                continue;
            };
            let Some(open) = opened.get(*project) else {
                continue;
            };
            let name = format!("{}/{}", open.config.session, seen.branch);
            let result = match step {
                Step::Sleep(_) => self
                    .sleep_one(open, workspace)
                    .map(|()| format!("slept {name} (idle)")),
                Step::Wake(_) => {
                    let context = self.context(open);
                    crate::sleep::wake(&context, &DriveAgents::new(&context), workspace)
                        .map(|_| format!("started {name} (a slot freed)"))
                }
            };
            done.push(result.unwrap_or_else(|error| format!("{name}: {error}")));
        }
        done
    }

    /// Makes room for `workspace` of the project at `root` under `max_running`: sleeps the least recently used idle
    /// workspace, or marks this one queued (keeping `services` to start) when every running one is busy.
    pub fn admit(
        &self,
        root: &Path,
        workspace: &Workspace,
        services: &[String],
        settings: &PowerSettings,
        now_ms: u64,
    ) -> Result<Admitted, String> {
        if settings.max_running == 0 {
            return Ok(Admitted::Run);
        }
        let opened = self.open();
        let surveyed = self.survey(&opened);
        let seen: Vec<Seen> = surveyed.iter().map(|(seen, _, _)| seen.clone()).collect();
        match admission(&seen, root, &workspace.branch, settings) {
            Admission::Run => Ok(Admitted::Run),
            Admission::SleepFirst(index) => {
                let (victim, project, victim_workspace) = &surveyed[index];
                let open = &opened[*project];
                self.sleep_one(open, victim_workspace)?;
                Ok(Admitted::MadeRoom(format!(
                    "{}/{}",
                    open.config.session, victim.branch
                )))
            }
            Admission::Queue => {
                let mut state = WorkspaceState::load(&workspace.path);
                let mut record = state.asleep.take().unwrap_or(SleepRecord {
                    since_ms: now_ms,
                    ..SleepRecord::default()
                });
                for service in services {
                    if !record.services.contains(service) {
                        record.services.push(service.clone());
                    }
                }
                state.asleep = Some(record);
                state.state = RunState::Queued;
                state.queued_ms.get_or_insert(now_ms);
                state
                    .save(&workspace.path)
                    .map_err(|error| format!("save {}: {error}", workspace.path.display()))?;
                let running = seen.iter().filter(|seen| seen.counts()).count();
                Ok(Admitted::Queued { running })
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admitted {
    Run,
    /// This workspace (`project/branch`) was put to sleep to make room.
    MadeRoom(String),
    /// Every slot is taken by a busy workspace; it starts once one frees.
    Queued {
        running: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: u64 = 60_000;

    fn seen(branch: &str, state: RunState, up: bool, busy: bool, last_used_ms: u64) -> Seen {
        Seen {
            root: "/p".into(),
            branch: branch.into(),
            state,
            queued_ms: None,
            up,
            busy,
            last_used_ms,
        }
    }

    #[test]
    fn idle_workspaces_sleep_once_the_clock_passes_their_minutes_but_a_working_one_never_does() {
        let settings = PowerSettings {
            idle_sleep_minutes: 30,
            max_running: 0,
        };
        let workspaces = [
            seen("used", RunState::Running, true, false, 0),
            seen("working", RunState::Running, true, true, 0),
            seen("stopped", RunState::Running, false, false, 0),
            seen("asleep", RunState::Sleeping, false, false, 0),
            seen("fresh", RunState::Running, true, false, 20 * MINUTE),
        ];
        assert!(plan(&workspaces, &settings, 29 * MINUTE).is_empty());
        assert_eq!(plan(&workspaces, &settings, 30 * MINUTE), [Step::Sleep(0)]);
        assert_eq!(
            plan(&workspaces, &settings, 50 * MINUTE),
            [Step::Sleep(0), Step::Sleep(4)]
        );
        let off = PowerSettings::default();
        assert!(plan(&workspaces, &off, 500 * MINUTE).is_empty());
    }

    #[test]
    fn queued_workspaces_start_into_free_slots_longest_waiting_first() {
        let settings = PowerSettings {
            idle_sleep_minutes: 30,
            max_running: 2,
        };
        let mut late = seen("late", RunState::Queued, false, false, 0);
        late.queued_ms = Some(9);
        let mut early = seen("early", RunState::Queued, false, false, 0);
        early.queued_ms = Some(1);
        let workspaces = [
            seen("busy", RunState::Running, true, true, 0),
            seen("idle", RunState::Running, true, false, 0),
            late,
            early,
        ];
        assert!(
            plan(&workspaces, &settings, 10 * MINUTE).is_empty(),
            "both slots taken"
        );
        assert_eq!(
            plan(&workspaces, &settings, 40 * MINUTE),
            [Step::Sleep(1), Step::Wake(3)],
            "the idle one sleeps and the earliest queued takes its slot"
        );
    }

    #[test]
    fn admission_sleeps_the_least_recently_used_idle_workspace_or_queues() {
        let settings = PowerSettings {
            idle_sleep_minutes: 0,
            max_running: 2,
        };
        let workspaces = [
            seen("a", RunState::Running, true, false, 5),
            seen("b", RunState::Running, true, false, 3),
            seen("c", RunState::Sleeping, false, false, 0),
        ];
        assert_eq!(
            admission(&workspaces, Path::new("/p"), "c", &settings),
            Admission::SleepFirst(1)
        );
        assert_eq!(
            admission(&workspaces, Path::new("/p"), "a", &settings),
            Admission::Run,
            "a running workspace keeps its own slot"
        );
        let busy = [
            seen("a", RunState::Running, true, true, 5),
            seen("b", RunState::Running, true, true, 3),
        ];
        assert_eq!(
            admission(&busy, Path::new("/p"), "c", &settings),
            Admission::Queue
        );
        assert_eq!(
            admission(&busy, Path::new("/p"), "c", &PowerSettings::default()),
            Admission::Run
        );
    }
}

impl Machine {
    /// The machine a project's context runs on.
    pub fn of(context: &WorkspaceContext<'_>) -> Machine {
        Machine {
            state: context.state.clone(),
            holders: context.runner.holders().clone(),
            binary: context.runner.binary().to_path_buf(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WakeOutcome {
    Woke {
        report: crate::sleep::WakeReport,
        /// The workspace put to sleep to make room, as `project/branch`.
        made_room: Option<String>,
    },
    Queued {
        running: usize,
    },
}

/// `wake`, after making room for it under `max_running`; it queues instead when every slot is busy.
pub fn wake_with_room(
    context: &WorkspaceContext<'_>,
    agents: &dyn Agents,
    workspace: &Workspace,
    settings: &PowerSettings,
) -> Result<WakeOutcome, String> {
    let state = WorkspaceState::load(&workspace.path);
    let mut made_room = None;
    if state.state != RunState::Running {
        let services = state
            .asleep
            .map(|record| record.services)
            .unwrap_or_default();
        match Machine::of(context).admit(
            context.runner.project_root(),
            workspace,
            &services,
            settings,
            crate::sleep::now_ms(),
        )? {
            Admitted::Run => {}
            Admitted::MadeRoom(name) => made_room = Some(name),
            Admitted::Queued { running } => return Ok(WakeOutcome::Queued { running }),
        }
    }
    let report = crate::sleep::wake(context, agents, workspace)?;
    Ok(WakeOutcome::Woke { report, made_room })
}

/// How often idle and queued workspaces are looked at.
pub const TICK_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// Ticks on a thread of its own for as long as the process lives, reading the settings each time so a change
/// needs no restart.
pub fn run_in_background(machine: Machine, log: fn(&str)) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("pom-workspace-schedule".into())
        .spawn(move || loop {
            std::thread::sleep(TICK_EVERY);
            let settings = PowerSettings::from_settings_file();
            for line in machine.tick(&settings, crate::sleep::now_ms()) {
                log(&line);
            }
        })
        .map(|_| ())
}
