use super::*;

fn repo_with(root: &Path, lock: &str, installed: bool) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("package-lock.json"), lock).unwrap();
    if installed {
        fs::create_dir_all(root.join("node_modules/left-pad")).unwrap();
        fs::write(root.join("node_modules/left-pad/index.js"), "pad").unwrap();
        fs::create_dir_all(root.join("node_modules/.cache")).unwrap();
        fs::write(root.join("node_modules/.cache/build"), "cache").unwrap();
    }
    root.to_path_buf()
}

fn options(fallback: Fallback) -> Options {
    Options {
        fallback,
        ..Options::default()
    }
}

#[test]
fn a_workspace_matching_main_gets_mains_modules_without_its_cache() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    let main = repo_with(&dir.path().join("main"), "v1", true);
    let worktree = repo_with(&dir.path().join("feat"), "v1", false);
    let restored = store
        .restore(
            "api",
            &worktree,
            &main,
            Some("v20.1.0"),
            &options(Fallback::Copy),
        )
        .unwrap();
    assert!(matches!(restored, Restored::FromStore(_)), "{restored:?}");
    assert_eq!(
        fs::read_to_string(worktree.join("node_modules/left-pad/index.js")).unwrap(),
        "pad"
    );
    assert!(!worktree.join("node_modules/.cache").exists());
    let entries = store.list().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].manager, "npm");
    assert_eq!(entries[0].live_workspaces(), [&worktree]);
    assert!(entries[0].size > 0);
    assert!(
        fs::read_to_string(main.join("node_modules/left-pad/index.js")).is_ok()
            && fs::write(main.join("node_modules/left-pad/index.js"), "still mine").is_ok(),
        "main's own files stay writable"
    );
}

#[test]
fn a_different_lockfile_or_node_major_installs_and_then_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    let main = repo_with(&dir.path().join("main"), "v1", true);
    let worktree = repo_with(&dir.path().join("feat"), "v2", false);
    let copy = options(Fallback::Copy);
    assert_eq!(
        store
            .restore("api", &worktree, &main, Some("v20"), &copy)
            .unwrap(),
        Restored::Install
    );
    let other_node = repo_with(&dir.path().join("other"), "v1", false);
    assert_eq!(
        store
            .restore("api", &other_node, &main, Some("v22"), &copy)
            .unwrap(),
        Restored::FromStore(store.supported_method(dir.path()).max_copy()),
        "a new node major fills its own copy from main when main's key matches too"
    );
    repo_with(&worktree, "v2", true);
    store
        .snapshot("api", &worktree, Some("v20"), &copy)
        .unwrap();
    let next = repo_with(&dir.path().join("next"), "v2", false);
    assert!(matches!(
        store
            .restore("api", &next, &main, Some("v20"), &copy)
            .unwrap(),
        Restored::FromStore(_)
    ));
    assert_eq!(store.list().unwrap().len(), 2);
}

impl Method {
    fn max_copy(self) -> Method {
        if self == Method::Clone {
            Method::Clone
        } else {
            Method::Copy
        }
    }
}

#[test]
fn run_install_fallback_and_disabled_store_leave_the_install_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    let main = repo_with(&dir.path().join("main"), "v1", true);
    let worktree = repo_with(&dir.path().join("feat"), "v1", false);
    let off = Options {
        enabled: false,
        ..Options::default()
    };
    assert_eq!(
        store.restore("api", &worktree, &main, None, &off).unwrap(),
        Restored::Install
    );
    if store.supported_method(dir.path()) != Method::Clone {
        assert_eq!(
            store
                .restore("api", &worktree, &main, None, &options(Fallback::Install))
                .unwrap(),
            Restored::Install
        );
    }
    fs::write(worktree.join("pnpm-lock.yaml"), "").unwrap();
    assert_eq!(
        store
            .restore("api", &worktree, &main, None, &Options::default())
            .unwrap(),
        Restored::SelfManaged("pnpm")
    );
}

#[test]
fn the_outlook_tells_instant_from_installing_before_create() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    let installed = repo_with(&dir.path().join("api"), "v1", true);
    let bare = repo_with(&dir.path().join("web"), "v1", false);
    let on = options(Fallback::Copy);
    assert_eq!(
        store.outlook("api", &installed, None, &on),
        Outlook::FromStore
    );
    assert_eq!(
        store.outlook("web", &bare, None, &on),
        Outlook::InstallsOnce
    );
    let off = Options {
        enabled: false,
        ..on
    };
    assert_eq!(
        store.outlook("api", &installed, None, &off),
        Outlook::Installs
    );
    fs::write(bare.join("pnpm-lock.yaml"), "").unwrap();
    assert_eq!(
        store.outlook("web", &bare, None, &on),
        Outlook::SelfManaged("pnpm")
    );
    let empty = dir.path().join("docs");
    fs::create_dir_all(&empty).unwrap();
    assert_eq!(store.outlook("docs", &empty, None, &on), Outlook::NotNode);
}

