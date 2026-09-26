//! Adding a repo to a project that already exists: it lands in main (cloned, or the clone already there), its
//! services are detected and its entry is written where the config keeps repos. Other workspaces get their
//! worktree afterwards, the way a new workspace would.

use std::path::{Path, PathBuf};

use pom_detect::{RepoDetection, ServiceKind};
use pom_paths::StateDir;

use crate::scaffold::{
    clone_remote, clone_with_changes, import_ignored_env, is_git_url, repo_name_from_url,
};
use crate::Project;

/// A repo to add: a git URL or a local folder, and an optional alias.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddRepoRequest {
    pub source: String,
    pub alias: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddedRepo {
    pub name: String,
    /// The config file its entry went into.
    pub file: PathBuf,
}

/// The repo's name as the project would call it.
pub fn repo_name(source: &str) -> String {
    let source = source.trim();
    if is_git_url(source) {
        return repo_name_from_url(source);
    }
    Path::new(source)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn add_repo(
    project: &Project,
    request: &AddRepoRequest,
    state: &StateDir,
) -> Result<AddedRepo, String> {
    let config = project
        .config
        .as_ref()
        .filter(|_| project.error.is_none())
        .ok_or("fix pom.yml first: it does not load")?;
    let source = request.source.trim();
    if source.is_empty() || source.starts_with('-') {
        return Err(format!("invalid repo source: {source}"));
    }
    let name = repo_name(source);
    if name.is_empty() {
        return Err(format!("cannot tell the repo name from {source}"));
    }
    if config.repos.contains_key(&name) {
        return Err(format!("{name} is already in the project"));
    }
    let main = pom_layout::workspace_root(&project.root, project.branch(), true);
    let destination = main.join(&name);
    if destination.exists() {
        // A clone already in main (made by hand, or left by a teammate's config) is taken as it is.
        if !pom_layout::is_git_repo(&destination) {
            return Err(format!(
                "{} exists and is not a git repo",
                destination.display()
            ));
        }
    } else if is_git_url(source) {
        clone_remote(source, &destination).map_err(|error| format!("clone {name}: {error}"))?;
    } else {
        let source_path = Path::new(source);
        if !pom_layout::is_git_repo(source_path) {
            return Err(format!("not a git repo: {source}"));
        }
        clone_with_changes(source_path, &destination)
            .map_err(|error| format!("clone {name}: {error}"))?;
        import_ignored_env(source_path, state, &project.session);
    }
    let detection = RepoDetection {
        name: name.clone(),
        alias: request.alias.trim().to_string(),
        apps: pom_detect::detect_repo(&destination),
        shared: pom_detect::parse_compose(&destination)
            .into_iter()
            .filter(|service| service.kind == ServiceKind::Shared)
            .collect(),
    };
    let (block, kinds) = pom_detect::emit_repo(&detection);
    let new_kinds: Vec<String> = kinds
        .into_iter()
        .filter(|kind| !config.shared_services.contains_key(kind))
        .collect();
    let file = write_entry(&project.config_path, &name, &block, &new_kinds)?;
    Ok(AddedRepo { name, file })
}

/// Clones a repo the config already names, but main lacks, from `source` (a git URL or local folder).
pub fn clone_into_main(
    project: &Project,
    name: &str,
    source: &str,
    state: &StateDir,
) -> Result<(), String> {
    let source = source.trim();
    if source.is_empty() || source.starts_with('-') {
        return Err(format!("invalid repo source: {source}"));
    }
    let main = pom_layout::workspace_root(&project.root, project.branch(), true);
    let destination = main.join(name);
    if destination.exists() {
        return Err(format!("{} already exists", destination.display()));
    }
    if is_git_url(source) {
        return clone_remote(source, &destination)
            .map_err(|error| format!("clone {name}: {error}"));
    }
    let source_path = Path::new(source);
    if !pom_layout::is_git_repo(source_path) {
        return Err(format!("not a git repo: {source}"));
    }
    clone_with_changes(source_path, &destination)
        .map_err(|error| format!("clone {name}: {error}"))?;
    import_ignored_env(source_path, state, &project.session);
    Ok(())
}

/// A likely URL for `name`: another repo of main's origin with its last path part swapped, since a
/// project's repos usually live side by side on the same host and owner.
pub fn guess_remote(project: &Project, name: &str) -> String {
    let main = pom_layout::workspace_root(&project.root, project.branch(), true);
    let Some(config) = project.config.as_ref() else {
        return String::new();
    };
    config
        .repos
        .keys()
        .filter(|repo| repo.as_str() != name)
        .find_map(|repo| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(main.join(repo))
                .args(["remote", "get-url", "origin"])
                .output()
                .ok()?;
            let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let cut = url.rfind(['/', ':'])?;
            let suffix = if url.ends_with(".git") { ".git" } else { "" };
            Some(format!("{}{name}{suffix}", &url[..=cut]))
        })
        .unwrap_or_default()
}

