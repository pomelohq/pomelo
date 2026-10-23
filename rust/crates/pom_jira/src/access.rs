//! Jira as the views use it, whichever machine asks it: this Mac with its own settings, or a remote project's
//! machine with the settings kept there.

use std::sync::Arc;

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};

use crate::{Board, Client, Issue, IssueDetail, JiraError, JiraSettings, SprintIssue, TokenOrigin};

/// Which tickets the new-workspace form lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketList {
    Sprint(i64),
    Backlog(i64),
    Assigned,
}

impl TicketList {
    pub fn encode(self) -> String {
        match self {
            TicketList::Sprint(board) => format!("sprint:{board}"),
            TicketList::Backlog(board) => format!("backlog:{board}"),
            TicketList::Assigned => "assigned".into(),
        }
    }

    pub fn decode(text: &str) -> Option<TicketList> {
        let text = text.trim();
        if text == "assigned" {
            return Some(TicketList::Assigned);
        }
        let (kind, board) = text.split_once(':')?;
        let board = board.parse().ok()?;
        match kind {
            "sprint" => Some(TicketList::Sprint(board)),
            "backlog" => Some(TicketList::Backlog(board)),
            _ => None,
        }
    }
}

/// What the views read from Jira.
pub trait Jira: Send + Sync {
    /// The account's display name and email.
    fn myself(&self) -> Result<(String, String), JiraError>;
    fn boards(&self) -> Result<Vec<Board>, JiraError>;
    fn issues(&self, list: TicketList) -> Result<Vec<SprintIssue>, JiraError>;
    fn search_by_keys(&self, keys: &[String]) -> Result<Vec<Issue>, JiraError>;
    fn issue_detail(&self, key: &str) -> Result<IssueDetail, JiraError>;
}

impl Jira for Client {
    fn myself(&self) -> Result<(String, String), JiraError> {
        Client::myself(self)
    }

    fn boards(&self) -> Result<Vec<Board>, JiraError> {
        Client::boards(self)
    }

    fn issues(&self, list: TicketList) -> Result<Vec<SprintIssue>, JiraError> {
        match list {
            TicketList::Sprint(board) => self.current_sprint_issues(board),
            TicketList::Backlog(board) => self.backlog_issues(board),
            TicketList::Assigned => self.assigned_issues(),
        }
    }

    fn search_by_keys(&self, keys: &[String]) -> Result<Vec<Issue>, JiraError> {
        Client::search_by_keys(self, keys)
    }

    fn issue_detail(&self, key: &str) -> Result<IssueDetail, JiraError> {
        Client::issue_detail(self, key)
    }
}

/// Reaches Jira when it is set up (it can be set up while the view is open), else `None`. May block: call it
/// off the main thread.
pub type Connect = Arc<dyn Fn() -> Option<Arc<dyn Jira>> + Send + Sync>;

/// Jira with this Mac's settings for `session`.
pub fn session_connect(state: &StateDir, session: &str) -> Connect {
    let (state, session) = (state.clone(), session.to_string());
    Arc::new(move || {
        crate::resolve(&state, &session).map(|client| Arc::new(client) as Arc<dyn Jira>)
    })
}

/// A session's Jira settings as Settings shows them: never the token itself, only where it comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JiraSetup {
    pub site: String,
    pub email: String,
    pub token: Option<TokenOrigin>,
}

impl JiraSetup {
    pub fn read(
        state: &StateDir,
        session: &str,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> JiraSetup {
        let stored = JiraSettings::load(state, session);
        let token = crate::resolve_token(state, session, &stored, env).map(|(_, origin)| origin);
        JiraSetup {
            site: stored.site,
            email: stored.email,
            token,
        }
    }

    /// Site, email and token are all there.
    pub fn ready(&self) -> bool {
        !self.site.trim().is_empty() && !self.email.trim().is_empty() && self.token.is_some()
    }
}

/// A change Settings makes: each field set replaces what is stored; `clear_token` forgets the stored token.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SetupChange {
    pub site: Option<String>,
    pub email: Option<String>,
    pub token: Option<String>,
    pub clear_token: bool,
}

impl SetupChange {
    /// Stores the change for `session` here.
    pub fn apply(&self, state: &StateDir, session: &str) -> Result<(), String> {
        if self.site.is_some() || self.email.is_some() {
            let mut stored = JiraSettings::load(state, session);
            if let Some(site) = &self.site {
                stored.site = site.trim().to_string();
            }
            if let Some(email) = &self.email {
                stored.email = email.trim().to_string();
            }
            stored
                .save(state, session)
                .map_err(|error| error.to_string())?;
        }
        if let Some(token) = self
            .token
            .as_deref()
            .filter(|token| !token.trim().is_empty())
        {
            crate::save_token(state, session, token)?;
        } else if self.clear_token {
            crate::save_token(state, session, "")?;
        }
        Ok(())
    }
}

/// Where Settings reads and writes a project's Jira settings when they are not on this Mac.
pub trait SetupStore: Send + Sync {
    fn read(&self) -> Result<JiraSetup, String>;
    fn change(&self, change: &SetupChange) -> Result<JiraSetup, String>;
    /// "Name (email)" of the account the settings sign in as.
    fn test(&self) -> Result<String, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_round_trip_through_their_text() {
        for list in [
            TicketList::Sprint(7),
            TicketList::Backlog(12),
            TicketList::Assigned,
        ] {
            assert_eq!(TicketList::decode(&list.encode()), Some(list));
        }
        assert_eq!(TicketList::decode("sprint:x"), None);
        assert_eq!(TicketList::decode("board:1"), None);
    }

    #[test]
    fn a_change_saves_only_what_it_names() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let state = StateDir::new(temp.path());
        let no_env = |_: &str| None;
        SetupChange {
            site: Some(" example.atlassian.net ".into()),
            email: Some("you@example.com".into()),
            token: Some("t0k".into()),
            clear_token: false,
        }
        .apply(&state, "myproject")?;
        let setup = JiraSetup::read(&state, "myproject", &no_env);
        assert!(setup.ready());
        assert_eq!(setup.site, "example.atlassian.net");
        assert_eq!(setup.token, Some(TokenOrigin::Secret));
        SetupChange {
            clear_token: true,
            ..SetupChange::default()
        }
        .apply(&state, "myproject")?;
        let setup = JiraSetup::read(&state, "myproject", &no_env);
        assert_eq!(setup.email, "you@example.com", "kept");
        assert_eq!(setup.token, None);
        assert!(!setup.ready());
        Ok(())
    }
}
