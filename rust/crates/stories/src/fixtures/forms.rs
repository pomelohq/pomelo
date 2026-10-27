//! The window's modals in the states the design boards draw them, filled from fixtures and driven through
//! their own clicks and keys, so the snapshot shows what a user would see.

use std::sync::Arc;
use std::time::{Duration, Instant};

use workspace::{EditKey, WindowModal, WINDOW_MODAL_BASE};
use workspaces_ui::{
    AddRepoModal, BranchSource, CreateWorkspaceModal, PickReposModal, RenameWorkspaceModal,
    RepoBranches, TicketList, TicketSource, UseBranchModal,
};

// The forms' own click ids, as they lay them out.
const NAME_FIELD: u64 = WINDOW_MODAL_BASE + 2;
const BRANCH_FIELD: u64 = WINDOW_MODAL_BASE + 3;
const BRANCH_BOX: u64 = WINDOW_MODAL_BASE + 300;
const ADD_REPO_USE_AI: u64 = WINDOW_MODAL_BASE + 5;
const ADD_REPO_WORKSPACE: u64 = WINDOW_MODAL_BASE + 100;
const TICKET_FIELD: u64 = WINDOW_MODAL_BASE + 7;
const PICK_REPO_BASE: u64 = WINDOW_MODAL_BASE + 100;

fn namer() -> workspaces_ui::Namer {
    Arc::new(|seed: &str, _: &str| {
        Ok(pom_agent::NameSuggestion {
            name: seed.into(),
            slug: seed.into(),
        })
    })
}

#[derive(Clone, Copy)]
enum Jira {
    Answers,
    Slow,
    Fails,
}

fn tickets() -> TicketSource {
    tickets_from(Jira::Answers)
}

fn tickets_from(jira: Jira) -> TicketSource {
    let issue = |key: &str, summary: &str| pom_jira::SprintIssue {
        key: key.into(),
        summary: summary.into(),
        status: "To Do".into(),
        category: "new".into(),
        mine: true,
        ..pom_jira::SprintIssue::default()
    };
    let issues = vec![
        issue("PROJ-118", "Login page"),
        issue("PROJ-120", "Password reset email"),
    ];
    TicketSource {
        boards: Arc::new(|| {
            Ok(vec![pom_jira::Board {
                id: 1,
                name: "Web".into(),
            }])
        }),
        issues: Arc::new(move |_| match jira {
            Jira::Answers => Ok(issues.clone()),
            Jira::Slow => {
                std::thread::sleep(Duration::from_secs(60));
                Ok(Vec::new())
            }
            Jira::Fails => Err("Jira answered 401 Unauthorized: check the API token".into()),
        }),
        board: Some(1),
        start: Some(TicketList::Sprint(1)),
        remember: Arc::new(|_| {}),
        only_mine: false,
    }
}

fn branches() -> BranchSource {
    branches_of(false)
}

/// The repo's branches; `few` keeps only main and the one origin branch the use-branch board names.
fn branches_of(few: bool) -> BranchSource {
    let info = |name: &str, remote: bool, author: &str, when: &str, subject: &str| {
        pom_workspace::BranchInfo {
            name: name.into(),
            remote,
            author: author.into(),
            relative_time: when.into(),
            subject: subject.into(),
        }
    };
    let listed = vec![
        info(
            "ana/checkout-ui",
            true,
            "Ana Lima",
            "40 minutes ago",
            "New checkout summary card",
        ),
        info(
            "ana/checkout-api",
            true,
            "Ana Lima",
            "2 hours ago",
            "Validate cart totals",
        ),
        info("main", false, "", "", ""),
    ];
    let listed: Vec<pom_workspace::BranchInfo> = if few {
        listed
            .into_iter()
            .filter(|info| info.name != "ana/checkout-api")
            .rev()
            .collect()
    } else {
        listed
    };
    BranchSource {
        list: Arc::new(move |_| {
            Ok(RepoBranches {
                base: "main".into(),
                branches: listed.clone(),
            })
        }),
        fetch: Arc::new(|_| Ok(())),
    }
}

/// Lets the modal's background work (lists, fetches) land, as the window's ticks would.
fn settle(modal: &mut dyn WindowModal) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        modal.tick();
        if !modal.busy() || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn create(existing: &[&str]) -> CreateWorkspaceModal {
    let outlooks: workspaces_ui::ModulesOutlook = Arc::new(|repo: &str| match repo {
        "api" => module_store::Outlook::InstallsOnce,
        "web" => module_store::Outlook::FromStore,
        _ => module_store::Outlook::Installs,
    });
    let mut modal = CreateWorkspaceModal::new(
        vec!["api".into(), "web".into(), "worker".into()],
        existing.iter().map(|branch| branch.to_string()).collect(),
        namer(),
    )
    .with_options(
        vec!["local".into(), "staging".into()],
        vec!["api".into(), "web".into()],
    )
    .with_modules(outlooks)
    .with_tickets(tickets())
    .with_branches(branches());
    settle(&mut modal);
    modal.text("PROJ-118");
    modal.key(EditKey::Enter, false);
    modal.click(BRANCH_BOX + 1);
    settle(&mut modal);
    modal.text("checkout-ui");
    modal.key(EditKey::Enter, false);
    settle(&mut modal);
    modal
}

