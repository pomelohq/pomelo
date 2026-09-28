//! What the onboarding page shows, shared between the page (a tab, recreated when the window switches
//! to the new project) and the app, which runs the setup and feeds its progress in.

use std::path::{Path, PathBuf};
use std::time::Instant;

use onboarding::{Check, CheckKind, CheckStatus, Counts, Segment};
use pom_agent::AgentCli;
use pom_core::RepoScan;
use workspace::text_field::TextField;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    NewProject,
    Progress,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Repositories,
    Setup,
    Review,
}

impl Step {
    pub const ALL: [Step; 3] = [Step::Repositories, Step::Setup, Step::Review];

    pub fn title(self) -> &'static str {
        match self {
            Step::Repositories => "Repositories",
            Step::Setup => "Setup",
            Step::Review => "Review",
        }
    }

    pub fn number(self) -> usize {
        Step::ALL.iter().position(|step| *step == self).unwrap_or(0) + 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddMode {
    Folders,
    Urls,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Branch,
    Url,
    Alias(usize),
    WorkspaceBranch,
}

pub struct RepoRow {
    /// A local folder or a git URL.
    pub source: String,
    pub alias: TextField,
    /// What detection found; `None` for a URL, which is only scanned once cloned.
    pub scan: Option<RepoScan>,
}

impl RepoRow {
    pub fn remote(&self) -> bool {
        !Path::new(&self.source).is_absolute()
    }
}

/// What the three steps collect.
pub struct Form {
    pub step: Step,
    pub name: TextField,
    pub branch: TextField,
    pub url: TextField,
    pub add_mode: AddMode,
    pub repos: Vec<RepoRow>,
    pub with_agent: bool,
    pub agent: AgentCli,
    pub import_secrets: bool,
    pub start_shared: bool,
    pub first_workspace: bool,
    pub workspace_branch: TextField,
    pub focus: Option<Field>,
}

fn field(text: &str) -> TextField {
    let mut field = TextField::default();
    field.set_text(text);
    field.move_to_end();
    field
}

impl Form {
    pub fn new() -> Form {
        Form {
            step: Step::Repositories,
            name: TextField::default(),
            branch: field("main"),
            url: TextField::default(),
            add_mode: AddMode::Folders,
            repos: Vec::new(),
            with_agent: true,
            agent: AgentCli::Claude,
            import_secrets: true,
            start_shared: true,
            first_workspace: true,
            workspace_branch: field("feat-first"),
            focus: Some(Field::Name),
        }
    }

    pub fn name(&self) -> String {
        self.name.text().trim().to_string()
    }

    pub fn default_branch(&self) -> String {
        match self.branch.text().trim() {
            "" => "main".to_string(),
            branch => branch.to_string(),
        }
    }

    pub fn workspace_branch(&self) -> String {
        self.workspace_branch.text().trim().to_string()
    }

    pub fn env_files(&self) -> usize {
        self.repos
            .iter()
            .filter_map(|repo| repo.scan.as_ref())
            .map(|scan| scan.env_files)
            .sum()
    }

    /// Shared services found in the added repos' compose files, each once.
    pub fn infra(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for name in self
            .repos
            .iter()
            .filter_map(|repo| repo.scan.as_ref())
            .flat_map(|scan| scan.infra.iter())
        {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    pub fn alias_of(&self, repo: &RepoRow) -> String {
        match repo.alias.text().trim() {
            "" => repo
                .scan
                .as_ref()
                .map(|scan| scan.name.clone())
                .unwrap_or_else(|| repo_name(&repo.source)),
            alias => alias.to_string(),
        }
    }
}

impl Default for Form {
    fn default() -> Form {
        Form::new()
    }
}

/// The last path segment of a folder or URL, without `.git`.
pub fn repo_name(source: &str) -> String {
    let trimmed = source.trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    trimmed
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(trimmed)
        .to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Clone,
    Scan,
    Configure,
    Verify,
    Repair,
}

impl Phase {
    pub const ALL: [Phase; 5] = [
        Phase::Clone,
        Phase::Scan,
        Phase::Configure,
        Phase::Verify,
        Phase::Repair,
    ];

    pub fn index(self) -> usize {
        Phase::ALL
            .iter()
            .position(|phase| *phase == self)
            .unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CloneProgress {
    pub percent: u8,
    /// Set once done: `true` for a local repo cloned with its work, `false` for a fetched URL.
    pub linked: Option<bool>,
}

/// A failed check waiting for a repair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub check: Check,
    /// The agent was sent it and is working on it.
    pub fixing: bool,
    /// The ways to fix it by hand are showing.
    pub manual: bool,
}

/// A running (or finished) setup.
pub struct Run {
    pub name: String,
    pub root: PathBuf,
    pub repos: Vec<String>,
    pub started: Instant,
    pub finished: Option<Instant>,
    pub phase: Phase,
    /// When each phase began and last ended, by `Phase::index`.
    pub phase_at: [Option<Instant>; 5],
    pub phase_end: [Option<Instant>; 5],
    pub clone: Vec<CloneProgress>,
    pub scans: Vec<RepoScan>,
    pub with_agent: bool,
    pub agent: AgentCli,
    /// The agent was stopped; the drafted config is finished by hand.
    pub agent_skipped: bool,
    /// What the config sets up so far, refreshed as the agent writes it.
    pub summary: Vec<Vec<Segment>>,
    pub checks: Vec<Check>,
    pub finding: Option<Finding>,
    /// A finding was repaired (or skipped) during this run.
    pub repaired: bool,
    pub paused: bool,
    /// Clone or scan failed; nothing was created.
    pub error: Option<String>,
    pub counts: Counts,
    pub import_secrets: bool,
    pub start_shared: bool,
    pub first_workspace: Option<String>,
    /// The agent dock is showing the agent's CLI.
    pub agent_shown: bool,
    /// "What was set up": each repo's alias, its stack and the services it runs.
    pub set_up: Vec<(String, Vec<String>, String)>,
    pub secrets_imported: usize,
    /// Services the user chose to leave out of the boot checks: (repo, service).
    pub skipped: Vec<(String, String)>,
}

impl Run {
    pub fn new(form: &Form, root: PathBuf) -> Run {
        let now = Instant::now();
        let mut phase_at = [None; 5];
        phase_at[0] = Some(now);
        Run {
            name: form.name(),
            root,
            repos: form.repos.iter().map(|repo| form.alias_of(repo)).collect(),
            started: now,
            finished: None,
            phase: Phase::Clone,
            phase_at,
            phase_end: [None; 5],
            clone: vec![CloneProgress::default(); form.repos.len()],
            scans: Vec::new(),
            with_agent: form.with_agent,
            agent: form.agent,
            agent_skipped: false,
            summary: Vec::new(),
            checks: Vec::new(),
            finding: None,
            repaired: false,
            paused: false,
            error: None,
            counts: Counts::default(),
            import_secrets: form.import_secrets,
            start_shared: form.start_shared,
            first_workspace: form
                .first_workspace
                .then(|| form.workspace_branch())
                .filter(|branch| !branch.is_empty()),
            agent_shown: false,
            set_up: Vec::new(),
            secrets_imported: 0,
            skipped: Vec::new(),
        }
    }

    pub fn enter(&mut self, next: Phase) {
        let now = Instant::now();
        self.phase_end[self.phase.index()] = Some(now);
        self.phase_at[next.index()].get_or_insert(now);
        self.phase_end[next.index()] = None;
        self.phase = next;
    }

    /// How long a phase took, or has taken so far.
    pub fn phase_seconds(&self, phase: Phase) -> Option<u64> {
        let start = self.phase_at[phase.index()]?;
        let end = self.phase_end[phase.index()].unwrap_or_else(Instant::now);
        Some(end.saturating_duration_since(start).as_secs())
    }

    /// The agent is doing the configuring right now.
    pub fn agent_configuring(&self) -> bool {
        self.phase == Phase::Configure && self.with_agent && !self.agent_skipped
    }

    pub fn agent_active(&self) -> bool {
        self.with_agent && !self.agent_skipped
    }

    /// A check came in: a running one replaces the row of the same check, a failure opens a finding.
    pub fn record(&mut self, check: Check) {
        match self
            .checks
            .iter_mut()
            .find(|known| known.kind == check.kind)
        {
            Some(known) => *known = check.clone(),
            None => self.checks.push(check.clone()),
        }
        if check.status == CheckStatus::Failed {
            self.finding = Some(Finding {
                check,
                fixing: false,
                manual: false,
            });
            self.enter(Phase::Repair);
        }
    }

    /// Starting verify again keeps what passed; only the failed check and what follows run again.
    pub fn restart_verify(&mut self) {
        if let Some(finding) = self.finding.take() {
            self.checks.retain(|check| check.kind != finding.check.kind);
            self.repaired = true;
        }
        self.enter(Phase::Verify);
    }

    /// Leaves the failed service out of the boot checks and verifies again.
    pub fn restart_verify_skipping(&mut self) {
        if let Some(Finding {
            check:
                Check {
                    kind: CheckKind::Boot { repo, service },
                    ..
                },
            ..
        }) = self.finding.as_ref()
        {
            self.skipped.push((repo.clone(), service.clone()));
        }
        self.restart_verify();
    }

    pub fn progress(&self) -> f32 {
        if self.finished.is_some() {
            return 1.0;
        }
        match self.phase {
            Phase::Clone => {
                let total = self.clone.len().max(1) as f32;
                let done: f32 = self
                    .clone
                    .iter()
                    .map(|clone| f32::from(clone.percent) / 100.0)
                    .sum();
                0.25 * done / total
            }
            Phase::Scan => 0.25,
            Phase::Configure => 0.5,
            Phase::Verify | Phase::Repair => 0.75,
        }
    }
}

/// Something the page asks the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    ChooseFolders,
    /// Create the project from the form.
    Create,
    /// Close the page (cancel on the form, or leave the finished page).
    Close,
    /// The page's tab was closed.
    TabClosed,
    /// Stop the setup and remove what it created.
    Cancel,
    TogglePause,
    ToggleAgentCli,
    OpenConfig,
    SkipAgent,
    ResumeAgent,
    /// The agent finished configuring (for a CLI that cannot say so itself).
    ConfigureDone,
    FixWithAgent,
    CancelFix,
    /// A terminal in the repo the finding is about.
    OpenTerminal {
        repo: String,
    },
    SkipService,
    RetryVerify,
    OpenWorkspace,
    StartMain,
}

/// The whole page's state.
pub struct Onboarding {
    pub screen: Screen,
    pub form: Form,
    pub run: Option<Run>,
    pub sessions_root: PathBuf,
    /// Each agent CLI with whether it is installed.
    pub agents: Vec<(AgentCli, bool)>,
    pub requests: Vec<Request>,
    /// Bumped on every change the app makes, so the page knows to repaint.
    pub version: u64,
}

impl Onboarding {
    pub fn new(sessions_root: PathBuf, agents: Vec<(AgentCli, bool)>) -> Onboarding {
        let mut form = Form::new();
        let installed = |cli: &AgentCli| {
            agents
                .iter()
                .any(|(known, installed)| known == cli && *installed)
        };
        match agents.iter().find(|(cli, _)| installed(cli)) {
            Some((cli, _)) => form.agent = *cli,
            None => form.with_agent = false,
        }
        Onboarding {
            screen: Screen::NewProject,
            form,
            run: None,
            sessions_root,
            agents,
            requests: Vec::new(),
            version: 0,
        }
    }

    pub fn agent_installed(&self, cli: AgentCli) -> bool {
        self.agents
            .iter()
            .any(|(known, installed)| *known == cli && *installed)
    }

    pub fn any_agent(&self) -> bool {
        self.agents.iter().any(|(_, installed)| *installed)
    }

    pub fn changed(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// Adds local folders: a folder that is a git repo, or each git repo directly inside one.
    pub fn add_folders(&mut self, folders: &[PathBuf]) -> usize {
        let mut added = 0;
        for folder in folders {
            let repos: Vec<PathBuf> = if pom_core::preview_repo(folder).is_some() {
                vec![folder.clone()]
            } else {
                let mut inside: Vec<PathBuf> = std::fs::read_dir(folder)
                    .map(|entries| {
                        entries
                            .filter_map(Result::ok)
                            .map(|entry| entry.path())
                            .filter(|path| path.join(".git").exists())
                            .collect()
                    })
                    .unwrap_or_default();
                inside.sort();
                inside
            };
            for repo in repos {
                if self.add_repo(repo.to_string_lossy().into_owned()) {
                    added += 1;
                }
            }
        }
        self.changed();
        added
    }

    /// Adds a repo by folder or URL; false when it is already there or not a repo.
    pub fn add_repo(&mut self, source: String) -> bool {
        let source = source.trim().to_string();
        if source.is_empty() || self.form.repos.iter().any(|repo| repo.source == source) {
            return false;
        }
        let scan = if Path::new(&source).is_absolute() {
            match pom_core::preview_repo(Path::new(&source)) {
                Some(scan) => Some(scan),
                None => return false,
            }
        } else {
            None
        };
        self.form.repos.push(RepoRow {
            source,
            alias: TextField::default(),
            scan,
        });
        true
    }

    /// Adds every URL in the URL field (one per line or separated by spaces) and empties it.
    pub fn add_urls(&mut self) -> usize {
        let text = self.form.url.text();
        let added = text
            .split_whitespace()
            .filter(|url| self.add_repo(url.to_string()))
            .count();
        self.form.url.set_text("");
        added
    }

    pub fn name_error(&self) -> Option<String> {
        let name = self.form.name();
        if name.is_empty() {
            return None;
        }
        if name.contains(['/', '\\']) || name.contains("..") || name.starts_with('.') {
            return Some("no slashes, '..' or leading dot".into());
        }
        let path = self.sessions_root.join(&name);
        path.exists()
            .then(|| format!("{} already exists", home_relative(&path)))
    }

    pub fn branch_error(&self) -> Option<String> {
        let branch = self.form.branch.text().trim().to_string();
        if branch.is_empty() {
            return None;
        }
        pom_workspace::validate_branch_name(&branch).err()
    }

    pub fn workspace_branch_error(&self) -> Option<String> {
        if !self.form.first_workspace {
            return None;
        }
        let branch = self.form.workspace_branch();
        if branch.is_empty() {
            return Some("name the branch".into());
        }
        if branch == self.form.default_branch() {
            return Some("that is main's branch".into());
        }
        pom_workspace::validate_branch_name(&branch).err()
    }

    pub fn repos_ready(&self) -> bool {
        !self.form.name().is_empty()
            && self.name_error().is_none()
            && self.branch_error().is_none()
            && !self.form.repos.is_empty()
    }

    pub fn can_create(&self) -> bool {
        self.repos_ready()
            && self.workspace_branch_error().is_none()
            && (!self.form.with_agent || self.agent_installed(self.form.agent))
    }

    pub fn start_run(&mut self) {
        let root = self.sessions_root.join(self.form.name());
        self.run = Some(Run::new(&self.form, root));
        self.screen = Screen::Progress;
        self.changed();
    }

    /// Back to the form, as it was, after a stopped or failed setup.
    pub fn back_to_form(&mut self) {
        self.screen = Screen::NewProject;
        self.form.step = Step::Review;
        self.changed();
    }

    pub fn finish(&mut self) {
        if let Some(run) = self.run.as_mut() {
            let now = Instant::now();
            run.phase_end[run.phase.index()] = Some(now);
            run.finished = Some(now);
        }
        self.screen = Screen::Done;
        self.changed();
    }
}

/// `~/...` for a path under the home folder.
pub fn home_relative(path: &Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&format!("{home}/")) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}
