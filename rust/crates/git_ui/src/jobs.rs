use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use git::access::GitAccess;

use crate::GitPanel;

/// How often the panel repaints while git work is under way, so its spinners turn.
const SPIN_FRAME: Duration = Duration::from_millis(50);

/// What a finished job does to the panel, on the UI thread.
pub(crate) type Apply = Box<dyn FnOnce(&mut GitPanel) + Send>;
type Work = Box<dyn FnOnce(&dyn GitAccess) -> Apply + Send>;
type Done = Arc<Mutex<Vec<(u64, Apply)>>>;

/// The part of the panel a running job is about, which shows a spinner meanwhile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    Repo(usize),
    /// A file or folder of a repo.
    Path(usize, String),
}

/// Git work off the UI thread. Changes run one at a time in the order asked, so two of them never race on a
/// repo; reads run alongside. Each result applies on the UI thread in the order it finished.
pub(crate) struct Jobs {
    pub access: Arc<dyn GitAccess>,
    waker: Arc<dyn Fn() + Send + Sync>,
    changes: Option<Sender<(u64, Work)>>,
    done: Done,
    outstanding: Arc<AtomicUsize>,
    /// Each job not applied yet: its ticket, what it is about, and whether it changes repos.
    running: Vec<(u64, Vec<Mark>, bool)>,
    next: u64,
    started: Instant,
}

impl Jobs {
    pub fn new(access: Arc<dyn GitAccess>, waker: Arc<dyn Fn() + Send + Sync>) -> Jobs {
        Jobs {
            access,
            waker,
            changes: None,
            done: Arc::new(Mutex::new(Vec::new())),
            outstanding: Arc::new(AtomicUsize::new(0)),
            running: Vec::new(),
            next: 1,
            started: Instant::now(),
        }
    }

    /// A change to repos: runs after every change asked before it.
    pub fn change(
        &mut self,
        marks: Vec<Mark>,
        work: impl FnOnce(&dyn GitAccess) -> Apply + Send + 'static,
    ) {
        let ticket = self.ticket(marks, true);
        let sent = self
            .worker()
            .is_some_and(|changes| changes.send((ticket, Box::new(work))).is_ok());
        if !sent {
            self.changes = None;
            self.fail(ticket);
        }
    }

    /// A read that changes nothing, so it need not wait for the changes ahead of it.
    pub fn read(
        &mut self,
        marks: Vec<Mark>,
        work: impl FnOnce(&dyn GitAccess) -> Apply + Send + 'static,
    ) {
        let ticket = self.ticket(marks, false);
        let (access, done) = (self.access.clone(), self.done.clone());
        let (outstanding, waker) = (self.outstanding.clone(), self.waker.clone());
        let spawned = std::thread::Builder::new()
            .name("git-panel-read".into())
            .spawn(move || finish(ticket, work(access.as_ref()), &done, &outstanding, &waker));
        if let Err(error) = spawned {
            eprintln!("git panel: read thread: {error}");
            self.fail(ticket);
        }
    }

    /// The results that came in since last asked, oldest first.
    pub fn take_done(&mut self) -> Vec<Apply> {
        let finished = match self.done.lock() {
            Ok(mut done) => std::mem::take(&mut *done),
            Err(_) => Vec::new(),
        };
        finished
            .into_iter()
            .map(|(ticket, apply)| {
                self.running.retain(|(running, _, _)| *running != ticket);
                apply
            })
            .collect()
    }

    pub fn idle(&self) -> bool {
        self.running.is_empty()
    }

    pub fn changing(&self) -> bool {
        self.running.iter().any(|(_, _, change)| *change)
    }

    /// Whether every job asked so far has finished (results may still wait in `take_done`).
    pub fn finished(&self) -> bool {
        self.outstanding.load(Ordering::Acquire) == 0
    }

    pub fn repo_busy(&self, repo: usize) -> bool {
        self.marks().any(|mark| *mark == Mark::Repo(repo))
    }

    /// A job runs on this path, on a folder holding it, or on its whole repo.
    pub fn path_busy(&self, repo: usize, path: &str) -> bool {
        self.marks().any(|mark| match mark {
            Mark::Repo(busy) => *busy == repo,
            Mark::Path(busy, busy_path) => {
                *busy == repo
                    && (busy_path == path
                        || path
                            .strip_prefix(busy_path.as_str())
                            .is_some_and(|rest| rest.starts_with('/')))
            }
        })
    }

    /// The clock spinners turn by.
    pub fn seconds(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    fn marks(&self) -> impl Iterator<Item = &Mark> {
        self.running.iter().flat_map(|(_, marks, _)| marks.iter())
    }

    fn ticket(&mut self, marks: Vec<Mark>, change: bool) -> u64 {
        let ticket = self.next;
        self.next += 1;
        self.running.push((ticket, marks, change));
        if self.outstanding.fetch_add(1, Ordering::AcqRel) == 0 {
            self.spin();
        }
        ticket
    }

    fn fail(&mut self, ticket: u64) {
        self.running.retain(|(running, _, _)| *running != ticket);
        self.outstanding.fetch_sub(1, Ordering::AcqRel);
    }

    fn worker(&mut self) -> Option<&Sender<(u64, Work)>> {
        if self.changes.is_none() {
            let (sender, receiver) = mpsc::channel::<(u64, Work)>();
            let (access, done) = (self.access.clone(), self.done.clone());
            let (outstanding, waker) = (self.outstanding.clone(), self.waker.clone());
            let spawned = std::thread::Builder::new()
                .name("git-panel-change".into())
                .spawn(move || {
                    for (ticket, work) in receiver {
                        finish(ticket, work(access.as_ref()), &done, &outstanding, &waker);
                    }
                });
            match spawned {
                Ok(_) => self.changes = Some(sender),
                Err(error) => eprintln!("git panel: change thread: {error}"),
            }
        }
        self.changes.as_ref()
    }

    fn spin(&self) {
        let (outstanding, waker) = (self.outstanding.clone(), self.waker.clone());
        let spawned = std::thread::Builder::new()
            .name("git-panel-spin".into())
            .spawn(move || {
                while outstanding.load(Ordering::Acquire) > 0 {
                    std::thread::sleep(SPIN_FRAME);
                    waker();
                }
            });
        if let Err(error) = spawned {
            eprintln!("git panel: spinner thread: {error}");
        }
    }
}

fn finish(
    ticket: u64,
    apply: Apply,
    done: &Mutex<Vec<(u64, Apply)>>,
    outstanding: &AtomicUsize,
    waker: &Arc<dyn Fn() + Send + Sync>,
) {
    match done.lock() {
        Ok(mut done) => done.push((ticket, apply)),
        Err(_) => eprintln!("git panel: a finished job had nowhere to go"),
    }
    outstanding.fetch_sub(1, Ordering::AcqRel);
    waker();
}
