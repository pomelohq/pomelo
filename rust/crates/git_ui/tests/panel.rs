//! The Git panel over a real repository: rows for the branch's changes, opening a file, and discarding
//! uncommitted work after confirming.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_ui::{GitPanel, RepoSource};
use workspace::{PaneKind, PanelRequest, SidePanelView};

const ROW_STRIDE: u64 = 4;

fn run(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("git");
    assert!(output.status.success(), "{args:?}");
}

fn row(index: u64) -> u64 {
    workspace::side_panel_base(PaneKind::Git) + index * ROW_STRIDE
}

fn texts(panel: &mut GitPanel) -> String {
    let node = panel.render(320.0, 400.0);
    ui::render(
        &node,
        ui::Rect::new(0.0, 0.0, 320.0, 400.0, ui::Rgba::TRANSPARENT),
    )
    .texts
    .iter()
    .map(|text| text.text.clone())
    .collect::<Vec<_>>()
    .join("|")
}

#[test]
fn lists_the_branch_changes_and_discards_after_confirming() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("api");
    std::fs::create_dir_all(&root).expect("repo");
    run(&root, &["init", "-q", "-b", "main"]);
    run(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("app.rs"), "one\n").expect("write");
    run(&root, &["add", "."]);
    run(&root, &["commit", "-q", "-m", "base"]);
    run(&root, &["checkout", "-q", "-b", "feat"]);
    std::fs::create_dir_all(root.join("src/deep")).expect("dir");
    std::fs::write(root.join("src/deep/new.rs"), "a\nb\n").expect("write");
    std::fs::write(root.join("app.rs"), "one\ntwo\n").expect("write");

    let mut panel = GitPanel::new(
        vec![RepoSource {
            name: "api".into(),
            root: root.clone(),
            default_branch: "main".into(),
        }],
        None,
        Arc::new(|| {}),
    );
    panel.render(320.0, 400.0);
    panel.wait_for_scan();
    let shown = texts(&mut panel);
    assert!(shown.contains("2 changed files"), "{shown}");
    assert!(shown.contains("api|feat"), "{shown}");
    assert!(shown.contains("app.rs|+1|-0"), "{shown}");
    assert!(shown.contains("new.rs|src/deep|+2|-0"), "{shown}");

    // Rows: 0 repo, 1 app.rs, 2 src/deep/new.rs.
    panel.click(row(1));
    assert!(matches!(
        panel.take_requests().as_slice(),
        [PanelRequest::OpenDiff { path, base: Some(base) }]
            if path.ends_with("app.rs") && base == "one\n"
    ));
    panel.click(row(2));
    assert!(matches!(
        panel.take_requests().as_slice(),
        [PanelRequest::OpenDiff { path, base: None }] if path.ends_with("new.rs")
    ));

    assert!(panel.open_menu(row(1)));
    let discard = panel
        .menu_items()
        .into_iter()
        .find(|item| item.label == "Discard Uncommitted Changes")
        .expect("discard offered");
    panel.menu_action(discard.id);
    let Some(PanelRequest::Prompt { tag, .. }) = panel.take_requests().into_iter().next() else {
        panic!("discarding asks first");
    };
    panel.prompt_answered(tag, 1);
    assert_eq!(
        std::fs::read_to_string(root.join("app.rs")).expect("read"),
        "one\ntwo\n"
    );
    panel.prompt_answered(tag, 0);
    assert_eq!(
        std::fs::read_to_string(root.join("app.rs")).expect("read"),
        "one\ntwo\n",
        "an answer only counts once"
    );

    panel.menu_action(0);
    assert!(panel.open_menu(row(1)));
    let discard = panel
        .menu_items()
        .into_iter()
        .find(|item| item.label == "Discard Uncommitted Changes")
        .expect("discard offered");
    panel.menu_action(discard.id);
    let Some(PanelRequest::Prompt { tag, .. }) = panel.take_requests().into_iter().next() else {
        panic!("asks again");
    };
    panel.prompt_answered(tag, 0);
    assert_eq!(
        std::fs::read_to_string(root.join("app.rs")).expect("read"),
        "one\n"
    );
    panel.wait_for_scan();
    assert!(texts(&mut panel).contains("1 changed file"));
}