/// What taking a repo out left behind: worktrees kept because they had changes, and main's clone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemovedRepo {
    pub removed_worktrees: Vec<String>,
    pub kept_worktrees: Vec<String>,
    pub main_clone: Option<PathBuf>,
}

/// Takes `repo` out of the config, then removes its worktree from every other workspace where it has no
/// changes. Main's clone stays: it may hold work, and the user deletes it knowingly.
pub fn remove_repo(project: &Project, repo: &str) -> Result<RemovedRepo, String> {
    pom_config::maintain::remove_repo(&project.config_path, repo)?;
    let main = pom_layout::workspace_root(&project.root, project.branch(), true);
    let clone = main.join(repo);
    let mut outcome = RemovedRepo {
        main_clone: clone.is_dir().then(|| clone.clone()),
        ..RemovedRepo::default()
    };
    for workspace in project
        .workspaces
        .iter()
        .filter(|workspace| !workspace.is_main)
    {
        let worktree = workspace.path.join(repo);
        if !worktree.is_dir() {
            continue;
        }
        let clean = std::process::Command::new("git")
            .arg("-C")
            .arg(&worktree)
            .args(["status", "--porcelain"])
            .output()
            .is_ok_and(|output| output.status.success() && output.stdout.is_empty());
        let removed = clean
            && std::process::Command::new("git")
                .arg("-C")
                .arg(&clone)
                .args(["worktree", "remove"])
                .arg(&worktree)
                .status()
                .is_ok_and(|status| status.success());
        if removed {
            outcome.removed_worktrees.push(workspace.branch.clone());
        } else {
            outcome.kept_worktrees.push(workspace.branch.clone());
        }
    }
    Ok(outcome)
}

fn shared_entries(kinds: &[String]) -> String {
    kinds
        .iter()
        .map(|kind| format!("  {kind}:\n    type: {kind}\n"))
        .collect()
}

/// Writes the entry into a new `pom.d/repos` fragment when the config is split, else into `pom.yml`, and
/// takes it back if the config then fails to load.
fn write_entry(
    config_path: &Path,
    name: &str,
    block: &str,
    new_kinds: &[String],
) -> Result<PathBuf, String> {
    let dir = config_path.parent().unwrap_or(Path::new("."));
    let fragments = pom_config::fragment_files(dir).unwrap_or_default();
    if !fragments.is_empty() {
        let repos_dir = dir.join(pom_config::FRAGMENT_DIR).join("repos");
        let next = fragments
            .iter()
            .filter(|path| path.starts_with(&repos_dir))
            .filter_map(|path| {
                let stem = path.file_name()?.to_string_lossy().into_owned();
                stem.split('-').next()?.parse::<u32>().ok()
            })
            .max()
            .map_or(1, |highest| highest + 1);
        let file = repos_dir.join(format!("{next:02}-{name}.yml"));
        let mut text = format!("repos:\n{block}");
        if !new_kinds.is_empty() {
            text.push_str(&format!("shared_services:\n{}", shared_entries(new_kinds)));
        }
        std::fs::create_dir_all(&repos_dir).map_err(|error| error.to_string())?;
        std::fs::write(&file, text).map_err(|error| error.to_string())?;
        if let Err(problem) = pom_config::edit::load_and_validate(config_path) {
            if let Err(error) = std::fs::remove_file(&file) {
                eprintln!("add repo: take back {}: {error}", file.display());
            }
            return Err(format!("the new entry breaks the config: {problem}"));
        }
        return Ok(file);
    }
    let before = std::fs::read_to_string(config_path).map_err(|error| error.to_string())?;
    let mut text = insert_under_top_key(&before, "repos", block);
    if !new_kinds.is_empty() {
        text = insert_under_top_key(&text, "shared_services", &shared_entries(new_kinds));
    }
    std::fs::write(config_path, &text).map_err(|error| error.to_string())?;
    if let Err(problem) = pom_config::edit::load_and_validate(config_path) {
        if let Err(error) = std::fs::write(config_path, &before) {
            eprintln!("add repo: restore {}: {error}", config_path.display());
        }
        return Err(format!("the new entry breaks the config: {problem}"));
    }
    Ok(config_path.to_path_buf())
}

