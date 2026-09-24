//! `mcp`: a stdio MCP server for the workspace the agent runs in. It needs no running app: the project
//! is the `pom.yml` above the current directory, the workspace comes from `--branch` or the
//! `workspace--<branch>` folder the agent was started in, else the main one.

mod config_files;
mod protocol;
mod tools;

use std::path::{Path, PathBuf};
use std::rc::Rc;

use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner};

pub use protocol::{Server, Tool, ToolArgs};
pub use tools::{tail_lines, Workspace};

const WORKSPACE_PREFIX: &str = "workspace--";

/// Handles `<binary> mcp [--branch <branch>]`; `None` when `args` is some other command.
pub fn run(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some("mcp") {
        return None;
    }
    let branch = flag(&args[2..], "--branch");
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let server = server_for(&cwd, branch.as_deref(), StateDir::from_env());
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    Some(match server.serve(stdin, stdout) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("mcp: {error}");
            1
        }
    })
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == name {
            return iter.next().cloned();
        }
        if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
    }
    None
}

/// The server for the project around `cwd`. Without a project (or with a config that fails to load)
/// it still answers, with no tools, so the agent's MCP client does not report a broken server.
pub fn server_for(cwd: &Path, branch: Option<&str>, state: StateDir) -> Server {
    let overflow_dir = state.path("mcp-out");
    let tools = workspace_for(cwd, branch, state)
        .map(|workspace| tools::tools(Rc::new(workspace)))
        .unwrap_or_default();
    Server {
        name: "pomelo".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        tools,
        overflow_dir,
    }
}

fn workspace_for(cwd: &Path, branch: Option<&str>, state: StateDir) -> Option<Workspace> {
    let config_path = cwd.ancestors().find_map(pom_core::config_in)?;
    let config = pom_config::Config::load(&config_path).ok()?;
    let branch = branch
        .filter(|branch| !branch.is_empty())
        .map(str::to_string)
        .or_else(|| branch_from_path(cwd))
        .unwrap_or_else(|| config.global_default_branch().to_string());
    let runner = ServiceRunner::new(RunnerOptions {
        project_root: config_path.parent()?.to_path_buf(),
        session: config.session.clone(),
        state: state.clone(),
        holders: SocketDir::from_env(),
        binary: std::env::current_exe().ok()?,
        docker: "docker".into(),
    });
    Some(Workspace {
        config_path,
        state,
        branch,
        runner,
    })
}

/// The branch of the `workspace--<branch>` folder `path` is in.
fn branch_from_path(path: &Path) -> Option<String> {
    path.components().find_map(|component| {
        component
            .as_os_str()
            .to_str()?
            .strip_prefix(WORKSPACE_PREFIX)
            .filter(|branch| !branch.is_empty())
            .map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_comes_from_the_workspace_folder() {
        assert_eq!(
            branch_from_path(Path::new("/p/workspace--feat-x/api/src")),
            Some("feat-x".to_string())
        );
        assert_eq!(branch_from_path(Path::new("/p/api")), None);
        let args: Vec<String> = ["--branch", "b1"].map(str::to_string).to_vec();
        assert_eq!(flag(&args, "--branch"), Some("b1".into()));
        assert_eq!(
            flag(&["--branch=b2".to_string()], "--branch"),
            Some("b2".into())
        );
    }
}
