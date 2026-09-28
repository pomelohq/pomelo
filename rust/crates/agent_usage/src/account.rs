use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub email: String,
    pub organization: String,
    /// "max", "pro", "team", as the credentials name it.
    pub plan: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Window {
    /// 0 to 100.
    pub used: f32,
    /// Unix seconds; `None` when the window has not started.
    pub resets_at: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    pub session: Window,
    pub weekly: Window,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LimitsError {
    SignedOut,
    /// Asked too often; try again later.
    RateLimited,
    Failed(String),
}

/// The account Claude Code is signed in with: its name and email from `~/.claude.json`, its plan from the
/// credentials. `None` when Claude Code was never signed in.
pub fn read_account(home: &Path) -> Option<Account> {
    let text = std::fs::read_to_string(home.join(".claude.json")).ok()?;
    let config: Value = serde_json::from_str(&text).ok()?;
    let oauth = config.get("oauthAccount")?;
    let field = |key: &str| {
        oauth
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let email = field("emailAddress");
    if email.is_empty() {
        return None;
    }
    let name = match field("displayName") {
        name if !name.is_empty() => name,
        _ => email.split('@').next().unwrap_or_default().to_string(),
    };
    let plan = credentials(home)
        .into_iter()
        .find_map(|credentials| {
            credentials
                .get("claudeAiOauth")?
                .get("subscriptionType")?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_default();
    Some(Account {
        name,
        email,
        organization: field("organizationName"),
        plan,
    })
}

/// Claude Code's stored credentials, the keychain entry first (the file can hold a token that has since
/// been refreshed there), then the file.
fn credentials(home: &Path) -> Vec<Value> {
    let mut found = Vec::new();
    let keychain = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .stderr(Stdio::null())
        .output();
    if let Some(output) = keychain.ok().filter(|output| output.status.success()) {
        if let Ok(value) = serde_json::from_slice(&output.stdout) {
            found.push(value);
        }
    }
    if let Ok(text) = std::fs::read_to_string(home.join(".claude/.credentials.json")) {
        if let Ok(value) = serde_json::from_str(&text) {
            found.push(value);
        }
    }
    found
}

fn access_tokens(home: &Path) -> Vec<String> {
    credentials(home)
        .iter()
        .filter_map(|credentials| {
            credentials
                .get("claudeAiOauth")?
                .get("accessToken")?
                .as_str()
                .map(str::to_string)
        })
        .collect()
}

/// The plan limits of the signed-in account, from the usage endpoint Claude Code's status line reads.
pub fn fetch_limits(home: &Path) -> Result<Limits, LimitsError> {
    let tokens = access_tokens(home);
    if tokens.is_empty() {
        return Err(LimitsError::SignedOut);
    }
    let mut last = LimitsError::SignedOut;
    for token in tokens {
        match request(&token) {
            Ok(body) => return parse_limits(&body),
            // A stale token in one place may be fresh in the other.
            Err(Some(401)) => last = LimitsError::SignedOut,
            Err(Some(429)) => return Err(LimitsError::RateLimited),
            Err(Some(status)) => {
                return Err(LimitsError::Failed(format!("usage returned {status}")))
            }
            Err(None) => return Err(LimitsError::Failed("usage request failed".into())),
        }
    }
    Err(last)
}

/// The response body, or the HTTP status it failed with. The token goes in on stdin, never in argv.
fn request(token: &str) -> Result<String, Option<u16>> {
    let mut child = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "10",
            "-w",
            "\n%{http_code}",
            "-H",
            "@-",
            "-H",
            "anthropic-beta: oauth-2025-04-20",
            USAGE_URL,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| None)?;
    if let Some(mut stdin) = child.stdin.take() {
        if writeln!(stdin, "Authorization: Bearer {token}").is_err() {
            return Err(None);
        }
    }
    let output = child.wait_with_output().map_err(|_| None)?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (body, status) = text.rsplit_once('\n').ok_or(None)?;
    match status.trim().parse::<u16>() {
        Ok(200) => Ok(body.to_string()),
        Ok(status) => Err(Some(status)),
        Err(_) => Err(None),
    }
}

pub fn parse_limits(body: &str) -> Result<Limits, LimitsError> {
    let value: Value =
        serde_json::from_str(body).map_err(|error| LimitsError::Failed(error.to_string()))?;
    let window = |key: &str| {
        let Some(window) = value.get(key).filter(|window| window.is_object()) else {
            return Window::default();
        };
        let used = ["utilization", "used_percentage", "percent"]
            .iter()
            .find_map(|key| window.get(*key).and_then(Value::as_f64))
            .unwrap_or(0.0);
        let resets_at = match window.get("resets_at") {
            Some(Value::Number(number)) => number.as_f64().map(|seconds| {
                if seconds > 1e12 {
                    (seconds / 1000.0) as u64
                } else {
                    seconds as u64
                }
            }),
            Some(Value::String(text)) => crate::parse_timestamp(text),
            _ => None,
        };
        Window {
            used: used as f32,
            resets_at,
        }
    };
    Ok(Limits {
        session: window("five_hour"),
        weekly: window("seven_day"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_read_either_percentage_field_and_any_reset_form() {
        let limits = parse_limits(
            r#"{"five_hour":{"utilization":23.0,"resets_at":"2026-09-29T01:10:00+00:00"},"seven_day":{"used_percentage":29,"resets_at":1790000000000},"seven_day_opus":null}"#,
        )
        .expect("parses");
        assert_eq!(limits.session.used, 23.0);
        assert_eq!(limits.session.resets_at, Some(1_790_644_200));
        assert_eq!(limits.weekly.used, 29.0);
        assert_eq!(limits.weekly.resets_at, Some(1_790_000_000));
        assert_eq!(
            parse_limits("{}").map(|limits| limits.session.used),
            Ok(0.0)
        );
        assert!(parse_limits("nope").is_err());
    }

    #[test]
    fn the_account_comes_from_the_claude_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert_eq!(read_account(temp.path()), None);
        std::fs::write(
            temp.path().join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"dev@example.com","displayName":"dev","organizationName":"Example"}}"#,
        )
        .expect("config");
        let account = read_account(temp.path()).expect("signed in");
        assert_eq!(account.name, "dev");
        assert_eq!(account.organization, "Example");
    }
}
