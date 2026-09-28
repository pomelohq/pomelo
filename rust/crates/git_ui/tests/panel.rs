//! The Git panel over real repositories: staging from both sections, one commit per repo, the three tabs,
//! history grouping, commits waiting on either side of origin, drift in the branch row, discarding after
//! confirming, review marks, remote actions and pull requests.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use git_ui::{GitPanel, RepoSource, Tab};
use workspace::{PanelRequest, SidePanelView};

const WIDTH: f32 = 330.0;
const HEIGHT: f32 = 1200.0;
/// A row's controls sit right after its own id: checkbox, review eye, remote chip, menu.
const CHECK: u64 = 1;
const EYE: u64 = 2;
const CHIP: u64 = 3;

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_out(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write(root: &Path, path: &str, text: &str) {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("dir");
    }
    std::fs::write(full, text).expect("write");
}

/// A repo with `files` committed on main, then on `branch`.
fn repo(parent: &Path, name: &str, files: &[&str], branch: &str) -> PathBuf {
    let root = parent.join(name);
    std::fs::create_dir_all(&root).expect("repo");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    for file in files {
        write(&root, file, "one\n");
    }
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "base"]);
    if branch != "main" {
        git(&root, &["checkout", "-q", "-b", branch]);
    }
    root
}

/// `root` pushed to a fresh bare remote as origin, tracking.
fn publish(parent: &Path, root: &Path, name: &str) -> PathBuf {
    let remote = parent.join(format!("{name}.git"));
    git(
        parent,
        &["init", "-q", "--bare", "-b", "main", &format!("{name}.git")],
    );
    git(
        root,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    );
    git(root, &["push", "-q", "-u", "origin", "HEAD"]);
    remote
}

fn source(name: &str, root: &Path, branch: &str) -> RepoSource {
    RepoSource {
        name: name.into(),
        root: root.to_path_buf(),
        default_branch: "main".into(),
        expected_branch: branch.into(),
        kept: false,
    }
}

fn panel(sources: Vec<RepoSource>) -> GitPanel {
    let mut panel = GitPanel::new(sources, None, Arc::new(|| {}));
    panel.render(WIDTH, HEIGHT);
    panel.wait_for_scan();
    panel
}

fn painted(panel: &mut GitPanel) -> ui::Painted {
    let node = panel.render(WIDTH, HEIGHT);
    ui::render(
        &node,
        ui::Rect::new(0.0, 0.0, WIDTH, HEIGHT, ui::Rgba::TRANSPARENT),
    )
}

fn texts(panel: &mut GitPanel) -> String {
    painted(panel)
        .texts
        .iter()
        .map(|text| text.text.clone())
        .collect::<Vec<_>>()
        .join("|")
}

/// The innermost click target under the `nth` text reading `needle`.
fn id_at_nth(panel: &mut GitPanel, needle: &str, nth: usize) -> u64 {
    let painted = painted(panel);
    let text = painted
        .texts
        .iter()
        .filter(|text| text.text == needle)
        .nth(nth)
        .unwrap_or_else(|| {
            panic!(
                "no {needle:?} #{nth} in {:?}",
                painted
                    .texts
                    .iter()
                    .map(|text| &text.text)
                    .collect::<Vec<_>>()
            )
        });
    let (x, y) = (text.x + 1.0, text.y + text.size / 2.0);
    painted
        .hits
        .iter()
        .rev()
        .find(|(rect, _)| x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h)
        .map(|(_, id)| *id)
        .unwrap_or_else(|| panic!("{needle:?} is not clickable"))
}

fn id_at(panel: &mut GitPanel, needle: &str) -> u64 {
    id_at_nth(panel, needle, 0)
}

fn click(panel: &mut GitPanel, needle: &str) {
    let id = id_at(panel, needle);
    panel.click(id);
}

