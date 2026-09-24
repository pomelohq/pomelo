//! Asking Claude for a workspace's display name and branch slug, from a seed slug and a description.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(25);
const NAME_MAX_CHARS: usize = 60;
/// Naming needs no tools; denying them keeps a one-shot prompt from touching anything.
const DENIED_TOOLS: &str = "Bash Edit Write Read Glob Grep WebFetch WebSearch NotebookEdit";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameSuggestion {
    pub name: String,
    pub slug: String,
}

pub fn naming_prompt(seed: &str, description: &str) -> String {
    let mut prompt = String::from(
        "You name developer workspaces. Return COMPACT JSON only, no prose:\n\
         {\"name\":\"<human title, Title Case, <=7 words, keep a leading ticket key UPPERCASE e.g. PROJ-1147>\",\
         \"slug\":\"<kebab-case, <=5 words, a-z0-9- only, keep the ticket key lowercase e.g. proj-1147>\"}\n",
    );
    prompt.push_str(&format!("Seed branch: {seed}\n"));
    if !description.trim().is_empty() {
        prompt.push_str(&format!("Description: {description}\n"));
    }
    prompt
}

/// The first `{` to the last `}` of Claude's answer, read as `{name, slug}`.
pub fn parse_suggestion(output: &str) -> Option<NameSuggestion> {
    let start = output.find('{')?;
    let end = output.rfind('}')?;
    let value: serde_json::Value = serde_json::from_str(output.get(start..=end)?).ok()?;
    let name: String = value["name"]
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .take(NAME_MAX_CHARS)
        .collect();
    let slug = ascii_slug(value["slug"].as_str().unwrap_or(""));
    (!name.is_empty() || !slug.is_empty()).then(|| NameSuggestion {
        name: name.trim().to_string(),
        slug,
    })
}

fn ascii_slug(text: &str) -> String {
    let mut slug = String::new();
    for c in text.trim().to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

/// Runs `claude -p` (from `claude`, e.g. a resolved path) on the naming prompt; gives up after 25 seconds.
pub fn suggest_name(
    claude: &str,
    tool_path: &str,
    seed: &str,
    description: &str,
) -> Result<NameSuggestion, String> {
    let mut child = Command::new(claude)
        .args([
            "-p",
            "--output-format",
            "text",
            "--disallowed-tools",
            DENIED_TOOLS,
        ])
        .current_dir(std::env::temp_dir())
        .env("PATH", tool_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not run Claude: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(naming_prompt(seed, description).as_bytes())
            .map_err(|error| format!("could not ask Claude: {error}"))?;
    }
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(stdout) = stdout.as_mut() {
            if let Err(error) = stdout.read_to_string(&mut text) {
                eprintln!("naming: read Claude's answer: {error}");
            }
        }
        text
    });
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                if let Err(error) = child.kill() {
                    eprintln!("naming: stop Claude: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("naming: reap Claude: {error}");
                }
                return Err("Claude took too long".into());
            }
            Err(error) => return Err(format!("Claude: {error}")),
        }
    };
    let output = reader.join().unwrap_or_default();
    if !status.success() {
        return Err("Claude could not suggest a name".into());
    }
    parse_suggestion(&output).ok_or_else(|| "Claude's answer had no name".into())
}

/// Whether a `claude` binary is where we would run it.
pub fn claude_available(claude: &str) -> bool {
    Path::new(claude).is_absolute() && Path::new(claude).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_matches_the_previous_core() {
        let prompt = naming_prompt("proj-101", "Fix the login page");
        assert!(prompt.starts_with("You name developer workspaces."));
        assert!(prompt.contains("\"slug\":\"<kebab-case"));
        assert!(prompt.ends_with("Seed branch: proj-101\nDescription: Fix the login page\n"));
        assert!(!naming_prompt("x", "  ").contains("Description"));
        assert!(prompt.is_ascii());
    }

    #[test]
    fn answers_are_read_leniently() {
        assert_eq!(
            parse_suggestion(
                "Sure! {\"name\":\"PROJ-101 Fix Login\",\"slug\":\"PROJ-101 fix_login!\"} done"
            ),
            Some(NameSuggestion {
                name: "PROJ-101 Fix Login".into(),
                slug: "proj-101-fix-login".into(),
            })
        );
        let long = format!("{{\"name\":\"{}\",\"slug\":\"a\"}}", "x".repeat(80));
        assert_eq!(parse_suggestion(&long).map(|s| s.name.len()), Some(60));
        assert_eq!(parse_suggestion("no json"), None);
        assert_eq!(parse_suggestion("{\"name\":\"\",\"slug\":\"\"}"), None);
    }

    #[test]
    fn a_fake_claude_is_asked_on_stdin() {
        let temp = tempfile::tempdir().expect("temp");
        let script = temp.path().join("claude");
        std::fs::write(
            &script,
            "#!/bin/sh\ngrep -q 'Seed branch: feat-x' && echo '{\"name\":\"Feat X\",\"slug\":\"feat-x\"}'\n",
        )
        .expect("script");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let claude = script.to_string_lossy().into_owned();
        assert!(claude_available(&claude));
        assert_eq!(
            suggest_name(&claude, "/usr/bin:/bin", "feat-x", ""),
            Ok(NameSuggestion {
                name: "Feat X".into(),
                slug: "feat-x".into(),
            })
        );
        assert!(suggest_name(&claude, "/usr/bin:/bin", "other", "").is_err());
    }
}
