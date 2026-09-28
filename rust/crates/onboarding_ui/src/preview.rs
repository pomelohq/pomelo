//! Sample states of the page, for headless snapshots.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use onboarding::{Check, CheckKind, CheckStatus, Counts, Segment};
use pom_agent::AgentCli;
use pom_core::RepoScan;
use workspace::text_field::TextField;

use crate::model::{CloneProgress, Finding, Onboarding, Phase, RepoRow, Run, Step};
use crate::{OnboardingPage, Shared};

fn scan(name: &str, stack: &[&str], compose: Option<&str>, infra: &[&str], env: usize) -> RepoScan {
    RepoScan {
        name: name.into(),
        alias: name.into(),
        stack: stack.iter().map(|text| text.to_string()).collect(),
        compose: compose.map(str::to_string),
        infra: infra.iter().map(|text| text.to_string()).collect(),
        env_files: env,
    }
}

fn strong(text: &str) -> Segment {
    Segment {
        text: text.into(),
        strong: true,
    }
}

fn plain(text: &str) -> Segment {
    Segment {
        text: text.into(),
        strong: false,
    }
}

fn check(kind: CheckKind, status: CheckStatus, detail: &str) -> Check {
    Check {
        kind,
        status,
        detail: detail.into(),
        output: String::new(),
        command: String::new(),
    }
}

/// The page in the state `which` names: repos, setup, review, progress, repair, skipped, done.
pub fn preview_page(which: &str) -> OnboardingPage {
    let mut state = Onboarding::new(
        PathBuf::from("/Users/you/Workspaces"),
        vec![
            (AgentCli::Claude, true),
            (AgentCli::Codex, true),
            (AgentCli::Gemini, false),
        ],
    );
    state.form.name.set_text("myproject");
    let scans = [
        scan(
            "api",
            &["Rails"],
            Some("docker-compose.yml"),
            &["postgres", "redis"],
            3,
        ),
        scan("web", &["Vite"], None, &[], 1),
    ];
    for (source, scan) in [
        "/Users/you/code/myproject-api",
        "/Users/you/code/myproject-web",
    ]
    .into_iter()
    .zip(scans.clone())
    {
        state.form.repos.push(RepoRow {
            source: source.into(),
            alias: TextField::default(),
            scan: Some(scan),
        });
    }
    state.form.repos.push(RepoRow {
        source: "git@github.com:acme/worker.git".into(),
        alias: TextField::default(),
        scan: None,
    });
    state.form.focus = None;
    match which {
        "setup" => state.form.step = Step::Setup,
        "review" => state.form.step = Step::Review,
        "repos" => {}
        _ => {
            state.form.step = Step::Review;
            state.start_run();
        }
    }
    if let Some(run) = state.run.as_mut() {
        fill_run(run, which, &scans);
    }
    if which == "done" {
        state.finish();
        if let Some(run) = state.run.as_mut() {
            run.finished = Some(run.started + std::time::Duration::from_secs(312));
        }
    }
    let shared: Shared = Rc::new(RefCell::new(state));
    OnboardingPage::new(shared)
}

fn fill_run(run: &mut Run, which: &str, scans: &[RepoScan]) {
    run.clone = vec![
        CloneProgress {
            percent: 100,
            linked: Some(true),
        },
        CloneProgress {
            percent: 100,
            linked: Some(true),
        },
        CloneProgress {
            percent: 100,
            linked: Some(false),
        },
    ];
    let mut scans = scans.to_vec();
    scans.push(scan(
        "worker",
        &["Node"],
        Some("compose.yml"),
        &["redis", "minio"],
        0,
    ));
    run.scans = scans;
    run.enter(Phase::Scan);
    run.enter(Phase::Configure);
    run.summary = vec![
        vec![
            strong("api"),
            plain(": services "),
            strong("web"),
            plain(" (bundle exec puma), "),
            strong("jobs"),
            plain(" (bundle exec sidekiq)"),
        ],
        vec![
            strong("api"),
            plain(": setup bundle install - migrate bin/rails db:migrate"),
        ],
        vec![
            strong("api"),
            plain(": DATABASE_URL -> postgresql://{{shared.postgres.url}}/{{db.main}}"),
        ],
        vec![
            strong("web"),
            plain(": services "),
            strong("dev"),
            plain(" (pnpm vite --port $PORT)"),
        ],
    ];
    run.agent_shown = true;
    if which == "progress" {
        return;
    }
    if which == "skipped" {
        run.agent_skipped = true;
    }
    run.enter(Phase::Verify);
    run.record(check(
        CheckKind::Doctor,
        CheckStatus::Passed,
        "0 errors, 0 warnings",
    ));
    run.record(check(
        CheckKind::Install { repo: "api".into() },
        CheckStatus::Passed,
        "bundle install - 41s",
    ));
    run.record(check(
        CheckKind::Install { repo: "web".into() },
        CheckStatus::Passed,
        "pnpm install - 18s",
    ));
    if which == "skipped" {
        run.record(check(
            CheckKind::Boot {
                repo: "api".into(),
                service: "web".into(),
            },
            CheckStatus::Running,
            "",
        ));
        return;
    }
    let mut boot = check(
        CheckKind::Boot {
            repo: "api".into(),
            service: "web".into(),
        },
        CheckStatus::Failed,
        "PG::ConnectionBad: database \"myproject_main\" does not exist",
    );
    boot.output = "=> Booting Puma\n=> Rails 7.1 application starting\nPG::ConnectionBad: database \"myproject_main\" does not exist".into();
    run.record(boot);
    if which == "fixing" {
        if let Some(finding) = run.finding.as_mut() {
            *finding = Finding {
                check: finding.check.clone(),
                fixing: true,
                manual: false,
            };
        }
    }
    if which == "done" {
        run.restart_verify();
        run.checks
            .retain(|check| check.status == CheckStatus::Passed);
        run.record(check(
            CheckKind::Boot {
                repo: "api".into(),
                service: "web".into(),
            },
            CheckStatus::Passed,
            "listening on :31022",
        ));
        run.counts = Counts {
            repos: 3,
            services: 4,
            shared: 3,
            databases: 2,
        };
        run.secrets_imported = 4;
        run.set_up = vec![
            (
                "api".into(),
                vec!["Rails".into()],
                "web, jobs - migrate, seed".into(),
            ),
            ("web".into(), vec!["Vite".into()], "dev".into()),
            ("worker".into(), vec!["Node".into()], "consumer".into()),
        ];
    }
}
