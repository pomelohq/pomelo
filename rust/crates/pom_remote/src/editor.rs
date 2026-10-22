//! Opening a remote workspace's folder in an editor that works over SSH, on this machine.

use std::path::{Path, PathBuf};

/// The command that opens `path` on the host reached as `alias` (`user@host[:port]`). `setting` is the
/// `remote_editor` template (`{host}`, `{path}` and `{url}` are filled in, each word stays one argument); empty
/// tries `zed ssh://<alias>/<path>`, then `code --remote ssh-remote+<alias> <path>`. `find` resolves a program.
pub fn editor_argv(
    setting: &str,
    alias: &str,
    path: &str,
    find: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<Vec<String>, String> {
    let url = format!("ssh://{alias}/{}", path.trim_start_matches('/'));
    let resolve = |program: &str| -> String {
        find(program).map_or_else(|| program.to_string(), |found| found.display().to_string())
    };
    let setting = setting.trim();
    if !setting.is_empty() {
        let mut words = setting.split_whitespace().map(|word| {
            word.replace("{host}", alias)
                .replace("{path}", path)
                .replace("{url}", &url)
        });
        let program = words.next().ok_or("remote_editor is empty")?;
        let mut argv = vec![resolve(&program)];
        argv.extend(words);
        return Ok(argv);
    }
    if let Some(zed) = find("zed") {
        return Ok(vec![zed.display().to_string(), url]);
    }
    if let Some(code) = find("code") {
        return Ok(vec![
            code.display().to_string(),
            "--remote".into(),
            format!("ssh-remote+{alias}"),
            path.to_string(),
        ]);
    }
    Err(
        "no editor that opens folders over SSH was found (zed or code on PATH); set remote_editor in settings.json"
            .into(),
    )
}

/// `program` in the folders of `search_path`, then where installers put command-line launchers.
pub fn find_program(program: &str, search_path: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program)).filter(|path| path.is_file());
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = std::env::split_paths(search_path).collect();
    dirs.extend(home.map(|home| home.join(".local/bin")));
    dirs.push("/opt/homebrew/bin".into());
    dirs.push("/usr/local/bin".into());
    dirs.push("/usr/bin".into());
    dirs.into_iter()
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(names: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |program| {
            names
                .contains(&program)
                .then(|| PathBuf::from(format!("/usr/local/bin/{program}")))
        }
    }

    #[test]
    fn the_default_prefers_zed_then_code() {
        let path = "/home/dev/pom/myproject/workspace--feat-login";
        assert_eq!(
            editor_argv(
                "",
                "dev@dev.example.net:2222",
                path,
                &only(&["zed", "code"])
            ),
            Ok(vec![
                "/usr/local/bin/zed".to_string(),
                "ssh://dev@dev.example.net:2222/home/dev/pom/myproject/workspace--feat-login"
                    .into(),
            ])
        );
        assert_eq!(
            editor_argv("", "dev@dev.example.net", path, &only(&["code"])),
            Ok(vec![
                "/usr/local/bin/code".to_string(),
                "--remote".into(),
                "ssh-remote+dev@dev.example.net".into(),
                path.into(),
            ])
        );
        assert!(editor_argv("", "dev", path, &only(&[])).is_err());
    }

    #[test]
    fn a_template_fills_each_word_and_keeps_a_path_with_spaces_whole() {
        let argv = editor_argv(
            "cursor --remote ssh-remote+{host} {path}",
            "me@100.64.0.10",
            "/home/me/my project",
            &only(&["cursor"]),
        );
        assert_eq!(
            argv,
            Ok(vec![
                "/usr/local/bin/cursor".to_string(),
                "--remote".into(),
                "ssh-remote+me@100.64.0.10".into(),
                "/home/me/my project".into(),
            ])
        );
        assert_eq!(
            editor_argv("myeditor {url}", "dev", "/srv/x", &only(&[])),
            Ok(vec!["myeditor".to_string(), "ssh://dev/srv/x".into()])
        );
    }

    #[test]
    fn programs_are_found_only_when_executable() {
        let temp = tempfile::tempdir().expect("temp");
        let program = temp.path().join("fake-editor");
        std::fs::write(&program, "#!/bin/sh\n").expect("write");
        let search = temp.path().to_string_lossy().into_owned();
        assert_eq!(find_program("fake-editor", &search), None);
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(find_program("fake-editor", &search), Some(program));
    }
}
