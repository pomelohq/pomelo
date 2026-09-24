//! Jira's REST API over `curl`. The Basic credentials reach curl on stdin (a `-K -` config), so the token never
//! appears in a process listing.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::adf::adf_markdown;

const TIMEOUT_SECONDS: &str = "8";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JiraError {
    Http(u16),
    Transport(String),
    Parse(String),
}

impl std::fmt::Display for JiraError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JiraError::Http(401 | 403) => {
                write!(formatter, "auth rejected - check email and token")
            }
            JiraError::Http(code) => write!(formatter, "jira: HTTP {code}"),
            JiraError::Transport(message) => write!(formatter, "jira: {message}"),
            JiraError::Parse(message) => write!(formatter, "jira: unexpected answer ({message})"),
        }
    }
}

impl std::error::Error for JiraError {}

/// Fetches a URL with the given `Authorization` header value; returns the HTTP status and body.
pub trait Transport: Send + Sync {
    fn get(&self, url: &str, authorization: &str) -> Result<(u16, String), JiraError>;
}

pub struct CurlTransport {
    pub program: PathBuf,
}

impl Default for CurlTransport {
    fn default() -> CurlTransport {
        CurlTransport {
            program: PathBuf::from("/usr/bin/curl"),
        }
    }
}

impl Transport for CurlTransport {
    fn get(&self, url: &str, authorization: &str) -> Result<(u16, String), JiraError> {
        let mut child = Command::new(&self.program)
            .args([
                "-sS",
                "--max-time",
                TIMEOUT_SECONDS,
                "-K",
                "-",
                "-w",
                "\n%{http_code}",
            ])
            .arg(url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| JiraError::Transport(format!("curl: {error}")))?;
        let config = format!(
            "header = \"Accept: application/json\"\nheader = \"Authorization: {}\"\n",
            authorization.replace('\\', "\\\\").replace('"', "\\\"")
        );
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(config.as_bytes())
                .map_err(|error| JiraError::Transport(format!("curl: {error}")))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| JiraError::Transport(format!("curl: {error}")))?;
        let text = String::from_utf8_lossy(&output.stdout);
        let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
        match code.trim().parse::<u16>() {
            Ok(code) if code > 0 => Ok((code, body.to_string())),
            _ => Err(JiraError::Transport(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            )),
        }
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = chunk.iter().enumerate().fold(0u32, |acc, (index, byte)| {
            acc | (u32::from(*byte) << (16 - 8 * index))
        });
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(char::from(
                    ALPHABET[((triple >> (18 - 6 * index)) & 63) as usize],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn url_escape(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

/// A ticket's status line, as the WORKSPACES rows show it (same shape as the previous core's cache).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub assignee: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SprintIssue {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub assignee: String,
    pub sprint: String,
    /// Assigned to the account the token belongs to.
    pub mine: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebLink {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub icon: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDetail {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub url: String,
    /// Markdown converted from Jira's document format, image attachments appended.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub comments: Vec<Comment>,
    #[serde(default)]
    pub web_links: Vec<WebLink>,
}

pub struct Client {
    site: String,
    authorization: String,
    transport: Box<dyn Transport>,
}

impl Client {
    pub fn new(site: &str, email: &str, token: &str, transport: Box<dyn Transport>) -> Client {
        Client {
            site: crate::normalize_site(site),
            authorization: format!("Basic {}", base64(format!("{email}:{token}").as_bytes())),
            transport,
        }
    }

    pub fn site(&self) -> &str {
        &self.site
    }

    pub fn browse_url(&self, key: &str) -> String {
        format!("{}/browse/{key}", self.site)
    }

    fn get(&self, path: &str) -> Result<Value, JiraError> {
        let (code, body) = self
            .transport
            .get(&format!("{}{path}", self.site), &self.authorization)?;
        if code != 200 {
            return Err(JiraError::Http(code));
        }
        serde_json::from_str(&body).map_err(|error| JiraError::Parse(error.to_string()))
    }

    /// The account's display name and email: what "Test connection" shows.
    pub fn myself(&self) -> Result<(String, String), JiraError> {
        let me = self.get("/rest/api/3/myself")?;
        Ok((text(&me["displayName"]), text(&me["emailAddress"])))
    }

    fn account_id(&self) -> Result<String, JiraError> {
        Ok(text(&self.get("/rest/api/3/myself")?["accountId"]))
    }

    pub fn boards(&self) -> Result<Vec<Board>, JiraError> {
        let body = self.get("/rest/agile/1.0/board?maxResults=50")?;
        Ok(body["values"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .map(|board| Board {
                        id: board["id"].as_i64().unwrap_or_default(),
                        name: text(&board["name"]),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Tickets of the board's active sprints, each marked when it is assigned to this account.
    pub fn current_sprint_issues(&self, board: i64) -> Result<Vec<SprintIssue>, JiraError> {
        let me = self.account_id().unwrap_or_default();
        let sprints = self.get(&format!(
            "/rest/agile/1.0/board/{board}/sprint?state=active&maxResults=10"
        ))?;
        let mut issues = Vec::new();
        for sprint in sprints["values"].as_array().into_iter().flatten() {
            let name = text(&sprint["name"]);
            let id = sprint["id"].as_i64().unwrap_or_default();
            let Ok(body) = self.get(&format!(
                "/rest/agile/1.0/sprint/{id}/issue?maxResults=200&fields=summary,status,assignee"
            )) else {
                continue;
            };
            for issue in body["issues"].as_array().into_iter().flatten() {
                let fields = &issue["fields"];
                let assignee = &fields["assignee"];
                let account = text(&assignee["accountId"]);
                issues.push(SprintIssue {
                    key: text(&issue["key"]),
                    summary: text(&fields["summary"]),
                    status: text(&fields["status"]["name"]),
                    assignee: text(&assignee["displayName"]),
                    sprint: name.clone(),
                    mine: !me.is_empty() && account == me,
                });
            }
        }
        Ok(issues)
    }

    pub fn search_by_keys(&self, keys: &[String]) -> Result<Vec<Issue>, JiraError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let jql = format!("key in ({})", keys.join(","));
        let body = self.get(&format!(
            "/rest/api/3/search/jql?fields=summary,status,assignee&maxResults=100&jql={}",
            url_escape(&jql)
        ))?;
        Ok(body["issues"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|issue| {
                let fields = &issue["fields"];
                let key = text(&issue["key"]);
                Issue {
                    url: self.browse_url(&key),
                    key,
                    summary: text(&fields["summary"]),
                    status: text(&fields["status"]["name"]),
                    category: text(&fields["status"]["statusCategory"]["key"]),
                    assignee: text(&fields["assignee"]["displayName"]),
                }
            })
            .collect())
    }

    pub fn issue_detail(&self, key: &str) -> Result<IssueDetail, JiraError> {
        let body = self.get(&format!(
            "/rest/api/3/issue/{key}?fields=summary,status,description,attachment,comment"
        ))?;
        let fields = &body["fields"];
        let mut description = adf_markdown(&fields["description"]);
        let images: Vec<String> = fields["attachment"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|file| text(&file["mimeType"]).starts_with("image/"))
            .filter(|file| !text(&file["content"]).is_empty())
            .map(|file| format!("![{}]({})", text(&file["filename"]), text(&file["content"])))
            .collect();
        if !images.is_empty() {
            description = format!(
                "{}\n\n### Attachments\n\n{}",
                description.trim(),
                images.join("\n\n")
            );
        }
        let comments = fields["comment"]["comments"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|comment| Comment {
                id: text(&comment["id"]),
                author: text(&comment["author"]["displayName"]),
                avatar: text(&comment["author"]["avatarUrls"]["48x48"]),
                created: text(&comment["created"]),
                body: adf_markdown(&comment["body"]),
            })
            .collect();
        let web_links = self
            .get(&format!("/rest/api/3/issue/{key}/remotelink"))
            .map(|links| {
                links
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|link| WebLink {
                        title: text(&link["object"]["title"]),
                        url: text(&link["object"]["url"]),
                        icon: text(&link["object"]["icon"]["url16x16"]),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(IssueDetail {
            key: key.to_string(),
            summary: text(&fields["summary"]),
            status: text(&fields["status"]["name"]),
            url: self.browse_url(key),
            description,
            comments,
            web_links,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(
            base64(b"you@example.com:t0k"),
            "eW91QGV4YW1wbGUuY29tOnQwaw=="
        );
    }

    #[test]
    fn jql_is_escaped_for_the_query_string() {
        assert_eq!(url_escape("key in (A-1,B-2)"), "key%20in%20%28A-1%2CB-2%29");
    }

    #[test]
    fn auth_failures_read_plainly() {
        assert_eq!(
            JiraError::Http(401).to_string(),
            "auth rejected - check email and token"
        );
        assert_eq!(JiraError::Http(500).to_string(), "jira: HTTP 500");
    }
}
