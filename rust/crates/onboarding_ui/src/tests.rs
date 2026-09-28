use std::path::Path;

use onboarding::{Check, CheckKind, CheckStatus};
use pom_agent::AgentCli;

use super::*;

fn git_repo(root: &Path, name: &str) -> std::path::PathBuf {
    let repo = root.join(name);
    std::fs::create_dir_all(&repo).expect("repo");
    let status = std::process::Command::new("git")
        .args(["init", "--quiet", "-b", "main"])
        .current_dir(&repo)
        .status()
        .expect("git");
    assert!(status.success());
    std::fs::write(repo.join("go.mod"), "module example.com/api\n").expect("go.mod");
    repo
}

fn new_page(root: &Path, agents: Vec<(AgentCli, bool)>) -> (OnboardingPage, Shared) {
    let shared: Shared = Rc::new(RefCell::new(Onboarding::new(root.join("sessions"), agents)));
    (OnboardingPage::new(shared.clone()), shared)
}

fn type_text(page: &mut OnboardingPage, text: &str) {
    page.input_text(text);
}

fn press(page: &mut OnboardingPage, key: &str, cmd: bool) -> TerminalKeyOutcome {
    let mut keystroke = Keystroke::new(key, Modifiers::default());
    keystroke.modifiers.cmd = cmd;
    page.keystroke(&keystroke)
}

#[test]
fn the_form_needs_a_name_and_a_repo_then_walks_to_create() {
    let temp = tempfile::tempdir().expect("tempdir");
    let api = git_repo(temp.path(), "api");
    let (mut page, shared) = new_page(temp.path(), vec![(AgentCli::Claude, true)]);
    type_text(&mut page, "shop");
    press(&mut page, "enter", false);
    assert_eq!(shared.borrow().form.step, Step::Repositories, "no repo yet");

    page.click(ids::CHOOSE);
    assert_eq!(
        shared.borrow_mut().take_requests(),
        vec![Request::ChooseFolders]
    );
    assert_eq!(
        shared
            .borrow_mut()
            .add_folders(&[temp.path().to_path_buf()]),
        1,
        "the repo inside the folder"
    );
    assert_eq!(
        shared.borrow().form.repos[0]
            .scan
            .as_ref()
            .map(|scan| scan.stack.clone()),
        Some(vec!["Go".to_string()])
    );
    assert_eq!(
        shared.borrow_mut().add_folders(std::slice::from_ref(&api)),
        0,
        "already added"
    );

    page.click(ids::MODE_URLS);
    page.paste(
        "git@github.com:acme/web.git\nhttps://github.com/acme/worker",
        None,
    );
    assert_eq!(shared.borrow().form.repos.len(), 3);
    assert!(shared.borrow().form.url.text().is_empty());

    press(&mut page, "enter", false);
    assert_eq!(shared.borrow().form.step, Step::Setup);
    assert!(shared.borrow().form.with_agent);
    page.click(ids::SETUP_MANUAL);
    page.click(ids::OPT_WORKSPACE);
    press(&mut page, "enter", false);
    assert_eq!(shared.borrow().form.step, Step::Review);
    press(&mut page, "enter", true);
    assert_eq!(shared.borrow_mut().take_requests(), vec![Request::Create]);

    shared.borrow_mut().start_run();
    let state = shared.borrow();
    let run = state.run.as_ref().expect("a run");
    assert_eq!(run.repos, vec!["api", "web", "worker"]);
    assert!(!run.with_agent);
    assert_eq!(run.first_workspace, None);
    assert_eq!(state.screen, Screen::Progress);
    assert_eq!(page.title(), "Setting up shop");
}

#[test]
fn names_branches_and_agents_are_checked() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join("sessions/taken")).expect("taken");
    let (mut page, shared) = new_page(
        temp.path(),
        vec![(AgentCli::Claude, false), (AgentCli::Codex, true)],
    );
    assert_eq!(
        shared.borrow().form.agent,
        AgentCli::Codex,
        "the first installed CLI"
    );
    type_text(&mut page, "taken");
    assert!(shared
        .borrow()
        .name_error()
        .is_some_and(|error| error.contains("already exists")));
    shared.borrow_mut().form.name.set_text("a/b");
    assert!(shared.borrow().name_error().is_some());
    shared.borrow_mut().form.name.set_text("fresh");
    shared.borrow_mut().form.workspace_branch.set_text("main");
    assert!(shared.borrow().workspace_branch_error().is_some());

    page.click(ids::AGENT_BASE);
    assert_eq!(
        shared.borrow().form.agent,
        AgentCli::Codex,
        "Claude is not installed"
    );

    let (mut page, shared) = new_page(temp.path(), vec![(AgentCli::Claude, false)]);
    assert!(
        !shared.borrow().form.with_agent,
        "no agent, manual by default"
    );
    page.click(ids::SETUP_AGENT);
    assert!(!shared.borrow().form.with_agent);
}

fn check(kind: CheckKind, status: CheckStatus) -> Check {
    Check {
        kind,
        status,
        detail: String::new(),
        output: String::new(),
        command: String::new(),
    }
}

#[test]
fn a_failed_check_opens_a_finding_and_verify_again_keeps_what_passed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut page, shared) = new_page(temp.path(), vec![(AgentCli::Claude, true)]);
    shared.borrow_mut().form.name.set_text("shop");
    shared.borrow_mut().start_run();
    {
        let mut state = shared.borrow_mut();
        let run = state.run.as_mut().expect("run");
        run.enter(Phase::Verify);
        run.record(check(CheckKind::Doctor, CheckStatus::Running));
        run.record(check(CheckKind::Doctor, CheckStatus::Passed));
        let boot = CheckKind::Boot {
            repo: "api".into(),
            service: "web".into(),
        };
        run.record(check(boot.clone(), CheckStatus::Failed));
        assert_eq!(run.checks.len(), 2, "a check's start and end share a row");
        assert_eq!(run.phase, Phase::Repair);
    }
    page.click(ids::FIX_AGENT);
    page.click(ids::FIX_MANUAL);
    assert!(shared
        .borrow()
        .run
        .as_ref()
        .and_then(|run| run.finding.as_ref())
        .is_some_and(|finding| finding.manual));
    page.click(ids::OPEN_TERMINAL);
    page.click(ids::SKIP_AGENT);
    assert_eq!(
        shared.borrow_mut().take_requests(),
        vec![
            Request::FixWithAgent,
            Request::OpenTerminal { repo: "api".into() },
            Request::SkipAgent
        ]
    );
    let mut state = shared.borrow_mut();
    let run = state.run.as_mut().expect("run");
    run.restart_verify_skipping();
    assert_eq!(run.skipped, vec![("api".to_string(), "web".to_string())]);
    assert!(run.finding.is_none());
    assert!(run.repaired);
    assert_eq!(run.phase, Phase::Verify);
    assert_eq!(run.checks.len(), 1, "the doctor stays passed");
    drop(state);
    shared.borrow_mut().finish();
    assert_eq!(page.title(), "shop is ready");
    assert!(ui::measure(&page.tree(900.0)).1 > 200.0);
}