#[test]
fn review_marks_hold_until_the_file_changes_and_survive_reopening() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("api");
    std::fs::create_dir_all(&root).expect("repo");
    run(&root, &["init", "-q", "-b", "main"]);
    run(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("a.rs"), "1\n").expect("write");
    std::fs::write(root.join("b.rs"), "1\n").expect("write");
    run(&root, &["add", "."]);
    run(&root, &["commit", "-q", "-m", "base"]);
    run(&root, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(root.join("a.rs"), "2\n").expect("write");
    std::fs::write(root.join("b.rs"), "2\n").expect("write");
    let reviews = temp.path().join("state/reviews/demo-feat.json");
    let open = || {
        let mut panel = GitPanel::new(
            vec![RepoSource {
                name: "api".into(),
                root: root.clone(),
                default_branch: "main".into(),
            }],
            Some(reviews.clone()),
            Arc::new(|| {}),
        );
        panel.render(320.0, 400.0);
        panel.wait_for_scan();
        panel.render(320.0, 400.0);
        panel
    };
    let mut panel = open();
    // Rows: 0 repo, 1 a.rs, 2 b.rs; control 2 is the reviewed box.
    panel.click(row(1) + 2);
    assert!(texts(&mut panel).contains("1 of 2 reviewed"));
    drop(panel);

    let mut panel = open();
    assert!(
        texts(&mut panel).contains("1 of 2 reviewed"),
        "marks are kept"
    );
    std::fs::write(root.join("a.rs"), "3\n").expect("write");
    panel.refresh();
    panel.wait_for_scan();
    assert!(
        texts(&mut panel).contains("2 changed files"),
        "an edit after the review clears the mark"
    );
}

const FOOTER: u64 = 9_001_000;

fn footer(offset: u64) -> u64 {
    workspace::side_panel_base(PaneKind::Git) + FOOTER + offset
}

