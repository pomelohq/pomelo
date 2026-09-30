use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

const REPO: &str = match option_env!("POMELO_UPDATE_REPO") {
    Some(repo) => repo,
    None => "pomelohq/pomelo",
};
const TAG_PREFIX: &str = "v";
const POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);
pub const UP_TO_DATE_SHOWN: Duration = Duration::from_secs(3);

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    /// The newest release is the one running.
    UpToDate,
    Downloading {
        version: String,
        progress: Option<f32>,
    },
    Verifying {
        version: String,
    },
    Ready {
        version: String,
    },
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckKind {
    Automatic,
    Manual,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub status: Status,
    pub manual: bool,
    pub dismissed: bool,
    pub supported: bool,
    pub last_checked: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq)]
struct Staged {
    version: String,
    app: PathBuf,
}

#[derive(Debug)]
struct State {
    status: Status,
    kind: CheckKind,
    running: bool,
    dismissed: Option<Status>,
    staged: Option<Staged>,
    last_checked: Option<SystemTime>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            status: Status::Idle,
            kind: CheckKind::Automatic,
            running: false,
            dismissed: None,
            staged: None,
            last_checked: None,
        }
    }
}

impl State {
    fn at_rest(&self) -> Status {
        match &self.staged {
            Some(staged) => Status::Ready {
                version: staged.version.clone(),
            },
            None => Status::Idle,
        }
    }

    fn begin(&mut self, kind: CheckKind) -> bool {
        if kind == CheckKind::Manual {
            self.dismissed = None;
        }
        if self.running {
            if self.kind == CheckKind::Automatic {
                self.kind = kind;
                if kind == CheckKind::Manual && self.status == self.at_rest() {
                    self.status = Status::Checking;
                }
            }
            return false;
        }
        self.running = true;
        self.kind = kind;
        if !(kind == CheckKind::Automatic && self.staged.is_some()) {
            self.status = Status::Checking;
        }
        true
    }

    fn found_nothing(&mut self, now: SystemTime) {
        self.running = false;
        self.last_checked = Some(now);
        self.status = match (&self.staged, self.kind) {
            (None, CheckKind::Manual) => Status::UpToDate,
            _ => self.at_rest(),
        };
    }

    fn downloading(&mut self, version: &str, progress: Option<f32>) {
        self.staged = None;
        self.status = Status::Downloading {
            version: version.to_string(),
            progress,
        };
    }

    fn verifying(&mut self, version: &str) {
        self.status = Status::Verifying {
            version: version.to_string(),
        };
    }

    fn staged(&mut self, staged: Staged, now: SystemTime) {
        self.running = false;
        self.last_checked = Some(now);
        self.status = Status::Ready {
            version: staged.version.clone(),
        };
        self.staged = Some(staged);
    }

    fn failed(&mut self, error: String) {
        self.running = false;
        self.status = match self.kind {
            CheckKind::Manual => Status::Failed(error),
            CheckKind::Automatic => self.at_rest(),
        };
    }

    fn dismiss(&mut self) {
        match &self.status {
            Status::Ready { .. } => self.dismissed = Some(self.status.clone()),
            Status::Failed(_) | Status::UpToDate => self.status = self.at_rest(),
            _ => {}
        }
    }

    fn expire_up_to_date(&mut self) {
        if self.status == Status::UpToDate {
            self.status = self.at_rest();
        }
    }

    fn snapshot(&self, supported: bool) -> Snapshot {
        Snapshot {
            status: self.status.clone(),
            manual: self.kind == CheckKind::Manual,
            dismissed: self.dismissed.as_ref() == Some(&self.status),
            supported,
            last_checked: self.last_checked,
        }
    }
}

fn state() -> MutexGuard<'static, State> {
    static CELL: OnceLock<Mutex<State>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(State::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

type Waker = Box<dyn Fn() + Send + Sync>;

fn waker() -> &'static OnceLock<Waker> {
    static WAKER: OnceLock<Waker> = OnceLock::new();
    &WAKER
}

