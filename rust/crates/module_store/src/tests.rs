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
            &RepoRules::default(),
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
            .restore(
                "api",
                &worktree,
                &main,
                Some("v20"),
                &copy,
                &RepoRules::default()
            )
            .unwrap(),
        Restored::Install
    );
    let other_node = repo_with(&dir.path().join("other"), "v1", false);
    assert_eq!(
        store
            .restore(
                "api",
                &other_node,
                &main,
                Some("v22"),
                &copy,
                &RepoRules::default()
            )
            .unwrap(),
        Restored::FromStore(store.supported_method(dir.path()).max_copy()),
        "a new node major fills its own copy from main when main's key matches too"
    );
    repo_with(&worktree, "v2", true);
    store
        .snapshot("api", &worktree, Some("v20"), &copy, &RepoRules::default())
        .unwrap();
    let next = repo_with(&dir.path().join("next"), "v2", false);
    assert!(matches!(
        store
            .restore(
                "api",
                &next,
                &main,
                Some("v20"),
                &copy,
                &RepoRules::default()
            )
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
        store
            .restore("api", &worktree, &main, None, &off, &RepoRules::default())
            .unwrap(),
        Restored::Install
    );
    if store.supported_method(dir.path()) != Method::Clone {
        assert_eq!(
            store
                .restore(
                    "api",
                    &worktree,
                    &main,
                    None,
                    &options(Fallback::Install),
                    &RepoRules::default()
                )
                .unwrap(),
            Restored::Install
        );
    }
    fs::write(worktree.join("pnpm-lock.yaml"), "").unwrap();
    assert_eq!(
        store
            .restore(
                "api",
                &worktree,
                &main,
                None,
                &Options::default(),
                &RepoRules::default()
            )
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
        store.outlook("api", &installed, None, &on, &RepoRules::default()),
        Outlook::FromStore
    );
    assert_eq!(
        store.outlook("web", &bare, None, &on, &RepoRules::default()),
        Outlook::InstallsOnce
    );
    let off = Options {
        enabled: false,
        ..on
    };
    assert_eq!(
        store.outlook("api", &installed, None, &off, &RepoRules::default()),
        Outlook::Installs
    );
    fs::write(bare.join("pnpm-lock.yaml"), "").unwrap();
    assert_eq!(
        store.outlook("web", &bare, None, &on, &RepoRules::default()),
        Outlook::SelfManaged("pnpm")
    );
    let empty = dir.path().join("docs");
    fs::create_dir_all(&empty).unwrap();
    assert_eq!(
        store.outlook("docs", &empty, None, &on, &RepoRules::default()),
        Outlook::NotNode
    );
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
            &[],
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
                &[],
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
fn a_scheduled_prune_runs_at_most_once_a_day_and_not_when_the_store_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    assert_eq!(store.prune_if_due(&Options::default()).unwrap(), None);
    let source = repo_with(&dir.path().join("source"), "v1", true);
    store
        .fill(
            "api",
            "old",
            Manager::Npm,
            &source.join("node_modules"),
            Method::Copy,
            &[],
        )
        .unwrap();
    let stamp = now();
    store
        .update(|index| index.entries[0].last_used = stamp - 30 * DAY)
        .unwrap();
    let off = Options {
        enabled: false,
        ..Options::default()
    };
    assert_eq!(store.prune_if_due(&off).unwrap(), None);
    let pruned = store
        .prune_if_due_at(&Options::default(), stamp)
        .unwrap()
        .expect("due");
    assert_eq!(pruned.removed, 1);
    assert_eq!(
        store
            .prune_if_due_at(&Options::default(), stamp + DAY - 1)
            .unwrap(),
        None
    );
    assert!(store
        .prune_if_due_at(&Options::default(), stamp + DAY)
        .unwrap()
        .is_some());
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

fn with_generated_client(root: &Path) {
    fs::create_dir_all(root.join("node_modules/.prisma/client")).unwrap();
    fs::write(
        root.join("node_modules/.prisma/client/index.js"),
        "main's client",
    )
    .unwrap();
    fs::create_dir_all(root.join("node_modules/.generated")).unwrap();
    fs::write(root.join("node_modules/.generated/out.js"), "out").unwrap();
}

#[test]
fn generated_client_folders_are_never_stored_or_linked() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    let main = repo_with(&dir.path().join("main"), "v1", false);
    let source = repo_with(&dir.path().join("source"), "v1", true);
    with_generated_client(&source);
    let hard = options(Fallback::HardLink);
    store
        .snapshot("api", &source, None, &hard, &RepoRules::default())
        .unwrap();
    let key = store.list().unwrap()[0].key.clone();
    let stored = store.entry_dir("api", &key);
    assert!(stored.join("left-pad/index.js").is_file());
    assert!(!stored.join(".prisma").exists());
    assert!(
        stored.join(".generated/out.js").is_file(),
        "only listed folders are private"
    );

    let first = repo_with(&dir.path().join("first"), "v1", false);
    let second = repo_with(&dir.path().join("second"), "v1", false);
    for worktree in [&first, &second] {
        let restored = store
            .restore("api", worktree, &main, None, &hard, &RepoRules::default())
            .unwrap();
        assert!(matches!(restored, Restored::FromStore(_)), "{restored:?}");
        assert_eq!(
            fs::read_to_string(worktree.join("node_modules/left-pad/index.js")).unwrap(),
            "pad"
        );
        assert!(!worktree.join("node_modules/.prisma").exists());
    }
    let client = first.join("node_modules/.prisma/client");
    fs::create_dir_all(&client).unwrap();
    fs::write(client.join("index.js"), "first's client").unwrap();
    assert!(!stored.join(".prisma").exists());
    assert!(!second.join("node_modules/.prisma").exists());
}

