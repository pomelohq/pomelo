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
    assert!(shown.contains("app.rs |+1|-0"), "{shown}");
    assert!(shown.contains("new.rs |src/deep|+2|-0"), "{shown}");

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