pub fn set_waker(wake: impl Fn() + Send + Sync + 'static) {
    if waker().set(Box::new(wake)).is_err() {
        eprintln!("[update] waker already installed");
    }
}

fn update<R>(change: impl FnOnce(&mut State) -> R) -> R {
    let result = change(&mut state());
    if let Some(wake) = waker().get() {
        wake();
    }
    result
}

pub fn snapshot() -> Snapshot {
    state().snapshot(supported())
}

pub fn supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        std::env::var("POMELO_AUTO_UPDATE").as_deref() != Ok("0") && production_app_root().is_some()
    })
}

fn production_app_root() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let app = exe.ancestors().nth(3)?; // pomelo -> MacOS -> Contents -> Pomelo.app
    (app.file_name()?.to_str()? == "Pomelo.app").then(|| app.to_path_buf())
}

pub fn check(kind: CheckKind) {
    let Some(app_root) = production_app_root() else {
        return;
    };
    if !supported() || !update(|state| state.begin(kind)) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || match check_and_stage(&app_root) {
            Ok(()) => {
                if snapshot().status == Status::UpToDate {
                    std::thread::sleep(UP_TO_DATE_SHOWN);
                    update(State::expire_up_to_date);
                }
            }
            Err(error) => {
                eprintln!("[update] {error}");
                update(|state| state.failed(error));
            }
        });
    if let Err(error) = spawned {
        update(|state| state.failed(format!("could not start the check: {error}")));
    }
}

pub fn set_automatic_checks(enabled: bool) {
    static AUTOMATIC: AtomicBool = AtomicBool::new(false);
    static POLLING: OnceLock<()> = OnceLock::new();
    if !supported() || AUTOMATIC.swap(enabled, Ordering::SeqCst) == enabled || !enabled {
        return;
    }
    POLLING.get_or_init(|| {
        let spawned = std::thread::Builder::new()
            .name("update-poll".into())
            .spawn(|| loop {
                std::thread::sleep(POLL_INTERVAL);
                if AUTOMATIC.load(Ordering::SeqCst) {
                    check(CheckKind::Automatic);
                }
            });
        if let Err(error) = spawned {
            eprintln!("[update] could not start the hourly check: {error}");
        }
    });
    check(CheckKind::Automatic);
}

pub fn dismiss() {
    update(State::dismiss);
}

pub fn restart() -> Result<std::convert::Infallible, String> {
    let result = install_staged().and_then(|app_root| {
        relaunch(&app_root)?;
        std::process::exit(0)
    });
    if let Err(error) = &result {
        update(|state| state.status = Status::Failed(error.clone()));
    }
    result
}

pub fn install_on_quit() {
    if state().staged.is_none() {
        return;
    }
    if let Err(error) = install_staged() {
        eprintln!("[update] install on quit: {error}");
    }
}

pub fn take_just_updated() -> Option<String> {
    let marker = marker_path();
    let text = std::fs::read_to_string(&marker).ok()?;
    if let Err(error) = std::fs::remove_file(&marker) {
        eprintln!("[update] remove {}: {error}", marker.display());
    }
    let to = text.lines().nth(1)?.trim().to_string();
    (to == current_version()).then_some(to)
}

pub fn release_url(version: &str) -> String {
    format!("https://github.com/{REPO}/releases/tag/{TAG_PREFIX}{version}")
}

pub fn releases_url() -> String {
    format!("https://github.com/{REPO}/releases")
}

pub fn release_notes(version: &str) -> Result<String, String> {
    if parse_version(version).is_none() {
        return Err(format!("not a release version: {version}"));
    }
    let body = curl(&format!(
        "https://api.github.com/repos/{REPO}/releases/tags/{TAG_PREFIX}{version}"
    ))?;
    let release: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("parse release: {e}"))?;
    Ok(release
        .get("body")
        .and_then(|body| body.as_str())
        .unwrap_or_default()
        .to_string())
}

/// The release signing key's public half: the same EdDSA key the previous app's updater trusts.
pub const UPDATE_PUBLIC_KEY: &str = "tGnmpupAySzHVMfQcDqtlMFoxuSLC9Pl6TtF4DmGECY=";

