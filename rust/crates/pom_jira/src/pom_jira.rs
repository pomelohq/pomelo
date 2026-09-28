//! Jira, read-only. The site and email live app-local per session (`integrations/<session>.json`, shared with the
//! previous core) and the API token in the session's secrets (`__jira_token`), else `JIRA_API_TOKEN`. Nothing goes
//! into `pom.yml`. Without all three the feature is off and callers hide it.

mod adf;
mod cache;
mod client;
mod suggest;

use std::path::PathBuf;

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};

pub use adf::adf_markdown;
pub use cache::IssueCache;
pub use client::{
    Board, Client, Comment, CurlTransport, Issue, IssueDetail, JiraError, SprintIssue, Transport,
    WebLink,
};
pub use suggest::{fuzzy_score, rank_suggestions};

pub const DEFAULT_TOKEN_ENV: &str = "JIRA_API_TOKEN";
pub const TOKEN_SECRET: &str = "__jira_token";

/// The Jira part of a session's integrations file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraSettings {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub site: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub email: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token_env: String,
}

fn integrations_path(state: &StateDir, session: &str) -> PathBuf {
    let name = if session.is_empty() { "_" } else { session };
    state.path("integrations").join(format!("{name}.json"))
}

impl JiraSettings {
    pub fn load(state: &StateDir, session: &str) -> JiraSettings {
        std::fs::read_to_string(integrations_path(state, session))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| serde_json::from_value(value.get("jira")?.clone()).ok())
            .unwrap_or_default()
    }

    /// Writes the Jira part, keeping whatever else the file holds.
    pub fn save(&self, state: &StateDir, session: &str) -> std::io::Result<()> {
        let path = integrations_path(state, session);
        let mut whole = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
        whole["jira"] = serde_json::to_value(self).map_err(std::io::Error::other)?;
        let mut text = serde_json::to_string_pretty(&whole).map_err(std::io::Error::other)?;
        text.push('\n');
        pom_paths::write_atomic(&path, text.as_bytes(), 0o644)
    }
}

/// `acme.atlassian.net/` -> `https://acme.atlassian.net`.
pub fn normalize_site(site: &str) -> String {
    let site = site.trim().trim_end_matches('/');
    if site.is_empty() || site.starts_with("http://") || site.starts_with("https://") {
        site.to_string()
    } else {
        format!("https://{site}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenOrigin {
    Secret,
    Environment,
}

/// The token and where it came from: the session's secret, else the environment variable the settings name
/// (or the default one).
pub fn resolve_token(
    state: &StateDir,
    session: &str,
    settings: &JiraSettings,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<(String, TokenOrigin)> {
    let secret = pom_secrets::SecretStore::new(state.clone(), session)
        .get(TOKEN_SECRET)
        .unwrap_or_else(|error| {
            eprintln!("jira: read the token secret: {error}");
            None
        })
        .filter(|token| !token.trim().is_empty());
    if let Some(token) = secret {
        return Some((token, TokenOrigin::Secret));
    }
    let name = if settings.token_env.is_empty() {
        DEFAULT_TOKEN_ENV
    } else {
        &settings.token_env
    };
    env(name)
        .filter(|token| !token.trim().is_empty())
        .map(|token| (token, TokenOrigin::Environment))
}

pub fn save_token(state: &StateDir, session: &str, token: &str) -> Result<(), String> {
    pom_secrets::SecretStore::new(state.clone(), session)
        .set(TOKEN_SECRET, token.trim())
        .map_err(|error| error.to_string())
}

/// A client for the session, or `None` while site, email or token is missing.
pub fn resolve(state: &StateDir, session: &str) -> Option<Client> {
    let settings = JiraSettings::load(state, session);
    if settings.site.trim().is_empty() || settings.email.trim().is_empty() {
        return None;
    }
    let (token, _) = resolve_token(state, session, &settings, &|name| std::env::var(name).ok())?;
    Some(Client::new(
        &settings.site,
        settings.email.trim(),
        &token,
        Box::new(CurlTransport::default()),
    ))
}

/// The ticket a branch names: a leading `abc-123` (case-insensitive), uppercased.
pub fn key_for_branch(branch: &str) -> Option<String> {
    let mut chars = branch.char_indices().peekable();
    match chars.next() {
        Some((_, first)) if first.is_ascii_alphabetic() => {}
        _ => return None,
    }
    let mut end_of_project = None;
    for (at, c) in chars.by_ref() {
        if c == '-' {
            end_of_project = Some(at);
            break;
        }
        if !c.is_ascii_alphanumeric() {
            return None;
        }
    }
    let dash = end_of_project?;
    let digits = branch[dash + 1..]
        .chars()
        .take_while(char::is_ascii_digit)
        .count();
    (digits > 0).then(|| branch[..dash + 1 + digits].to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_come_from_the_start_of_the_branch() {
        for (branch, key) in [
            ("task-725-add-chat", Some("TASK-725")),
            ("abc-123", Some("ABC-123")),
            ("Ab2-9x", Some("AB2-9")),
            ("fix/login", None),
            ("725-no-project", None),
            ("abc-", None),
            ("", None),
        ] {
            assert_eq!(key_for_branch(branch).as_deref(), key, "{branch}");
        }
    }

    #[test]
    fn sites_get_a_scheme_and_lose_the_trailing_slash() {
        assert_eq!(
            normalize_site(" acme.atlassian.net/ "),
            "https://acme.atlassian.net"
        );
        assert_eq!(normalize_site("http://jira.local"), "http://jira.local");
        assert_eq!(normalize_site(""), "");
    }

    #[test]
    fn settings_share_the_file_with_other_integrations() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let state = StateDir::new(temp.path());
        let path = temp.path().join("integrations/demo.json");
        std::fs::create_dir_all(path.parent().expect("parent"))?;
        std::fs::write(
            &path,
            r#"{"jira":{"site":"old"},"sync":{"configured":true}}"#,
        )?;
        assert_eq!(JiraSettings::load(&state, "demo").site, "old");
        let settings = JiraSettings {
            site: "https://acme.atlassian.net".into(),
            email: "you@example.com".into(),
            token_env: String::new(),
        };
        settings.save(&state, "demo")?;
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        assert_eq!(saved["sync"]["configured"], true);
        assert_eq!(saved["jira"]["email"], "you@example.com");
        assert!(saved["jira"].get("token_env").is_none());
        assert_eq!(JiraSettings::load(&state, "demo"), settings);
        assert_eq!(JiraSettings::load(&state, "other"), JiraSettings::default());
        Ok(())
    }

    #[test]
    fn the_secret_wins_over_the_environment() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let state = StateDir::new(temp.path());
        let settings = JiraSettings::default();
        let env = |name: &str| (name == DEFAULT_TOKEN_ENV).then(|| "from-env".to_string());
        assert_eq!(
            resolve_token(&state, "demo", &settings, &env),
            Some(("from-env".to_string(), TokenOrigin::Environment))
        );
        save_token(&state, "demo", " from-secret ")?;
        assert_eq!(
            resolve_token(&state, "demo", &settings, &env),
            Some(("from-secret".to_string(), TokenOrigin::Secret))
        );
        let named = JiraSettings {
            token_env: "OTHER".into(),
            ..JiraSettings::default()
        };
        assert_eq!(resolve_token(&state, "fresh", &named, &env), None);
        assert!(resolve(&state, "demo").is_none(), "no site or email");
        Ok(())
    }
}