/// The create form as it opens, its Ticket list dropped down over Jira answering this way.
fn create_tickets(jira: Jira) -> CreateWorkspaceModal {
    let mut modal = CreateWorkspaceModal::new(
        vec!["api".into(), "web".into(), "worker".into()],
        Vec::new(),
        namer(),
    )
    .with_tickets(tickets_from(jira))
    .with_branches(branches());
    // A Jira that never answers keeps the form busy, so settle only waits for the branches.
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        modal.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    if !matches!(jira, Jira::Slow) {
        settle(&mut modal);
    }
    modal.click(TICKET_FIELD);
    modal
}

/// The modal a scene draws: `create`, `create-branch`, `create-taken`, `create-tickets`,
/// `create-tickets-loading`, `create-tickets-error`, `rename`, `add-repo`, `add-repo-blank`, `use-branch`,
/// `pick-repos`, or `connect-` and a step of Connect to Host.
pub fn modal(name: &str) -> Option<Box<dyn WindowModal>> {
    if let Some(step) = name.strip_prefix("connect-") {
        return super::host::connect_modal(step)
            .map(|modal| Box::new(modal) as Box<dyn WindowModal>);
    }
    Some(match name {
        "create" => {
            let mut modal = create(&[]);
            modal.click(NAME_FIELD);
            Box::new(modal)
        }
        "create-branch" => {
            let mut modal = create(&[]);
            modal.click(BRANCH_BOX + 1);
            settle(&mut modal);
            modal.text("checkout");
            modal.key(EditKey::Up, false);
            Box::new(modal)
        }
        "create-taken" => {
            let mut modal = create(&["feat-login"]);
            modal.click(BRANCH_FIELD);
            modal.key(EditKey::SelectAll, false);
            modal.text("feat-login");
            Box::new(modal)
        }
        "create-tickets" => Box::new(create_tickets(Jira::Answers)),
        "create-tickets-loading" => Box::new(create_tickets(Jira::Slow)),
        "create-tickets-error" => Box::new(create_tickets(Jira::Fails)),
        "add-repo-blank" => Box::new(AddRepoModal::new(
            vec!["PROJ-101 Fix login".into(), "PROJ-107 New sidebar".into()],
            vec!["api".into(), "web".into()],
            Box::new(Vec::new),
            true,
        )),
        "pick-repos" => {
            let mut modal = PickReposModal::new("feat-login", vec!["worker".into(), "docs".into()]);
            modal.click(PICK_REPO_BASE);
            Box::new(modal)
        }
        "rename" => Box::new(RenameWorkspaceModal::new(
            "feat-login",
            "PROJ-101 Fix login page",
            namer(),
        )),
        "add-repo" => {
            let mut modal = AddRepoModal::new(
                vec!["PROJ-101 Fix login".into(), "PROJ-107 New sidebar".into()],
                vec!["api".into(), "web".into()],
                Box::new(Vec::new),
                true,
            );
            modal.text("git@github.com:acme/mobile.git");
            modal.click(ADD_REPO_WORKSPACE + 1);
            modal.click(ADD_REPO_USE_AI);
            Box::new(modal)
        }
        "use-branch" => {
            let mut modal =
                UseBranchModal::new("web", "feat-login", Some("feat-login"), branches_of(true));
            settle(&mut modal);
            modal.text("ana/checkout-ui");
            Box::new(modal)
        }
        _ => return None,
    })
}

/// The confirmation a scene draws: `delete`, `discard-all`, `discard-file`.
pub fn prompt(name: &str) -> Option<workspace::Prompt> {
    if name == "save-conflict" {
        return super::host::conflict_prompt();
    }
    let (message, detail, confirm) = match name {
        "delete" => (
            "Delete Workspace".to_string(),
            workspace::delete_workspace_detail("PROJ-112 Retry failed mail", "retry-failed-mail"),
            "Delete",
        ),
        "discard-all" => (
            "Discard every tracked change in every repo?".to_string(),
            "Staged and unstaged changes to tracked files are lost; new files stay.".to_string(),
            "Discard",
        ),
        "discard-file" => (
            "Discard uncommitted changes to db/schema.rb?".to_string(),
            "Its staged and unstaged changes are lost; the branch's commits stay.".to_string(),
            "Discard",
        ),
        _ => return None,
    };
    let warning = (name == "delete")
        .then(|| workspace::unpushed_warning(&[("worker".to_string(), 2)]))
        .flatten();
    Some(workspace::Prompt {
        token: 0,
        message,
        detail: Some(detail),
        buttons: vec![confirm.into(), "Cancel".into()],
        warning,
    })
}