/// Checks `bytes` against a base64 Ed25519 `signature` made with the key whose public half is `public_key`.
pub fn verify(bytes: &[u8], signature: &str, public_key: &str) -> Result<(), String> {
    use base64::Engine;
    let engine = base64::engine::general_purpose::STANDARD;
    let key: [u8; 32] = engine
        .decode(public_key)
        .ok()
        .and_then(|raw| raw.try_into().ok())
        .ok_or("bad update public key")?;
    let signature: [u8; 64] = engine
        .decode(signature)
        .ok()
        .and_then(|raw| raw.try_into().ok())
        .ok_or("the update's signature is malformed")?;
    let key =
        ed25519_dalek::VerifyingKey::from_bytes(&key).map_err(|e| format!("update key: {e}"))?;
    key.verify_strict(bytes, &ed25519_dalek::Signature::from_bytes(&signature))
        .map_err(|_| "the download's signature did not verify".to_string())
}

fn parse_version(text: &str) -> Option<Vec<u32>> {
    let parts: Option<Vec<u32>> = text.split('.').map(|part| part.parse().ok()).collect();
    parts.filter(|parts| parts.len() == 3)
}

fn join_version(version: &[u32]) -> String {
    version
        .iter()
        .map(|part| part.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

fn needs_download(latest: &[u32], current: &[u32], staged: Option<&[u32]>) -> bool {
    latest > current && staged.is_none_or(|staged| latest > staged)
}

struct Release {
    version: Vec<u32>,
    tarball: String,
    signature: String,
    size: Option<u64>,
}

fn asset<'a>(release: &'a serde_json::Value, suffix: &str) -> Option<&'a serde_json::Value> {
    release.get("assets")?.as_array()?.iter().find(|asset| {
        asset
            .get("name")
            .and_then(|name| name.as_str())
            .is_some_and(|name| name.ends_with(suffix))
    })
}

fn download_url(asset: &serde_json::Value) -> Option<String> {
    asset
        .get("browser_download_url")
        .and_then(|url| url.as_str())
        .map(String::from)
}

fn newest_release(releases: &serde_json::Value) -> Option<Release> {
    let mut best: Option<Release> = None;
    for release in releases.as_array()? {
        let tag = release
            .get("tag_name")
            .and_then(|tag| tag.as_str())
            .unwrap_or_default();
        let Some(version) = tag.strip_prefix(TAG_PREFIX).and_then(parse_version) else {
            continue;
        };
        let Some(tarball) = asset(release, ".app.tar.gz") else {
            continue;
        };
        let (Some(tarball_url), Some(signature)) = (
            download_url(tarball),
            asset(release, ".app.tar.gz.sig").and_then(download_url),
        ) else {
            continue;
        };
        if best.as_ref().is_none_or(|best| version > best.version) {
            best = Some(Release {
                version,
                tarball: tarball_url,
                signature,
                size: tarball.get("size").and_then(|size| size.as_u64()),
            });
        }
    }
    best
}

fn cache_root() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join("Library/Caches/Pomelo"),
        None => std::env::temp_dir().join("Pomelo"),
    }
}

/// Not under Caches: macOS may empty that while a staged update waits for a restart.
fn staging_root() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join("Library/Application Support/Pomelo/update"),
        None => cache_root().join("update"),
    }
}

fn marker_path() -> PathBuf {
    cache_root().join("just-updated")
}

fn check_and_stage(app_root: &Path) -> Result<(), String> {
    let body = curl(&format!(
        "https://api.github.com/repos/{REPO}/releases?per_page=20"
    ))?;
    let releases: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("parse releases: {e}"))?;
    let current = parse_version(current_version()).ok_or("bad current version")?;
    let staged = state()
        .staged
        .as_ref()
        .and_then(|staged| parse_version(&staged.version));
    let Some(release) = newest_release(&releases)
        .filter(|release| needs_download(&release.version, &current, staged.as_deref()))
    else {
        update(|state| state.found_nothing(SystemTime::now()));
        return Ok(());
    };
    let version = join_version(&release.version);
    eprintln!(
        "[update] {} -> {version} available, downloading into {}",
        current_version(),
        app_root.display()
    );
    update(|state| state.downloading(&version, release.size.map(|_| 0.0)));
    let staged = download_and_stage(&release, &version)?;
    update(|state| state.staged(staged, SystemTime::now()));
    Ok(())
}

