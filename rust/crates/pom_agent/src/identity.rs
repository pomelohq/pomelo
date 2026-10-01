//! Who an agent session is. Every agent holder is `ws-<session>-<branch>-<kind>`, so its name says which
//! project and workspace it belongs to; the launch also exports that identity, so the agent's hook processes
//! know it without guessing from the folder they run in.

use crate::launch::shell_quote;

pub const HOLDER_ENV: &str = "POM_AGENT_HOLDER";
pub const ROLE_ENV: &str = "POM_AGENT_ROLE";
pub const PROJECT_ENV: &str = "POM_PROJECT_SESSION";
pub const WORKSPACE_ENV: &str = "POM_WORKSPACE";
pub const DRIVER_ENV: &str = "POM_AGENT_DRIVER";

pub const CLAUDE_DRIVER: &str = "claude";
pub const NO_DRIVER: &str = "none";

/// The holder name prefix of every holder of a workspace.
pub fn workspace_prefix(session: &str, branch: &str) -> String {
    format!(
        "ws-{}-{}-",
        session.replace('/', "_"),
        branch.replace('/', "_")
    )
}

/// What an agent holder runs, read from the part of its name after the workspace prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderRole {
    pub role: String,
    pub driver: &'static str,
    /// Programs and other agents may send it turns.
    pub drivable: bool,
}

fn numbered(rest: &str) -> bool {
    !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit())
}

fn role_name(name: &str) -> bool {
    name.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The role of an agent holder's kind suffix (`claude-raw`, `fixer-2`, `side-review-1`, `claude-reviewer`,
/// `agent-codex`); `None` for any other holder of the workspace, such as a service or a terminal.
pub fn role_of_kind(kind: &str) -> Option<HolderRole> {
    let claude = |role: String, drivable: bool| HolderRole {
        role,
        driver: CLAUDE_DRIVER,
        drivable,
    };
    if kind == "claude-raw" {
        return Some(claude("claude".into(), true));
    }
    if let Some(number) = kind.strip_prefix("claude-raw-") {
        return numbered(number).then(|| claude(format!("claude-{number}"), true));
    }
    if kind == "fixer" || kind.strip_prefix("fixer-").is_some_and(numbered) {
        return Some(claude(kind.into(), true));
    }
    if kind == "onboarder" {
        return Some(claude(kind.into(), false));
    }
    if let Some(side) = kind.strip_prefix("side-") {
        let (side_role, number) = side.rsplit_once('-')?;
        return (["ask", "review", "fix"].contains(&side_role) && numbered(number))
            .then(|| claude(format!("{side_role}-{number}"), true));
    }
    if let Some(cli) = kind.strip_prefix("agent-") {
        return role_name(cli).then(|| HolderRole {
            role: cli.into(),
            driver: NO_DRIVER,
            drivable: false,
        });
    }
    let fresh = kind.strip_prefix("claude-")?;
    (role_name(fresh) && !fresh.starts_with("raw")).then(|| claude(fresh.into(), true))
}

/// The role of `holder` when it is an agent of workspace `branch` of project `session`.
pub fn holder_role(holder: &str, session: &str, branch: &str) -> Option<HolderRole> {
    role_of_kind(holder.strip_prefix(&workspace_prefix(session, branch))?)
}

/// Whether a holder name looks like any workspace's agent, without knowing which workspace.
pub fn looks_like_agent_holder(holder: &str) -> bool {
    let Some(rest) = holder.strip_prefix("ws-") else {
        return false;
    };
    // Session and branch may contain dashes, so try every split for a kind suffix that is an agent's.
    rest.match_indices('-')
        .any(|(at, _)| role_of_kind(&rest[at + 1..]).is_some())
}

/// The holder name of a role started on its own (`pom agent start --fresh --role reviewer`).
pub fn fresh_holder(session: &str, branch: &str, role: &str) -> String {
    format!("{}claude-{role}", workspace_prefix(session, branch))
}

/// One agent session's identity, as its launch exports it to the agent and its hooks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub holder: String,
    pub role: String,
    pub project: String,
    pub branch: String,
    pub driver: String,
}