fn git_out(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn settle(panel: &mut GitPanel) -> Vec<PanelRequest> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut requests = Vec::new();
    loop {
        panel.render(320.0, 400.0);
        requests.extend(panel.take_requests());
        if !panel.operation_running() || std::time::Instant::now() > deadline {
            panel.wait_for_scan();
            return requests;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn stages_commits_and_publishes_from_the_panel() {
    let temp = tempfile::tempdir().expect("temp");
    let remote = temp.path().join("remote.git");
    run(
        temp.path(),
        &["init", "-q", "--bare", "-b", "main", "remote.git"],
    );
    let root = temp.path().join("api");
    std::fs::create_dir_all(&root).expect("repo");
    run(&root, &["init", "-q", "-b", "main"]);
    run(&root, &["config", "commit.gpgsign", "false"]);
    run(&root, &["config", "user.name", "t"]);
    run(&root, &["config", "user.email", "t@example.com"]);
    std::fs::write(root.join("app.rs"), "one\n").expect("write");
    run(&root, &["add", "."]);
    run(&root, &["commit", "-q", "-m", "base"]);
    run(
        &root,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    );
    run(&root, &["push", "-q", "-u", "origin", "main"]);
    run(&root, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(root.join("app.rs"), "one\ntwo\n").expect("write");
    std::fs::write(root.join("notes.md"), "draft\n").expect("write");

    let mut panel = GitPanel::new(
        vec![RepoSource {
            name: "api".into(),
            root: root.clone(),
            default_branch: "main".into(),
        }],
        None,
        Arc::new(|| {}),
    );
    panel.render(320.0, 400.0);
    panel.wait_for_scan();
    let shown = texts(&mut panel);
    assert!(shown.contains("Stage All"), "{shown}");
    assert!(shown.contains("Publish"), "{shown}");
    assert!(shown.contains("Commit Tracked"), "{shown}");
    assert!(
        shown.contains("Update app.rs"),
        "the single tracked change is suggested: {shown}"
    );

    panel.click(row(2) + 3);
    panel.wait_for_scan();
    assert_eq!(
        git_out(&root, &["diff", "--cached", "--name-only"]),
        "notes.md"
    );
    let shown = texts(&mut panel);
    assert!(shown.ends_with("Create notes.md|Commit"), "{shown}");

    panel.click_at(footer(1), 20.0, 12.0);
    assert!(panel.text_focused());
    panel.text("Add notes\n\nWhy they help");
    panel.key(workspace::EditKey::ReplaceAll, false);
    let requests = settle(&mut panel);
    assert!(requests.is_empty(), "a commit that works says nothing");
    assert_eq!(git_out(&root, &["log", "-1", "--format=%s"]), "Add notes");
    assert_eq!(git_out(&root, &["status", "--porcelain"]), "M app.rs");

    panel.click(footer(4));
    let requests = settle(&mut panel);
    assert!(
        matches!(
            requests.as_slice(),
            [PanelRequest::ToastAction { message, action, .. }]
                if message == "Pushed feat to origin" && action == "View Log"
        ),
        "{:?}",
        requests.len()
    );
    assert_eq!(
        git_out(&root, &["rev-parse", "--abbrev-ref", "feat@{upstream}"]),
        "origin/feat"
    );
    let shown = texts(&mut panel);
    assert!(shown.contains("Fetch"), "tracked and even: {shown}");
}

#[test]
fn a_failed_push_offers_its_log() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("api");
    std::fs::create_dir_all(&root).expect("repo");
    run(&root, &["init", "-q", "-b", "main"]);
    run(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("app.rs"), "one\n").expect("write");
    run(&root, &["add", "."]);
    run(&root, &["commit", "-q", "-m", "base"]);
    run(
        &root,
        &["remote", "add", "origin", "/nonexistent/remote.git"],
    );
    let mut panel = GitPanel::new(
        vec![RepoSource {
            name: "api".into(),
            root,
            default_branch: "main".into(),
        }],
        None,
        Arc::new(|| {}),
    );
    panel.render(320.0, 400.0);
    panel.wait_for_scan();
    panel.click(footer(4));
    let requests = settle(&mut panel);
    assert!(matches!(
        requests.as_slice(),
        [PanelRequest::ToastAction { message, action, then }]
            if message == "git push failed"
                && action == "View Log"
                && matches!(**then, PanelRequest::OpenItem(_))
    ));
}

#[test]
fn the_repo_row_opens_its_pull_request() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("web");
    std::fs::create_dir_all(&root).expect("repo");
    run(&root, &["init", "-q", "-b", "main"]);
    run(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("app.rs"), "one\n").expect("write");
    run(&root, &["add", "."]);
    run(&root, &["commit", "-q", "-m", "base"]);
    let prs = pull_request_ui::PullRequests::new(
        pom_paths::StateDir::new(temp.path().join("state")),
        "myproject",
        Arc::new(|| {}),
    );
    let target = pom_forge::PrTarget {
        repo: "web".into(),
        owner: "acme".into(),
        name: "web".into(),
        head: "main".into(),
    };
    prs.show(
        "main",
        vec![(
            root.clone(),
            target,
            pom_forge::PullRequest {
                number: 42,
                state: "OPEN".into(),
                ..pom_forge::PullRequest::default()
            },
        )],
    );
    let mut panel = GitPanel::new(
        vec![RepoSource {
            name: "web".into(),
            root,
            default_branch: "main".into(),
        }],
        None,
        Arc::new(|| {}),
    )
    .with_pull_requests(prs);
    panel.render(320.0, 400.0);
    panel.wait_for_scan();
    let shown = texts(&mut panel);
    assert!(
        shown.contains("Pull Requests|1|#42"),
        "listed at the top: {shown}"
    );
    assert!(shown.contains("web|main|#42"), "{shown}");
    // Rows: 0 the pull requests header, 1 web's pull request, 2 the repo.
    panel.click(row(1));
    assert!(matches!(
        panel.take_requests().as_slice(),
        [PanelRequest::Reveal { id, .. }] if id == "pr:web:main"
    ));
    panel.click(row(2) + 2);
    assert!(matches!(
        panel.take_requests().as_slice(),
        [PanelRequest::Reveal { id, .. }] if id == "pr:web:main"
    ));
    panel.click(row(0));
    assert!(
        !texts(&mut panel).contains("Pull Requests|1|#42"),
        "the list folds"
    );
}

#[test]
fn shows_one_repo_at_a_time_and_picks_another_from_the_selector() {
    let temp = tempfile::tempdir().expect("temp");
    let mut sources = Vec::new();
    for name in ["web", "api", "infra"] {
        let root = temp.path().join(name);
        std::fs::create_dir_all(&root).expect("repo");
        run(&root, &["init", "-q", "-b", "main"]);
        run(&root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join(format!("{name}.rs")), "one\n").expect("write");
        run(&root, &["add", "."]);
        run(&root, &["commit", "-q", "-m", "base"]);
        run(&root, &["checkout", "-q", "-b", "feat"]);
        std::fs::write(root.join(format!("{name}.rs")), "one\ntwo\n").expect("write");
        sources.push(RepoSource {
            name: name.into(),
            root,
            default_branch: "main".into(),
        });
    }
    let mut panel = GitPanel::new(sources, None, Arc::new(|| {}));
    panel.render(320.0, 400.0);
    panel.wait_for_scan();
    let shown = texts(&mut panel);
    assert!(
        shown.contains("web.rs") && !shown.contains("api.rs"),
        "{shown}"
    );

    panel.click(footer(6));
    assert!(panel.text_focused(), "the selector takes typing");
    let shown = texts(&mut panel);
    assert!(
        shown.contains("api|infra|web|Select a repository..."),
        "sorted, filter below: {shown}"
    );
    panel.text("inf");
    assert!(panel.key(workspace::EditKey::Enter, false));
    let shown = texts(&mut panel);
    assert!(
        shown.contains("infra.rs") && !shown.contains("web.rs"),
        "{shown}"
    );
    assert!(!panel.text_focused(), "picking closes it");

    panel.click(footer(6));
    assert!(panel.key(workspace::EditKey::Escape, false));
    assert!(!texts(&mut panel).contains("Select a repository..."));
}