fn download_and_stage(release: &Release, version: &str) -> Result<Staged, String> {
    let root = staging_root();
    if let Err(error) = std::fs::remove_dir_all(&root) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(format!("clear {}: {error}", root.display()));
        }
    }
    let dir = root.join(version);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let tarball = dir.join("Pomelo.app.tar.gz");
    download(&release.tarball, release.size, &tarball, version)?;

    update(|state| state.verifying(version));
    let signature = curl(&release.signature)?;
    let bytes = std::fs::read(&tarball).map_err(|e| format!("read update: {e}"))?;
    verify(&bytes, signature.trim(), UPDATE_PUBLIC_KEY)?;
    run(Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&dir))?;
    if let Err(error) = std::fs::remove_file(&tarball) {
        eprintln!("[update] remove {}: {error}", tarball.display());
    }
    let app = dir.join("Pomelo.app");
    if !app.is_dir() {
        return Err("the download has no Pomelo.app".into());
    }
    Ok(Staged {
        version: version.to_string(),
        app,
    })
}

fn download(url: &str, size: Option<u64>, to: &Path, version: &str) -> Result<(), String> {
    let mut child = Command::new("curl")
        .args([
            "-fsSL",
            "--connect-timeout",
            "20",
            "--max-time",
            "1800",
            "-o",
        ])
        .arg(to)
        .arg(url)
        .spawn()
        .map_err(|e| format!("curl spawn: {e}"))?;
    let mut shown_percent = 0;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => return Err(format!("curl: {error}")),
        }
        if let Some(total) = size.filter(|total| *total > 0) {
            let received = std::fs::metadata(to).map_or(0, |meta| meta.len());
            let progress = (received as f32 / total as f32).clamp(0.0, 1.0);
            let percent = (progress * 100.0) as u32;
            if percent != shown_percent {
                shown_percent = percent;
                update(|state| state.downloading(version, Some(progress)));
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    if status.success() {
        Ok(())
    } else {
        Err(format!("the download failed ({status})"))
    }
}

fn install_staged() -> Result<PathBuf, String> {
    let app_root = production_app_root().ok_or("only the installed Pomelo updates itself")?;
    let staged = state().staged.clone().ok_or("no update is ready")?;
    // macOS may purge ~/Library/Caches while the update waits there.
    if !staged.app.join("Contents/MacOS").is_dir() {
        state().staged = None;
        return Err("the downloaded update was removed; Try Again downloads it again".into());
    }
    let parked = app_root.with_file_name(".Pomelo.app.previous");
    if parked.exists() {
        std::fs::remove_dir_all(&parked).map_err(|e| format!("clear {}: {e}", parked.display()))?;
    }
    std::fs::rename(&app_root, &parked).map_err(|e| format!("move the old app aside: {e}"))?;
    if let Err(error) = move_into_place(&staged.app, &app_root) {
        if let Err(cleanup) = std::fs::remove_dir_all(&app_root) {
            eprintln!("[update] remove partial install: {cleanup}");
        }
        std::fs::rename(&parked, &app_root).map_err(|e| format!("{error}; restore: {e}"))?;
        return Err(error);
    }
    if let Err(error) = std::fs::remove_dir_all(&parked) {
        eprintln!("[update] remove {}: {error}", parked.display());
    }
    if let Err(error) = std::fs::remove_dir_all(staging_root()) {
        eprintln!("[update] clear staging: {error}");
    }
    let marker = marker_path();
    if let Err(error) = std::fs::write(
        &marker,
        format!("{}\n{}\n", current_version(), staged.version),
    ) {
        eprintln!("[update] write {}: {error}", marker.display());
    }
    state().staged = None;
    Ok(app_root)
}

/// A rename when both sit on one volume (atomic, nothing copied), else a copy.
fn move_into_place(staged: &Path, app_root: &Path) -> Result<(), String> {
    match std::fs::rename(staged, app_root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            run(Command::new("ditto").arg(staged).arg(app_root)).map_err(explain_denied)
        }
        Err(error) => Err(explain_denied(format!(
            "move {} to {}: {error}",
            staged.display(),
            app_root.display()
        ))),
    }
}