impl Identity {
    /// The identity an agent holder's name gives it, for the launch to export.
    pub fn for_holder(holder: &str, session: &str, branch: &str) -> Identity {
        let role = holder_role(holder, session, branch);
        Identity {
            holder: holder.to_string(),
            role: role
                .as_ref()
                .map_or_else(|| "claude".to_string(), |role| role.role.clone()),
            project: session.to_string(),
            branch: branch.to_string(),
            driver: role.map_or(CLAUDE_DRIVER, |role| role.driver).to_string(),
        }
    }

    /// The identity an agent process was started with; `None` for one started before launches exported it.
    pub fn from_env() -> Option<Identity> {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Some(Identity {
            holder: var(HOLDER_ENV)?,
            role: var(ROLE_ENV)?,
            project: var(PROJECT_ENV)?,
            branch: var(WORKSPACE_ENV)?,
            driver: var(DRIVER_ENV).unwrap_or_else(|| CLAUDE_DRIVER.to_string()),
        })
    }

    /// `NAME='value' ...` for a launch script's `export`.
    pub fn exports(&self) -> String {
        [
            (HOLDER_ENV, &self.holder),
            (ROLE_ENV, &self.role),
            (PROJECT_ENV, &self.project),
            (WORKSPACE_ENV, &self.branch),
            (DRIVER_ENV, &self.driver),
        ]
        .iter()
        .map(|(name, value)| format!("{name}={}", shell_quote(value)))
        .collect::<Vec<_>>()
        .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_holders_name_their_role_and_other_holders_do_not() {
        let role =
            |holder: &str| holder_role(holder, "myproject", "feat/login").map(|role| role.role);
        assert_eq!(
            role("ws-myproject-feat_login-claude-raw").as_deref(),
            Some("claude")
        );
        assert_eq!(
            role("ws-myproject-feat_login-claude-raw-2").as_deref(),
            Some("claude-2")
        );
        assert_eq!(
            role("ws-myproject-feat_login-fixer").as_deref(),
            Some("fixer")
        );
        assert_eq!(
            role("ws-myproject-feat_login-fixer-3").as_deref(),
            Some("fixer-3")
        );
        assert_eq!(
            role("ws-myproject-feat_login-side-review-1").as_deref(),
            Some("review-1")
        );
        assert_eq!(
            role("ws-myproject-feat_login-claude-reviewer").as_deref(),
            Some("reviewer")
        );
        assert_eq!(
            role("ws-myproject-feat_login-agent-codex").as_deref(),
            Some("codex")
        );
        assert_eq!(
            role("ws-myproject-feat_login-onboarder").as_deref(),
            Some("onboarder")
        );
        assert_eq!(
            role("ws-myproject-feat_login-gateway"),
            None,
            "a workspace service"
        );
        assert_eq!(role("ws-myproject-feat_login-side-review-x"), None);
        assert_eq!(
            role("ws-other-feat_login-claude-raw"),
            None,
            "another project"
        );
        assert_eq!(
            role("ws-myproject-feat_login-x-claude-raw"),
            None,
            "a holder of branch feat/login-x is not one of feat/login's"
        );
        let codex =
            holder_role("ws-myproject-main-agent-codex", "myproject", "main").expect("codex");
        assert!(!codex.drivable && codex.driver == NO_DRIVER);
        assert!(
            !holder_role("ws-myproject-main-onboarder", "myproject", "main")
                .expect("onboarder")
                .drivable
        );
        assert!(looks_like_agent_holder("ws-my-project-feat-x-claude-raw"));
        assert!(!looks_like_agent_holder("ws-myproject-main-gateway"));
        assert!(!looks_like_agent_holder("svc-myproject-main-api-web"));
        assert_eq!(
            fresh_holder("myproject", "feat/login", "reviewer"),
            "ws-myproject-feat_login-claude-reviewer"
        );
    }

    #[test]
    fn the_exported_identity_survives_the_shell() {
        let identity = Identity::for_holder("ws-myproject-feat_x-fixer", "myproject", "feat/x");
        assert_eq!(identity.role, "fixer");
        let script = format!("export {}; printf '%s|%s|%s|%s|%s' \"$POM_AGENT_HOLDER\" \"$POM_AGENT_ROLE\" \"$POM_PROJECT_SESSION\" \"$POM_WORKSPACE\" \"$POM_AGENT_DRIVER\"", identity.exports());
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .expect("sh");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "ws-myproject-feat_x-fixer|fixer|myproject|feat/x|claude"
        );
    }
}