/// `text` with `block` (already indented under it) appended to the end of the top-level mapping `key`,
/// creating the key at the end of the file when it is missing. Everything else is left as written.
fn insert_under_top_key(text: &str, key: &str, block: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let header = format!("{key}:");
    let Some(start) = lines
        .iter()
        .position(|line| line.trim_end() == header || line.starts_with(&format!("{header} ")))
    else {
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("{header}\n{block}"));
        return out;
    };
    // The mapping ends at the next line that starts in column 0 with content; trailing blank or comment
    // lines stay after the inserted entry.
    let mut end = lines.len();
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        let top_level = !line.is_empty() && !line.starts_with([' ', '\t', '#']);
        if top_level {
            end = index;
            break;
        }
    }
    while end > start + 1 && (lines[end - 1].trim().is_empty() || lines[end - 1].starts_with('#')) {
        end -= 1;
    }
    let mut out: Vec<String> = lines[..end].iter().map(|line| line.to_string()).collect();
    out.extend(block.lines().map(str::to_string));
    out.extend(lines[end..].iter().map(|line| line.to_string()));
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_goes_to_the_end_of_its_mapping_and_leaves_the_rest_alone() {
        let text = "session: demo\nrepos:\n  api:\n    alias: be\n\n# shared\nshared_services:\n  redis:\n    type: redis\n";
        let out = insert_under_top_key(text, "repos", "  web:\n    alias: web\n");
        assert_eq!(
            out,
            "session: demo\nrepos:\n  api:\n    alias: be\n  web:\n    alias: web\n\n# shared\nshared_services:\n  redis:\n    type: redis\n"
        );
        let out = insert_under_top_key("session: demo\n", "repos", "  web: {}\n");
        assert_eq!(out, "session: demo\nrepos:\n  web: {}\n");
        assert_eq!(repo_name("git@github.com:acme/web.git"), "web");
        assert_eq!(repo_name("/src/api/"), "api");
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn a_local_repo_is_cloned_into_main_and_gets_a_fragment_in_a_split_config() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("src/web");
        std::fs::create_dir_all(&source).expect("source");
        std::fs::write(
            source.join("package.json"),
            r#"{"name":"web","scripts":{"dev":"vite"},"dependencies":{"vite":"5"}}"#,
        )
        .expect("package.json");
        git(&source, &["init", "-q", "-b", "main"]);
        git(&source, &["add", "."]);
        git(&source, &["commit", "-q", "-m", "init"]);

        let root = temp.path().join("demo");
        std::fs::create_dir_all(root.join("workspace--main")).expect("main");
        std::fs::create_dir_all(root.join("pom.d/repos")).expect("pom.d");
        std::fs::write(root.join("pom.yml"), "session: demo\n").expect("root");
        std::fs::write(
            root.join("pom.d/repos/01-api.yml"),
            "repos:\n  api:\n    alias: api\n",
        )
        .expect("fragment");
        let state = StateDir::new(temp.path().join("state"));
        let project = Project::open(&root.join("pom.yml"), &state);
        let request = AddRepoRequest {
            source: source.display().to_string(),
            alias: "fe".into(),
        };
        let added = add_repo(&project, &request, &state).expect("add");
        assert_eq!(added.name, "web");
        assert_eq!(added.file, root.join("pom.d/repos/02-web.yml"));
        assert!(root.join("workspace--main/web/.git").exists());
        let reloaded = Project::open(&root.join("pom.yml"), &state);
        let repo = reloaded
            .config
            .as_ref()
            .and_then(|config| config.repos.get("web").cloned())
            .expect("web in config");
        assert_eq!(repo.alias, "fe");
        assert!(
            add_repo(&reloaded, &request, &state).is_err(),
            "twice is refused"
        );
    }
}