fn explain_denied(error: String) -> String {
    if error.contains("Operation not permitted") || error.contains("os error 1)") {
        format!(
            "{error}. macOS blocked it: allow Pomelo in System Settings > Privacy & Security > App Management, then Try Again"
        )
    } else {
        error
    }
}

fn relaunch(app_root: &Path) -> Result<(), String> {
    Command::new("/bin/sh")
        .arg("-c")
        .arg("while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open \"$2\"")
        .arg("relaunch")
        .arg(std::process::id().to_string())
        .arg(app_root)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("relaunch: {e}"))
}

fn curl(url: &str) -> Result<String, String> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--connect-timeout",
            "20",
            "--max-time",
            "60",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: pomelo-updater",
            url,
        ])
        .output()
        .map_err(|e| format!("curl spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!("curl {url}: status {}", out.status));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("curl utf8: {e}"))
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let output = cmd.output().map_err(|e| format!("spawn {cmd:?}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    match stderr.trim().lines().last() {
        Some(reason) => Err(format!("{cmd:?}: {reason}")),
        None => Err(format!("{cmd:?}: {}", output.status)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use ed25519_dalek::Signer;

    #[test]
    fn the_update_moves_into_place_and_a_failure_says_why() {
        let dir = std::env::temp_dir().join(format!("pomelo-update-{}", std::process::id()));
        let staged = dir.join("staged/Pomelo.app");
        std::fs::create_dir_all(staged.join("Contents/MacOS")).expect("staged");
        let target = dir.join("Applications/Pomelo.app");
        std::fs::create_dir_all(target.parent().expect("parent")).expect("applications");
        move_into_place(&staged, &target).expect("moved");
        assert!(target.join("Contents/MacOS").is_dir());
        assert!(!staged.exists());
        let error = run(Command::new("ditto").arg(&staged).arg(dir.join("copy"))).unwrap_err();
        assert!(error.contains("real path"), "{error}");
        assert!(
            explain_denied("rename: Operation not permitted (os error 1)".into())
                .contains("App Management")
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn only_an_update_signed_by_the_release_key_passes() {
        let engine = base64::engine::general_purpose::STANDARD;
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let public = engine.encode(key.verifying_key().to_bytes());
        let signature = engine.encode(key.sign(b"Pomelo.app tarball").to_bytes());
        assert!(verify(b"Pomelo.app tarball", &signature, &public).is_ok());
        assert!(verify(b"tampered tarball", &signature, &public).is_err());
        assert!(verify(b"Pomelo.app tarball", &signature, UPDATE_PUBLIC_KEY).is_err());
        assert!(verify(b"x", "not base64!", &public).is_err());
    }

    #[test]
    fn versions_compare_by_number_and_skip_what_is_staged() {
        assert_eq!(parse_version("0.10.2"), Some(vec![0, 10, 2]));
        assert_eq!(parse_version("0.10"), None);
        assert_eq!(parse_version("0.1.x"), None);
        assert!(needs_download(&[0, 10, 0], &[0, 9, 9], None));
        assert!(!needs_download(&[0, 9, 9], &[0, 9, 9], None));
        assert!(!needs_download(&[0, 7, 4], &[0, 7, 3], Some(&[0, 7, 4])));
        assert!(needs_download(&[0, 7, 5], &[0, 7, 3], Some(&[0, 7, 4])));
        assert_eq!(join_version(&[1, 2, 3]), "1.2.3");
    }

    #[test]
    fn the_newest_signed_release_wins() {
        let releases = serde_json::json!([
            {"tag_name": "v0.8.0", "assets": [{"name": "Pomelo.app.tar.gz", "browser_download_url": "u8"}]},
            {"tag_name": "v0.7.4", "assets": [
                {"name": "Pomelo.app.tar.gz", "browser_download_url": "u74", "size": 1200},
                {"name": "Pomelo.app.tar.gz.sig", "browser_download_url": "s74"}
            ]},
            {"tag_name": "v0.7.10", "assets": [
                {"name": "Pomelo.app.tar.gz", "browser_download_url": "u710"},
                {"name": "Pomelo.app.tar.gz.sig", "browser_download_url": "s710"}
            ]},
            {"tag_name": "nightly", "assets": []}
        ]);
        let newest = newest_release(&releases);
        assert_eq!(
            newest
                .as_ref()
                .map(|r| (r.version.clone(), r.tarball.as_str())),
            Some((vec![0, 7, 10], "u710"))
        );
        assert_eq!(newest.and_then(|r| r.size), None);
    }

    fn ready(state: &mut State, version: &str) {
        state.staged(
            Staged {
                version: version.into(),
                app: PathBuf::from("/tmp/Pomelo.app"),
            },
            SystemTime::UNIX_EPOCH,
        );
    }

    #[test]
    fn a_quiet_check_that_finds_nothing_or_fails_shows_nothing() {
        let mut state = State::default();
        assert!(state.begin(CheckKind::Automatic));
        assert_eq!(state.status, Status::Checking);
        assert!(!state.snapshot(true).manual);
        state.failed("offline".into());
        assert_eq!(state.status, Status::Idle);
        assert!(state.begin(CheckKind::Automatic));
        state.found_nothing(SystemTime::UNIX_EPOCH);
        assert_eq!(state.status, Status::Idle);
        assert_eq!(state.last_checked, Some(SystemTime::UNIX_EPOCH));
    }

    #[test]
    fn a_manual_check_reports_up_to_date_then_fades_and_reports_failures() {
        let mut state = State::default();
        assert!(state.begin(CheckKind::Manual));
        assert!(state.snapshot(true).manual);
        state.found_nothing(SystemTime::UNIX_EPOCH);
        assert_eq!(state.status, Status::UpToDate);
        state.expire_up_to_date();
        assert_eq!(state.status, Status::Idle);
        assert!(state.begin(CheckKind::Manual));
        state.failed("the download's signature did not verify".into());
        assert!(matches!(state.status, Status::Failed(_)));
        state.dismiss();
        assert_eq!(state.status, Status::Idle);
    }

    #[test]
    fn asking_during_a_quiet_check_makes_it_manual() {
        let mut state = State::default();
        assert!(state.begin(CheckKind::Automatic));
        assert!(!state.begin(CheckKind::Manual));
        assert_eq!(state.kind, CheckKind::Manual);
        state.found_nothing(SystemTime::UNIX_EPOCH);
        assert_eq!(state.status, Status::UpToDate);
    }

    #[test]
    fn a_ready_update_stays_through_quiet_checks_and_dismissal_lasts_until_a_manual_check() {
        let mut state = State::default();
        assert!(state.begin(CheckKind::Automatic));
        state.downloading("0.7.4", Some(0.45));
        state.verifying("0.7.4");
        ready(&mut state, "0.7.4");
        let ready_status = Status::Ready {
            version: "0.7.4".into(),
        };
        assert_eq!(state.status, ready_status);
        state.dismiss();
        assert!(state.snapshot(true).dismissed);
        assert!(state.begin(CheckKind::Automatic));
        assert_eq!(
            state.status, ready_status,
            "no flicker while checking quietly"
        );
        state.failed("offline".into());
        assert_eq!(state.status, ready_status);
        assert!(state.snapshot(true).dismissed);
        assert!(state.begin(CheckKind::Manual));
        assert!(!state.snapshot(true).dismissed);
        state.found_nothing(SystemTime::UNIX_EPOCH);
        assert_eq!(state.status, ready_status);
    }
}
