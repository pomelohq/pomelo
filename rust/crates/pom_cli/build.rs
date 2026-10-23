//! `POM_VERSION`: the workspace version for a release build, `<version>-dev+<commit>` for any other, so a binary
//! built by hand never reads as newer than the release it came from.

use std::path::PathBuf;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|text| !text.is_empty())
}

fn watch_git() {
    let (Some(git_dir), Some(common_dir)) = (
        git(&["rev-parse", "--absolute-git-dir"]),
        git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]),
    ) else {
        return;
    };
    let git_dir = PathBuf::from(git_dir);
    let common_dir = PathBuf::from(common_dir);
    let mut watched = vec![
        git_dir.join("HEAD"),
        common_dir.join("packed-refs"),
        common_dir.join("refs/tags"),
    ];
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watched.push(common_dir.join(reference));
    }
    for path in watched.iter().filter(|path| path.exists()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=POM_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=GITHUB_WORKFLOW");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let explicit = std::env::var("POM_BUILD_VERSION")
        .ok()
        .filter(|value| !value.is_empty());
    let release_workflow = std::env::var("GITHUB_WORKFLOW").is_ok_and(|name| name == "Release");
    watch_git();
    let tag = format!("v{version}");
    let tagged = git(&["tag", "--points-at", "HEAD"])
        .is_some_and(|tags| tags.lines().any(|line| line.trim() == tag));
    let reported = match explicit {
        Some(explicit) => explicit,
        None if release_workflow || tagged => version,
        None => match git(&["rev-parse", "--short=9", "HEAD"]) {
            Some(commit) => format!("{version}-dev+{commit}"),
            None => format!("{version}-dev"),
        },
    };
    println!("cargo:rustc-env=POM_VERSION={reported}");
}
