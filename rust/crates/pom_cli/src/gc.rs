//! `pom ws gc`: delete the workspaces whose pull requests are merged (or closed).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use pom_forge::{Client, PrTarget, PullRequest};
use pom_workspace::DeleteRequest;
use serde_json::{json, Value};

use crate::{say, Session};

const SCHEMA: &str = "pom.ws-gc/v1";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GcArgs {
    pub merged: bool,
    pub closed: bool,
    pub yes: bool,
    pub dry_run: bool,
    pub json: bool,
    pub force: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    WouldDelete,
    Deleted,
    Kept,
    Skipped,
    Failed,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Action::WouldDelete => "would_delete",
            Action::Deleted => "deleted",
            Action::Kept => "kept",
            Action::Skipped => "skipped",
            Action::Failed => "failed",
        }
    }
}

struct Candidate {
    branch: String,
    reason: String,
    prs: Vec<(String, PullRequest)>,
    unsaved: Vec<String>,
    action: Action,
    notes: Vec<String>,
}

/// Asks whether to delete one workspace; `None` when nobody can answer (not a terminal).
pub(crate) type Ask<'a> = Option<&'a mut dyn FnMut(&mut dyn Write, &str) -> Result<bool, String>>;

fn target_for(repo: &str, root: &Path, branch: &str) -> Option<PrTarget> {
    let remote = git::working_copy::remote_url(root, "origin")?;
    let (_, owner, name) = git::working_copy::parse_github_remote(&remote)?;
    Some(PrTarget {
        repo: repo.to_string(),
        owner,
        name,
        head: branch.to_string(),
    })
}

impl Session {
    pub(crate) fn gc(&self, args: &GcArgs, out: &mut dyn Write) -> Result<(), String> {
        let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin()) && !args.json;
        let mut confirm = |out: &mut dyn Write, question: &str| crate::db::confirm(out, question);
        let ask: Ask<'_> = if interactive {
            Some(&mut confirm)
        } else {
            None
        };
        if !args.yes && !args.dry_run && ask.is_none() {
            return Err(
                "ws gc deletes workspaces: pass --yes, or --dry-run to only list them".into(),
            );
        }
        let client = pom_forge::resolve(&self.state, &self.config.session)
            .ok_or("no GitHub token: set GH_TOKEN, or store the project's `github` secret")?;
        self.gc_with(args, &client, ask, out)
    }

    pub(crate) fn gc_with(
        &self,
        args: &GcArgs,
        client: &Client,
        mut ask: Ask<'_>,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let merged = args.merged || !args.closed;
        let workspaces: Vec<(&str, Vec<PrTarget>)> = self
            .project
            .workspaces
            .iter()
            .filter(|workspace| !workspace.is_main)
            .map(|workspace| {
                let targets = workspace
                    .repos
                    .iter()
                    .filter_map(|repo| target_for(&repo.name, &repo.path, &repo.branch))
                    .collect();
                (workspace.branch.as_str(), targets)
            })
            .collect();
        let all: Vec<PrTarget> = workspaces
            .iter()
            .flat_map(|(_, targets)| targets.iter().cloned())
            .collect();
        let (found, failure) = pom_forge::fetch_latest(client, &all);
        if let Some(error) = failure {
            return Err(format!("look up pull requests on GitHub: {error}"));
        }
        let mut candidates: Vec<Candidate> = Vec::new();
        for (branch, targets) in &workspaces {
            let prs: Vec<(String, PullRequest)> = targets
                .iter()
                .filter_map(|target| {
                    let pr = found.get(&target.key()).cloned().flatten()?;
                    Some((target.repo.clone(), pr))
                })
                .collect();
            let Some(reason) = pom_forge::finished_reason(
                prs.iter().map(|(repo, pr)| (repo.as_str(), pr)),
                merged,
                args.closed,
            ) else {
                continue;
            };
            let unsaved = pom_workspace::unsaved_work(
                &self.config,
                &self.project.root,
                branch,
                &pushed_heads(&prs),
            );
            candidates.push(Candidate {
                branch: branch.to_string(),
                reason,
                prs,
                unsaved,
                action: Action::WouldDelete,
                notes: Vec::new(),
            });
        }
        if candidates.is_empty() && !args.json {
            return say(out, "no workspace has a finished pull request");
        }
        for candidate in &mut candidates {
            if !args.json {
                say(out, &format!("{}  {}", candidate.branch, candidate.reason))?;
            }
            if !candidate.unsaved.is_empty() && !args.force {
                candidate.action = Action::Kept;
                candidate.notes = candidate.unsaved.clone();
                if !args.json {
                    for line in &candidate.unsaved {
                        say(out, &format!("  kept: {line}"))?;
                    }
                    say(out, "  (--force deletes it anyway)")?;
                }
                continue;
            }
            if args.dry_run {
                continue;
            }
            if !args.yes {
                let question = format!("Delete workspace {}? [y/N] ", candidate.branch);
                let answer = match ask.as_mut() {
                    Some(ask) => ask(out, &question)?,
                    None => false,
                };
                if !answer {
                    candidate.action = Action::Skipped;
                    continue;
                }
            }
            self.gc_delete(candidate, args, out)?;
        }
        let failed = candidates
            .iter()
            .filter(|candidate| candidate.action == Action::Failed)
            .count();
        if args.json {
            let list: Vec<Value> = candidates.iter().map(candidate_json).collect();
            say(
                out,
                &serde_json::to_string(&json!({"schema": SCHEMA, "workspaces": list}))
                    .map_err(|error| error.to_string())?,
            )?;
        }
        if failed > 0 {
            return Err(format!("{failed} workspace(s) could not be deleted"));
        }
        Ok(())
    }

    fn gc_delete(
        &self,
        candidate: &mut Candidate,
        args: &GcArgs,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let request = DeleteRequest {
            branch: candidate.branch.clone(),
            keep_unsaved_work: !args.force,
            pushed_heads: pushed_heads(&candidate.prs),
            ..DeleteRequest::default()
        };
        let context = self.context();
        let result = if args.json {
            pom_workspace::delete(&context, &request, &|_| {})
        } else {
            crate::workspaces::stream(out, |sink| pom_workspace::delete(&context, &request, sink))?
        };
        match result {
            Ok(outcome) => {
                candidate.action = Action::Deleted;
                candidate.notes = outcome.warnings;
                if !args.json {
                    say(out, &format!("  deleted {}", candidate.branch))?;
                }
            }
            Err(error) => {
                let kept = error.message.strip_prefix("kept: ");
                candidate.action = if kept.is_some() {
                    Action::Kept
                } else {
                    Action::Failed
                };
                candidate.notes = vec![kept.unwrap_or(&error.to_string()).to_string()];
                if !args.json {
                    say(
                        out,
                        &format!("  {}: {}", candidate.action.name(), candidate.notes[0]),
                    )?;
                }
            }
        }
        Ok(())
    }
}

