//! `pom init` and `pom onboard`: a new project from repos, then either Claude turning its seed config into a
//! runnable one interactively in this terminal, or a review list for doing it by hand.

use std::io::Write;
use std::path::{Path, PathBuf};

use pom_paths::StateDir;

use crate::args::Args;
use crate::say;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OnboardCommand {
    Init {
        name: Option<String>,
        claude: bool,
    },
    Onboard {
        session: Option<String>,
        new: Option<String>,
        repos: Vec<String>,
        branch: String,
        no_ai: bool,
    },
}

pub(crate) fn parse(name: &str, words: &[&str]) -> Result<OnboardCommand, String> {
    if name == "init" {
        let args = Args::parse(words, &[])?;
        args.allow(&["--ai", "--claude"])?;
        args.at_most(1, "init")?;
        return Ok(OnboardCommand::Init {
            name: args.positional.first().cloned(),
            claude: args.has("--ai") || args.has("--claude"),
        });
    }
    let args = Args::parse(words, &["--new", "--repo", "--branch"])?;
    args.allow(&["--new", "--repo", "--branch", "--no-ai"])?;
    args.at_most(1, "onboard")?;
    let new = args.value(&["--new"]);
    let repos = args.values("--repo");
    if new.is_some() && repos.is_empty() {
        return Err("onboard --new needs at least one --repo <path or git URL>".into());
    }
    if new.is_none() && !repos.is_empty() {
        return Err("--repo only goes with --new".into());
    }
    Ok(OnboardCommand::Onboard {
        session: args.positional.first().cloned(),
        new,
        repos,
        branch: args.value(&["--branch"]).unwrap_or_else(|| "main".into()),
        no_ai: args.has("--no-ai"),
    })
}

pub(crate) fn execute(
    command: &OnboardCommand,
    cwd: &Path,
    out: &mut dyn Write,
) -> Result<(), String> {
    let state = StateDir::from_env();
    match command {
        OnboardCommand::Init { name, claude } => {
            let top = git_output(cwd, &["rev-parse", "--show-toplevel"])
                .ok_or("not inside a git repo")?;
            let top = PathBuf::from(top);
            let name = name.clone().unwrap_or_else(|| {
                top.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            let branch = git_output(
                &top,
                &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
            )
            .and_then(|head| head.split_once('/').map(|(_, branch)| branch.to_string()))
            .unwrap_or_else(|| "main".into());
            let project = scaffold(&state, &name, &branch, &[top.display().to_string()], out)?;
            if *claude {
                return onboard_interactively(&state, &project.join("pom.yml"), out);
            }
            review_by_hand(&project.join("pom.yml"), cwd, out)
        }
        OnboardCommand::Onboard {
            session,
            new,
            repos,
            branch,
            no_ai,
        } => {
            let config_path = match (new, session) {
                (Some(name), _) => {
                    let sources: Vec<String> = repos
                        .iter()
                        .map(|repo| {
                            let path = cwd.join(repo);
                            if path.exists() {
                                path.display().to_string()
                            } else {
                                repo.clone()
                            }
                        })
                        .collect();
                    scaffold(&state, name, branch, &sources, out)?.join("pom.yml")
                }
                (None, Some(name)) => {
                    let sessions = pom_sessions::Sessions::load(&state);
                    let entry = sessions
                        .get(name)
                        .ok_or_else(|| format!("no session {name}"))?;
                    pom_core::config_in(Path::new(&entry.path))
                        .ok_or_else(|| format!("no pom.yml in {}", entry.path))?
                }
                (None, None) => cwd
                    .ancestors()
                    .find_map(pom_core::config_in)
                    .ok_or("no pom.yml here or in any parent directory")?,
            };
            if *no_ai {
                return review_by_hand(&config_path, cwd, out);
            }
            onboard_interactively(&state, &config_path, out)
        }
    }
}

/// The manual path: what was drafted, what still needs attention, and how to go on.
fn review_by_hand(config_path: &Path, cwd: &Path, out: &mut dyn Write) -> Result<(), String> {
    say(out, &format!("\nreview {}:", config_path.display()))?;
    // A drafted config usually still has gaps; list them without failing the command.
    if let Err(problems) = crate::doctor(Some(config_path), cwd, out) {
        say(out, &format!("{problems}: fix them in pom.yml"))?;
    }
    let project = config_path.parent().unwrap_or(cwd);
    say(
        out,
        &format!(
            "\nedit it with `pom config edit`, then cd {} and run `pom start`",
            project.display()
        ),
    )
}

fn scaffold(
    state: &StateDir,
    name: &str,
    branch: &str,
    sources: &[String],
    out: &mut dyn Write,
) -> Result<PathBuf, String> {
    say(
        out,
        &format!("creating project {name} from {} repo(s)...", sources.len()),
    )?;
    let request = pom_core::ScaffoldRequest {
        name: name.to_string(),
        root: String::new(),
        default_branch: branch.to_string(),
        repos: sources
            .iter()
            .map(|path| pom_core::RepoSpec {
                path: path.clone(),
                alias: String::new(),
            })
            .collect(),
    };
    let project = pom_core::scaffold_session(&request, state)?;
    say(out, &format!("wrote {}", project.join("pom.yml").display()))?;
    Ok(project)
}

/// Claude with the onboarding prompt, in this terminal; it asks before acting as it always does.
fn onboard_interactively(
    state: &StateDir,
    config_path: &Path,
    out: &mut dyn Write,
) -> Result<(), String> {
    let project = pom_core::Project::open(config_path, state);
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    let binary = std::env::current_exe().map_err(|error| error.to_string())?;
    let cwd = project.active_root();
    let branch = project.branch().to_string();
    let launch = pom_agent::onboard_launch(&pom_agent::LaunchContext {
        state,
        home: &home,
        binary: &binary,
        tool_path: pom_services::tool_path(),
        session: &project.session,
        branch: &branch,
        is_main: true,
        cwd: &cwd,
    });
    let (program, args) = launch
        .argv
        .split_first()
        .ok_or("the agent has no command")?;
    say(
        out,
        &format!("onboarding {}: Claude opens here\n", project.session),
    )?;
    out.flush().map_err(|error| error.to_string())?;
    let status = std::process::Command::new(program)
        .args(args)
        .current_dir(&launch.cwd)
        .status()
        .map_err(|error| format!("start {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the agent exited with {status}"))
    }
}

fn git_output(dir: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onboard_arguments() {
        assert_eq!(
            parse(
                "onboard",
                &["--new", "shop", "--repo", "../api", "--repo", "../web"]
            ),
            Ok(OnboardCommand::Onboard {
                session: None,
                new: Some("shop".into()),
                repos: vec!["../api".into(), "../web".into()],
                branch: "main".into(),
                no_ai: false,
            })
        );
        assert!(matches!(
            parse("onboard", &["shop", "--no-ai"]),
            Ok(OnboardCommand::Onboard { no_ai: true, .. })
        ));
        assert!(parse("onboard", &["--new", "shop"]).is_err());
        assert!(parse("onboard", &["--repo", "x"]).is_err());
        assert_eq!(
            parse("init", &["demo", "--claude"]),
            Ok(OnboardCommand::Init {
                name: Some("demo".into()),
                claude: true
            })
        );
        assert!(matches!(
            parse("init", &["--ai"]),
            Ok(OnboardCommand::Init { claude: true, .. })
        ));
    }
}