#[test]
fn a_repos_modules_private_folders_stay_out_of_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    let main = repo_with(&dir.path().join("main"), "v1", true);
    with_generated_client(&main);
    let worktree = repo_with(&dir.path().join("feat"), "v1", false);
    let private = vec![".generated".to_string(), "../escape".to_string()];
    store
        .restore(
            "api",
            &worktree,
            &main,
            None,
            &options(Fallback::Copy),
            &RepoRules {
                private,
                setup: Vec::new(),
            },
        )
        .unwrap();
    let key = store.list().unwrap()[0].key.clone();
    let stored = store.entry_dir("api", &key);
    for root in [&stored, &worktree.join("node_modules")] {
        assert!(root.join("left-pad/index.js").is_file());
        assert!(!root.join(".prisma").exists());
        assert!(!root.join(".generated").exists());
    }
    assert!(
        main.join("node_modules/.generated/out.js").is_file(),
        "main keeps its own"
    );
}

#[test]
fn a_copy_stored_before_a_folder_was_private_restores_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    let main = repo_with(&dir.path().join("main"), "v1", false);
    let source = repo_with(&dir.path().join("source"), "v1", true);
    store
        .snapshot(
            "api",
            &source,
            None,
            &options(Fallback::Copy),
            &RepoRules::default(),
        )
        .unwrap();
    let key = store.list().unwrap()[0].key.clone();
    let stored = store.entry_dir("api", &key);
    fs::create_dir_all(stored.join(".prisma/client")).unwrap();
    fs::write(stored.join(".prisma/client/index.js"), "someone's client").unwrap();
    fs::create_dir_all(stored.join(".generated")).unwrap();

    let worktree = repo_with(&dir.path().join("feat"), "v1", false);
    let rules = RepoRules {
        private: vec![".generated".to_string()],
        setup: Vec::new(),
    };
    for fallback in [Fallback::HardLink, Fallback::Copy] {
        remove_tree(&worktree.join("node_modules")).unwrap();
        store
            .restore("api", &worktree, &main, None, &options(fallback), &rules)
            .unwrap();
        assert!(worktree.join("node_modules/left-pad/index.js").is_file());
        assert!(
            !worktree.join("node_modules/.prisma").exists(),
            "{fallback:?}"
        );
        assert!(
            !worktree.join("node_modules/.generated").exists(),
            "{fallback:?}"
        );
    }
}

fn inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).unwrap().ino()
}

fn read_only(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o222 == 0
}

#[test]
fn private_packages_are_writable_copies_in_a_hard_linked_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    let source = repo_with(&dir.path().join("source"), "v1", true);
    let modules = source.join("node_modules");
    for package in ["@prisma/client/runtime", "prisma/build", "sdk"] {
        fs::create_dir_all(modules.join(package)).unwrap();
    }
    fs::write(modules.join("@prisma/client/runtime/library.js"), "runtime").unwrap();
    fs::write(modules.join("prisma/build/index.js"), "cli").unwrap();
    fs::write(modules.join("sdk/index.js"), "sdk").unwrap();
    let private = vec!["sdk".to_string()];
    store
        .fill(
            "api",
            "k",
            Manager::Npm,
            &modules,
            Method::HardLink,
            &private,
        )
        .unwrap();
    let stored = store.entry_dir("api", "k");
    let stored_runtime = stored.join("@prisma/client/runtime/library.js");
    assert!(read_only(&stored_runtime));
    assert_eq!(
        inode(&modules.join("left-pad/index.js")),
        inode(&stored.join("left-pad/index.js"))
    );
    assert_ne!(
        inode(&modules.join("@prisma/client/runtime/library.js")),
        inode(&stored_runtime),
        "the installing workspace keeps its own prisma"
    );
    assert!(!read_only(
        &modules.join("@prisma/client/runtime/library.js")
    ));

    let target = dir.path().join("feat/node_modules");
    restore_into(&stored, &target, Method::HardLink, &private).unwrap();
    assert_eq!(
        inode(&target.join("left-pad/index.js")),
        inode(&stored.join("left-pad/index.js"))
    );
    for file in [
        "@prisma/client/runtime/library.js",
        "prisma/build/index.js",
        "sdk/index.js",
    ] {
        assert_ne!(
            inode(&target.join(file)),
            inode(&stored.join(file)),
            "{file}"
        );
        assert!(!read_only(&target.join(file)), "{file}");
    }

    let runtime = target.join("@prisma/client/runtime/library.js");
    let generated = target.join(".prisma/client");
    fs::create_dir_all(&generated).unwrap();
    for round in 0..3 {
        let copied = generated.join("library.js");
        fs::copy(&runtime, &copied).unwrap_or_else(|error| panic!("generate {round}: {error}"));
        fs::set_permissions(&copied, fs::metadata(&runtime).unwrap().permissions()).unwrap();
    }
    fs::write(&runtime, "patched in the workspace").unwrap();
    assert_eq!(fs::read_to_string(&stored_runtime).unwrap(), "runtime");
    assert!(read_only(&stored_runtime));
    assert!(!stored.join(".prisma").exists());
}

#[test]
fn private_folders_starting_with_a_dot_are_never_stored_and_others_are() {
    let names = vec![
        ".generated".to_string(),
        "sdk".to_string(),
        "../up".to_string(),
    ];
    assert_eq!(unshared(&names), [".cache", ".prisma", ".generated"]);
    let dir = tempfile::tempdir().unwrap();
    for name in ["@prisma", "sdk", ".generated"] {
        fs::create_dir_all(dir.path().join(name)).unwrap();
    }
    assert_eq!(private_copies(dir.path(), &names), ["@prisma", "sdk"]);
}