fn pushed_heads(prs: &[(String, PullRequest)]) -> BTreeMap<String, String> {
    prs.iter()
        .filter(|(_, pr)| !pr.head_ref_oid.is_empty())
        .map(|(repo, pr)| (repo.clone(), pr.head_ref_oid.clone()))
        .collect()
}

fn candidate_json(candidate: &Candidate) -> Value {
    let prs: Vec<Value> = candidate
        .prs
        .iter()
        .map(|(repo, pr)| {
            json!({
                "repo": repo,
                "number": pr.number,
                "state": pr.state.to_lowercase(),
                "url": pr.url,
            })
        })
        .collect();
    json!({
        "branch": candidate.branch,
        "reason": candidate.reason,
        "prs": prs,
        "action": candidate.action.name(),
        "notes": candidate.notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    use pom_core::Project;
    use pom_forge::{ForgeError, Response, Transport};
    use pom_paths::StateDir;
    use pom_ptyhost::SocketDir;
    use pom_services::{RunnerOptions, ServiceRunner};

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Answers each branch's lookup from `prs`: branch -> (state, head commit).
    struct Forge {
        prs: BTreeMap<String, (String, String)>,
    }

    impl Transport for Forge {
        fn post(&self, _url: &str, _token: &str, body: &str) -> Result<Response, ForgeError> {
            let request: Value = serde_json::from_str(body).expect("body");
            let query = request["query"].as_str().unwrap_or_default();
            let mut data = serde_json::Map::new();
            for line in query.lines() {
                let Some((alias, rest)) = line.trim().split_once(": repository(") else {
                    continue;
                };
                let head = rest
                    .split("headRefName: \"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .unwrap_or_default();
                let nodes: Vec<Value> = self
                    .prs
                    .get(head)
                    .map(|(state, oid)| {
                        json!({"number": 7, "state": state, "headRefName": head, "headRefOid": oid,
                               "url": "https://github.com/acme/api/pull/7"})
                    })
                    .into_iter()
                    .collect();
                data.insert(alias.into(), json!({"pullRequests": {"nodes": nodes}}));
            }
            Ok(Response {
                status: 200,
                body: json!({ "data": data }).to_string(),
                ..Response::default()
            })
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        state: StateDir,
        session: Session,
    }

    fn fixture(branches: &[&str]) -> Fixture {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("project");
        let main = root.join("workspace--main/api");
        std::fs::create_dir_all(&main).expect("main");
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("README"), "x\n").expect("readme");
        git(&main, &["add", "."]);
        git(&main, &["commit", "-q", "-m", "init"]);
        git(
            &main,
            &["remote", "add", "origin", "https://github.com/acme/api.git"],
        );
        for branch in branches {
            let folder = root.join(format!("workspace--{branch}/api"));
            git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    branch,
                    &folder.to_string_lossy(),
                ],
            );
            std::fs::write(folder.join("work.txt"), "w\n").expect("work");
            git(&folder, &["add", "."]);
            git(&folder, &["commit", "-q", "-m", "work"]);
        }
        std::fs::write(
            root.join("pom.yml"),
            "session: demo\nrepos:\n  api:\n    services:\n      web: rails s\n",
        )
        .expect("pom.yml");
        let state = StateDir::new(temp.path().join("state"));
        let project = Project::open(&root.join("pom.yml"), &state);
        let config = project.config.clone().expect("config");
        let runner = ServiceRunner::new(RunnerOptions {
            project_root: root.clone(),
            session: "demo".into(),
            state: state.clone(),
            holders: SocketDir::new(temp.path().join("s")),
            binary: PathBuf::from("/nonexistent"),
            docker: PathBuf::from("/nonexistent/docker"),
        });
        Fixture {
            root,
            state: state.clone(),
            session: Session {
                project,
                state,
                config,
                runner,
                branch: "main".into(),
                is_main: true,
            },
            _temp: temp,
        }
    }

    fn head(fixture: &Fixture, branch: &str) -> String {
        git(
            &fixture.root.join(format!("workspace--{branch}/api")),
            &["rev-parse", "HEAD"],
        )
    }

    #[test]
    fn gc_deletes_a_merged_workspace_whole_and_keeps_a_dirty_or_open_one() {
        let fixture = fixture(&["feat-done", "feat-dirty", "feat-open"]);
        std::fs::write(
            fixture.root.join("workspace--feat-dirty/api/README"),
            "edited\n",
        )
        .expect("edit");
        let agents = fixture.state.path("agents");
        for file in [
            "sessions/s1/state.json".to_string(),
            "index/demo/feat-done.json".into(),
            "leases/ws-demo-feat-done-claude-raw.json".into(),
            "workspace-log/demo/feat-done.ndjson".into(),
            "state-feat-done.json".into(),
        ] {
            let path = agents.join(file);
            std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
            std::fs::write(path, "{}").expect("file");
        }
        std::fs::write(
            agents.join("sessions/s1/state.json"),
            json!({"project": "demo", "branch": "feat-done", "holder": "ws-demo-feat-done-claude-raw"})
                .to_string(),
        )
        .expect("session");
        let forge = Forge {
            prs: [
                ("feat-done", "MERGED", head(&fixture, "feat-done")),
                ("feat-dirty", "MERGED", head(&fixture, "feat-dirty")),
                ("feat-open", "OPEN", head(&fixture, "feat-open")),
            ]
            .into_iter()
            .map(|(branch, state, oid)| (branch.to_string(), (state.to_string(), oid)))
            .collect(),
        };
        let client = Client::new("token", Box::new(forge));
        let args = GcArgs {
            yes: true,
            json: true,
            ..GcArgs::default()
        };
        let mut out = Vec::new();
        fixture
            .session
            .gc_with(&args, &client, None, &mut out)
            .expect("gc");
        let report: Value = serde_json::from_slice(&out).expect("json");
        assert_eq!(report["schema"], SCHEMA);
        let actions: Vec<(String, String)> = report["workspaces"]
            .as_array()
            .expect("list")
            .iter()
            .map(|entry| {
                (
                    entry["branch"].as_str().unwrap_or_default().to_string(),
                    entry["action"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            actions,
            [
                ("feat-dirty".to_string(), "kept".to_string()),
                ("feat-done".to_string(), "deleted".to_string()),
            ]
        );
        assert_eq!(
            report["workspaces"][0]["notes"][0],
            "api has 1 uncommitted change"
        );
        assert!(!fixture.root.join("workspace--feat-done").exists());
        assert!(fixture.root.join("workspace--feat-dirty").exists());
        assert!(fixture.root.join("workspace--feat-open").exists());
        let mut left = Vec::new();
        let mut stack = vec![agents];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                if entry.path().is_dir() {
                    stack.push(entry.path());
                } else {
                    left.push(entry.path());
                }
            }
        }
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn without_yes_or_a_terminal_nothing_is_deleted() {
        let fixture = fixture(&["feat-done"]);
        let forge = Forge {
            prs: [(
                "feat-done".to_string(),
                ("MERGED".to_string(), head(&fixture, "feat-done")),
            )]
            .into_iter()
            .collect(),
        };
        let client = Client::new("token", Box::new(forge));
        let mut out = Vec::new();
        fixture
            .session
            .gc_with(&GcArgs::default(), &client, None, &mut out)
            .expect("gc");
        assert!(fixture.root.join("workspace--feat-done").exists());
        let mut answered = Vec::new();
        let mut no = |_: &mut dyn Write, question: &str| {
            answered.push(question.to_string());
            Ok(false)
        };
        fixture
            .session
            .gc_with(&GcArgs::default(), &client, Some(&mut no), &mut out)
            .expect("gc");
        assert_eq!(answered, ["Delete workspace feat-done? [y/N] "]);
        assert!(fixture.root.join("workspace--feat-done").exists());
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.contains("feat-done  api #7 merged"), "{text}");
    }
}