fn menu_pick(panel: &mut GitPanel, label: &str) {
    let item = panel
        .menu_items()
        .into_iter()
        .find(|item| item.label == label)
        .unwrap_or_else(|| panic!("{label:?} not in the menu"));
    panel.menu_action(item.id);
}

fn settle(panel: &mut GitPanel) -> Vec<PanelRequest> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut requests = Vec::new();
    loop {
        panel.render(WIDTH, HEIGHT);
        requests.extend(panel.take_requests());
        if !panel.operation_running() || std::time::Instant::now() > deadline {
            panel.wait_for_scan();
            return requests;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn staged(root: &Path) -> String {
    git_out(root, &["diff", "--cached", "--name-only"])
}

#[test]
fn ticking_in_not_staged_stages_and_unticking_in_staged_unstages() {
    let temp = tempfile::tempdir().expect("temp");
    let root = repo(temp.path(), "api", &["app.rs", "lib.rs"], "feat");
    write(&root, "app.rs", "one\ntwo\n");
    write(&root, "src/deep/new.rs", "a\n");
    let mut panel = panel(vec![source("api", &root, "feat")]);
    let shown = texts(&mut panel);
    assert!(shown.contains("Changes|(2)"), "{shown}");
    assert!(shown.contains("API"), "{shown}");
    assert!(
        shown.contains("Staged|0|Nothing staged yet - tick a file below|Not staged|2"),
        "{shown}"
    );
    assert!(
        shown.contains("src/deep"),
        "single-child folders fold: {shown}"
    );

    let app = id_at(&mut panel, "app.rs");
    panel.click(app + CHECK);
    panel.wait_for_scan();
    assert_eq!(staged(&root), "app.rs");
    let shown = texts(&mut panel);
    assert!(shown.contains("Staged|1"), "{shown}");

    write(&root, "app.rs", "one\ntwo\nthree\n");
    panel.refresh();
    panel.wait_for_scan();
    let shown = texts(&mut panel);
    assert_eq!(
        shown.matches("|app.rs|").count(),
        2,
        "a partly staged file shows in both sections: {shown}"
    );

    let in_staged = id_at_nth(&mut panel, "app.rs", 0);
    panel.click(in_staged + CHECK);
    panel.wait_for_scan();
    assert_eq!(staged(&root), "", "unticking in Staged unstages");

    let section = id_at(&mut panel, "Not staged");
    panel.click(section + CHECK);
    panel.wait_for_scan();
    assert_eq!(
        staged(&root),
        "app.rs\nsrc/deep/new.rs",
        "the section acts on the repo"
    );

    let folder = id_at_nth(&mut panel, "src/deep", 0);
    panel.click(folder + CHECK);
    panel.wait_for_scan();
    assert_eq!(staged(&root), "app.rs", "a folder unstages as one");

    click(&mut panel, "Staged");
    assert!(
        !texts(&mut panel).contains("Staged|1|app.rs"),
        "the section folds"
    );
}

#[test]
fn one_commit_per_included_repo_with_the_same_message() {
    let temp = tempfile::tempdir().expect("temp");
    let api = repo(temp.path(), "api", &["a.rs"], "feat");
    let web = repo(temp.path(), "web", &["w.ts"], "feat");
    let docs = repo(temp.path(), "docs", &["d.md"], "feat");
    for (root, file) in [(&api, "a.rs"), (&web, "w.ts"), (&docs, "d.md")] {
        write(root, file, "two\n");
        git(root, &["add", "."]);
    }
    let mut panel = panel(vec![
        source("api", &api, "feat"),
        source("web", &web, "feat"),
        source("docs", &docs, "feat"),
    ]);
    let shown = texts(&mut panel);
    assert!(shown.contains("3 commits: api 1, web 1, docs 1"), "{shown}");

    click(&mut panel, "3 commits: api 1, web 1, docs 1");
    assert!(matches!(
        panel.take_requests().as_slice(),
        [PanelRequest::OpenMenu]
    ));
    menu_pick(&mut panel, "docs (1 staged)");
    let shown = texts(&mut panel);
    assert!(shown.contains("2 commits: api 1, web 1"), "{shown}");

    let editor = id_at(&mut panel, "Enter commit message");
    panel.click_at(editor, 20.0, 12.0);
    assert!(panel.text_focused());
    panel.text("Share the login flow");
    panel.key(workspace::EditKey::ReplaceAll, false);
    let requests = settle(&mut panel);
    assert!(requests.is_empty(), "a commit that works says nothing");
    for root in [&api, &web] {
        assert_eq!(
            git_out(root, &["log", "-1", "--format=%s"]),
            "Share the login flow"
        );
        assert_eq!(git_out(root, &["rev-list", "--count", "main..HEAD"]), "1");
    }
    assert_eq!(
        git_out(&docs, &["log", "-1", "--format=%s"]),
        "base",
        "left out"
    );
    assert_eq!(staged(&docs), "d.md");

    let shown = texts(&mut panel);
    assert!(shown.contains("Last commit"), "{shown}");
    click(&mut panel, "Share the login flow - web");
    let shown = texts(&mut panel);
    assert!(
        shown.contains("w.ts") && shown.contains("not pushed yet"),
        "{shown}"
    );
    let file = id_at(&mut panel, "w.ts");
    panel.click(file);
    assert!(matches!(
        panel.take_requests().as_slice(),
        [PanelRequest::OpenItem(_)]
    ));

    click(&mut panel, "Changes");
    let undo = texts(&mut panel);
    assert!(undo.contains("Last commit"), "{undo}");
}

#[test]
fn uncommit_keeps_the_changes_staged() {
    let temp = tempfile::tempdir().expect("temp");
    let root = repo(temp.path(), "api", &["a.rs"], "feat");
    write(&root, "a.rs", "two\n");
    git(&root, &["commit", "-q", "-am", "Change a"]);
    let mut panel = panel(vec![source("api", &root, "feat")]);
    let last = id_at(&mut panel, "Change a - api");
    // The uncommit button follows the subject.
    panel.click(last + 1);
    panel.wait_for_scan();
    assert_eq!(git_out(&root, &["log", "-1", "--format=%s"]), "base");
    assert_eq!(staged(&root), "a.rs");
    let requests = panel.take_requests();
    assert!(
        matches!(requests.as_slice(), [PanelRequest::Toast(message)] if message.contains("staged again"))
    );
}

#[test]
fn the_tabs_show_changes_remote_and_history() {
    let temp = tempfile::tempdir().expect("temp");
    let root = repo(temp.path(), "api", &["a.rs"], "feat");
    write(&root, "a.rs", "two\n");
    git(&root, &["commit", "-q", "-am", "Change a"]);
    let mut panel = panel(vec![source("api", &root, "feat")]);
    assert_eq!(panel.tab(), Tab::Changes);
    assert!(texts(&mut panel).contains("Nothing to commit in api"));

    click(&mut panel, "Remote");
    assert_eq!(panel.tab(), Tab::Remote);
    let shown = texts(&mut panel);
    assert!(shown.contains("Fetch All"), "{shown}");
    assert!(shown.contains("0 of 1 files reviewed"), "{shown}");
    assert!(shown.contains("Publish"), "{shown}");
    assert!(shown.contains("Not published"), "{shown}");
    assert!(shown.contains("Files changed on this branch"), "{shown}");

    click(&mut panel, "History");
    assert_eq!(panel.tab(), Tab::History);
    let shown = texts(&mut panel);
    assert!(
        shown.contains("1 commit since the branch left main"),
        "{shown}"
    );
    assert!(shown.contains("Change a"), "{shown}");
    assert!(shown.contains("1 not pushed"), "{shown}");
    click(&mut panel, "1 not pushed");
    assert_eq!(panel.tab(), Tab::Remote);
}

#[test]
fn history_groups_by_repo_or_runs_as_one_timeline() {
    let temp = tempfile::tempdir().expect("temp");
    let api = repo(temp.path(), "api", &["a.rs"], "feat");
    let web = repo(temp.path(), "web", &["w.ts"], "feat");
    for (root, message) in [(&api, "Api one"), (&api, "Api two"), (&web, "Web one")] {
        write(root, "extra.txt", message);
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", message]);
    }
    let mut panel = panel(vec![
        source("api", &api, "feat"),
        source("web", &web, "feat"),
    ]);
    panel.set_tab(Tab::History);
    let shown = texts(&mut panel);
    assert!(
        shown.contains("3 commits since the branch left main"),
        "{shown}"
    );
    let api_at = shown.find("API").expect("api group");
    let web_at = shown.find("WEB").expect("web group");
    assert!(api_at < shown.find("Api two").expect("api commit"));
    assert!(shown.find("Api one") < Some(web_at), "{shown}");
    assert!(web_at < shown.find("Web one").expect("web commit"));

    click(&mut panel, "API");
    let shown = texts(&mut panel);
    assert!(!shown.contains("Api two"), "the group folds: {shown}");

    click(&mut panel, "API");
    let options = painted(&mut panel)
        .hits
        .iter()
        .map(|(_, id)| *id)
        .find(|id| *id == workspace::side_panel_base(workspace::PaneKind::Git) + 9_000_004)
        .expect("view options");
    panel.click(options);
    menu_pick(&mut panel, "One Timeline");
    let shown = texts(&mut panel);
    assert!(shown.contains("Today"), "{shown}");
    assert!(!shown.contains("API"), "{shown}");
    assert_eq!(
        shown.matches("|api|").count(),
        2,
        "each row names its repo: {shown}"
    );

    click(&mut panel, "Web one");
    let shown = texts(&mut panel);
    assert!(
        shown.contains("extra.txt"),
        "the commit opens with its files: {shown}"
    );
    click(&mut panel, "History");
    assert!(texts(&mut panel).contains("Api two"));
}

#[test]
fn remote_lists_commits_not_pushed_and_not_pulled() {
    let temp = tempfile::tempdir().expect("temp");
    let root = repo(temp.path(), "api", &["a.rs"], "feat");
    let remote = publish(temp.path(), &root, "api");
    git(&root, &["config", "pull.rebase", "false"]);
    write(&root, "a.rs", "two\n");
    git(&root, &["commit", "-q", "-am", "Mine to push"]);
    let other = temp.path().join("other");
    git(
        temp.path(),
        &[
            "clone",
            "-q",
            "-b",
            "feat",
            &remote.to_string_lossy(),
            "other",
        ],
    );
    git(&other, &["config", "commit.gpgsign", "false"]);
    write(&other, "b.rs", "theirs\n");
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "Theirs to pull"]);
    git(&other, &["push", "-q", "origin", "feat"]);

    let mut panel = panel(vec![source("api", &root, "feat")]);
    panel.set_tab(Tab::Remote);
    let shown = texts(&mut panel);
    assert!(
        shown.contains("Not pushed") && shown.contains("Mine to push"),
        "{shown}"
    );
    assert!(
        !shown.contains("Theirs to pull"),
        "not fetched yet: {shown}"
    );

    click(&mut panel, "Fetch All");
    settle(&mut panel);
    let shown = texts(&mut panel);
    assert!(shown.contains("On origin, not pulled"), "{shown}");
    assert!(shown.contains("Theirs to pull"), "{shown}");
    assert!(shown.contains("Sync"), "one ahead and one behind: {shown}");
    assert!(shown.contains("1 repo differs from origin"), "{shown}");

    let card = id_at(&mut panel, "api");
    panel.click(card + CHIP);
    let requests = settle(&mut panel);
    assert!(!requests.is_empty(), "the sync reports what it did");
    assert_eq!(
        git_out(&root, &["status", "-sb"]).lines().next(),
        Some("## feat...origin/feat")
    );
    assert!(texts(&mut panel).contains("Every repo matches origin"));
}