#[test]
fn hard_linked_copies_are_read_only_and_shared() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    let first = repo_with(&dir.path().join("first"), "v1", true);
    let hard = Options {
        fallback: Fallback::HardLink,
        ..Options::default()
    };
    store
        .fill(
            "api",
            "k",
            Manager::Npm,
            &first.join("node_modules"),
            Method::HardLink,
        )
        .unwrap();
    let stored = store.entry_dir("api", "k");
    let second = dir.path().join("second/node_modules");
    import::import(&stored, &second, Method::HardLink, &[]).unwrap();
    let file = "left-pad/index.js";
    assert_eq!(
        fs::metadata(second.join(file)).unwrap().ino(),
        fs::metadata(stored.join(file)).unwrap().ino()
    );
    assert!(fs::write(second.join(file), "edited in place").is_err());
    assert_eq!(fs::read_to_string(stored.join(file)).unwrap(), "pad");
    // Reinstalling replaces files rather than editing them, which read-only files allow.
    fs::remove_file(second.join(file)).unwrap();
    fs::write(second.join(file), "new version").unwrap();
    assert_eq!(fs::read_to_string(stored.join(file)).unwrap(), "pad");
    assert!(hard.enabled);
}

#[test]
fn prune_drops_expired_copies_then_the_oldest_over_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    let source = repo_with(&dir.path().join("source"), "v1", true);
    for key in ["old", "mid", "new"] {
        store
            .fill(
                "api",
                key,
                Manager::Npm,
                &source.join("node_modules"),
                Method::Copy,
            )
            .unwrap();
    }
    let stamp = now();
    store
        .update(|index| {
            for entry in &mut index.entries {
                entry.last_used = match entry.key.as_str() {
                    "old" => stamp - 30 * DAY,
                    "mid" => stamp - DAY,
                    _ => stamp,
                };
            }
        })
        .unwrap();
    let pruned = store.prune(&Options::default()).unwrap();
    assert_eq!(pruned.removed, 1);
    let keys: Vec<String> = store.list().unwrap().into_iter().map(|e| e.key).collect();
    assert_eq!(keys, ["new", "mid"]);
    store
        .update(|index| {
            for entry in &mut index.entries {
                entry.size = 15 << 30;
            }
        })
        .unwrap();
    store.prune(&Options::default()).unwrap();
    let keys: Vec<String> = store.list().unwrap().into_iter().map(|e| e.key).collect();
    assert_eq!(keys, ["new"]);
    assert!(!dir.path().join("store/api/old").exists());
    let cleared = store.clear().unwrap();
    assert_eq!(cleared.removed, 1);
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn copies_from_before_the_index_are_adopted() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("store");
    fs::create_dir_all(root.join("web/0123456789abcdef/node_modules/react")).unwrap();
    fs::write(
        root.join("web/0123456789abcdef/node_modules/react/index.js"),
        "react",
    )
    .unwrap();
    let entries = Store::at(&root).list().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].repo, "web");
    assert_eq!(entries[0].manager, "");
    assert_eq!(entries[0].size, 5);
}

#[test]
fn options_read_the_settings_keys_and_their_defaults_match() {
    let settings = settings::Settings::default();
    let value = serde_json::to_value(&settings).unwrap();
    assert_eq!(Options::from_settings(&value), Options::default());
    let value = serde_json::json!({
        "modules_store_enabled": false,
        "modules_fallback": "install",
        "modules_size_limit_gb": 5,
        "modules_unused_days": 0,
    });
    assert_eq!(
        Options::from_settings(&value),
        Options {
            enabled: false,
            fallback: Fallback::Install,
            size_limit_gb: 5,
            unused_days: 0,
        }
    );
    assert_eq!(
        Options::default().method(Method::HardLink),
        Some(Method::HardLink)
    );
    assert_eq!(
        options(Fallback::Copy).method(Method::HardLink),
        Some(Method::Copy)
    );
    assert_eq!(options(Fallback::Install).method(Method::Copy), None);
    assert_eq!(
        options(Fallback::Install).method(Method::Clone),
        Some(Method::Clone)
    );
    assert_eq!(format_size(3 << 30), "3.0 GB");
    assert_eq!(format_size(5 << 20), "5 MB");
}