#[test]
fn the_branch_row_points_out_repos_on_another_branch() {
    let temp = tempfile::tempdir().expect("temp");
    let api = repo(temp.path(), "api", &["a.rs"], "feat-login");
    let web = repo(temp.path(), "web", &["w.ts"], "ana/mail-retry");
    let mut panel = panel(vec![
        source("api", &api, "feat-login"),
        source("web", &web, "feat-login"),
    ]);
    let shown = texts(&mut panel);
    assert!(shown.contains("feat-login| in 1 of 2"), "{shown}");
    assert!(shown.contains("|web on ana/mail"), "{shown}");

    click(&mut panel, "feat-login");
    assert!(matches!(
        panel.take_requests().as_slice(),
        [PanelRequest::OpenMenu]
    ));
    let labels: Vec<String> = panel
        .menu_items()
        .into_iter()
        .map(|item| item.label.into_owned())
        .collect();
    assert!(
        labels.contains(&"Keep ana/mail-retry for web".to_string()),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"Switch web to feat-login".to_string()),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"Copy Branch Name".to_string()),
        "{labels:?}"
    );
    let pick = panel
        .menu_items()
        .into_iter()
        .find(|item| item.label.starts_with("api "))
        .expect("a row per repo");
    panel.menu_action(pick.id);
    assert!(matches!(
        panel.take_requests().as_slice(),
        [PanelRequest::PickBranch { repo }] if repo == "api"
    ));

    click(&mut panel, "feat-login");
    panel.take_requests();
    menu_pick(&mut panel, "Switch web to feat-login");
    assert!(matches!(
        panel.take_requests().as_slice(),
        [PanelRequest::SwitchBranch { repo }] if repo == "web"
    ));

    click(&mut panel, "feat-login");
    panel.take_requests();
    menu_pick(&mut panel, "Keep ana/mail-retry for web");
    assert!(matches!(
        panel.take_requests().as_slice(),
        [PanelRequest::KeepBranch { repo, branch }] if repo == "web" && branch == "ana/mail-retry"
    ));
    let shown = texts(&mut panel);
    assert!(shown.contains("feat-login| + 1 other"), "{shown}");
    assert!(
        !shown.contains("web on ana/mail"),
        "kept is not drift: {shown}"
    );
}

#[test]
fn discards_after_confirming() {
    let temp = tempfile::tempdir().expect("temp");
    let root = repo(temp.path(), "api", &["app.rs"], "feat");
    write(&root, "app.rs", "one\ntwo\n");
    let mut panel = panel(vec![source("api", &root, "feat")]);
    let file = id_at(&mut panel, "app.rs");
    panel.click(file);
    assert!(matches!(
        panel.take_requests().as_slice(),
        [PanelRequest::OpenDiff { path, base: Some(base) }] if path.ends_with("app.rs") && base == "one\n"
    ));
    assert!(panel.open_menu(file));
    menu_pick(&mut panel, "Discard Uncommitted Changes");
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
    assert!(panel.open_menu(file));
    menu_pick(&mut panel, "Discard Uncommitted Changes");
    let Some(PanelRequest::Prompt { tag, .. }) = panel.take_requests().into_iter().next() else {
        panic!("asks again");
    };
    panel.prompt_answered(tag, 0);
    assert_eq!(
        std::fs::read_to_string(root.join("app.rs")).expect("read"),
        "one\n"
    );
    panel.wait_for_scan();
    assert!(texts(&mut panel).contains("Nothing to commit in api"));
}

#[test]
fn review_marks_hold_until_the_file_changes_and_survive_reopening() {
    let temp = tempfile::tempdir().expect("temp");
    let root = repo(temp.path(), "api", &["a.rs", "b.rs"], "feat");
    write(&root, "a.rs", "2\n");
    write(&root, "b.rs", "2\n");
    let reviews = temp.path().join("state/reviews/demo-feat.json");
    let open = || {
        let mut panel = GitPanel::new(
            vec![source("api", &root, "feat")],
            Some(reviews.clone()),
            Arc::new(|| {}),
        );
        panel.render(WIDTH, HEIGHT);
        panel.wait_for_scan();
        panel.set_tab(Tab::Remote);
        panel
    };
    let mut panel = open();
    assert!(texts(&mut panel).contains("0 of 2 files reviewed"));
    let a = id_at(&mut panel, "a.rs");
    panel.click(a + EYE);
    assert!(texts(&mut panel).contains("1 of 2 files reviewed"));
    drop(panel);

    let mut panel = open();
    assert!(
        texts(&mut panel).contains("1 of 2 files reviewed"),
        "marks are kept"
    );
    write(&root, "a.rs", "3\n");
    panel.refresh();
    panel.wait_for_scan();
    assert!(
        texts(&mut panel).contains("0 of 2 files reviewed"),
        "an edit after the review clears the mark"
    );
}

#[test]
fn publishes_from_the_card_and_a_failed_push_offers_its_log() {
    let temp = tempfile::tempdir().expect("temp");
    let root = repo(temp.path(), "api", &["app.rs"], "main");
    publish(temp.path(), &root, "api");
    git(&root, &["checkout", "-q", "-b", "feat"]);
    write(&root, "app.rs", "two\n");
    git(&root, &["commit", "-q", "-am", "Change"]);
    let broken = repo(temp.path(), "web", &["w.ts"], "feat");
    git(
        &broken,
        &["remote", "add", "origin", "/nonexistent/remote.git"],
    );
    let mut panel = panel(vec![
        source("api", &root, "feat"),
        source("web", &broken, "feat"),
    ]);
    panel.set_tab(Tab::Remote);
    let chip = id_at_nth(&mut panel, "Publish", 0);
    panel.click(chip);
    let requests = settle(&mut panel);
    assert!(
        matches!(
            requests.as_slice(),
            [PanelRequest::ToastAction { message, action, .. }]
                if message == "Pushed feat to origin" && action == "View Log"
        ),
        "{}",
        requests.len()
    );
    assert_eq!(
        git_out(&root, &["rev-parse", "--abbrev-ref", "feat@{upstream}"]),
        "origin/feat"
    );
    let shown = texts(&mut panel);
    assert!(shown.contains("Up to date"), "{shown}");

    let chip = id_at(&mut panel, "Publish");
    panel.click(chip);
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
fn a_pull_request_block_opens_it_in_the_app() {
    let temp = tempfile::tempdir().expect("temp");
    let root = repo(temp.path(), "web", &["app.rs"], "feat");
    let prs = pull_request_ui::PullRequests::new(
        pom_paths::StateDir::new(temp.path().join("state")),
        "myproject",
        Arc::new(|| {}),
    );
    let target = pom_forge::PrTarget {
        repo: "web".into(),
        owner: "acme".into(),
        name: "web".into(),
        head: "feat".into(),
    };
    prs.show(
        "feat",
        vec![(
            root.clone(),
            target,
            pom_forge::PullRequest {
                number: 42,
                title: "Login page".into(),
                state: "OPEN".into(),
                checks: "fail".into(),
                url: "https://example.com/acme/web/pull/42".into(),
                ..pom_forge::PullRequest::default()
            },
        )],
    );
    let mut panel = GitPanel::new(vec![source("web", &root, "feat")], None, Arc::new(|| {}))
        .with_pull_requests(prs);
    panel.render(WIDTH, HEIGHT);
    panel.wait_for_scan();
    panel.set_tab(Tab::Remote);
    let shown = texts(&mut panel);
    assert!(
        shown.contains("#42 Login page|CI failed|Review pending"),
        "{shown}"
    );
    click(&mut panel, "#42 Login page");
    assert!(matches!(
        panel.take_requests().as_slice(),
        [PanelRequest::Reveal { id, .. }] if id == "pr:web:feat"
    ));
}
