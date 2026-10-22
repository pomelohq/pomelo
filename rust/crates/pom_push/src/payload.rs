//! The fields of a GitHub `push` delivery that decide which workspaces it reaches.

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Push {
    #[serde(rename = "ref", default)]
    pub reference: String,
    #[serde(default)]
    pub after: String,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub commits: Vec<serde_json::Value>,
    #[serde(default)]
    pub repository: Repository,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Repository {
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub clone_url: String,
    #[serde(default)]
    pub ssh_url: String,
    #[serde(default)]
    pub git_url: String,
    #[serde(default)]
    pub html_url: String,
}

impl Push {
    /// `form` is a delivery sent as `application/x-www-form-urlencoded` (`payload=<json>`).
    pub fn parse(body: &[u8], form: bool) -> Result<Push, String> {
        let json = if form {
            let text = String::from_utf8_lossy(body);
            let encoded = text
                .split('&')
                .find_map(|pair| pair.strip_prefix("payload="))
                .ok_or("a form delivery without a payload field")?;
            percent_decode(encoded).into_bytes()
        } else {
            body.to_vec()
        };
        serde_json::from_slice(&json).map_err(|error| format!("not a push payload: {error}"))
    }

    /// The branch pushed to; `None` for a tag.
    pub fn branch(&self) -> Option<&str> {
        self.reference
            .strip_prefix("refs/heads/")
            .filter(|branch| !branch.is_empty())
    }
}

impl Repository {
    /// Every way the repository can be cloned, normalised the way a local `origin` is.
    pub fn remotes(&self) -> Vec<String> {
        let mut remotes: Vec<String> = [
            &self.clone_url,
            &self.ssh_url,
            &self.git_url,
            &self.html_url,
        ]
        .into_iter()
        .filter(|url| !url.is_empty())
        .map(|url| pom_layout::normalize_remote(url))
        .collect();
        remotes.sort();
        remotes.dedup();
        remotes
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'+' => out.push(b' '),
            b'%' if at + 3 <= bytes.len() => {
                match std::str::from_utf8(&bytes[at + 1..at + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                {
                    Some(byte) => {
                        out.push(byte);
                        at += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"ref":"refs/heads/feat-x","before":"a","after":"b","deleted":false,
        "commits":[{"id":"b"}],
        "repository":{"full_name":"myorg/api","clone_url":"https://github.com/myorg/api.git",
        "ssh_url":"git@github.com:myorg/api.git","git_url":"git://github.com/myorg/api.git",
        "html_url":"https://github.com/myorg/api"}}"#;

    #[test]
    fn reads_the_branch_and_every_remote() {
        let push = Push::parse(BODY.as_bytes(), false).expect("push");
        assert_eq!(push.branch(), Some("feat-x"));
        assert_eq!(push.after, "b");
        assert_eq!(push.commits.len(), 1);
        assert_eq!(push.repository.remotes(), vec!["github.com/myorg/api"]);
        let tag = Push {
            reference: "refs/tags/v1".into(),
            ..Push::default()
        };
        assert_eq!(tag.branch(), None);
    }

    #[test]
    fn a_form_delivery_carries_the_same_json() {
        let encoded: String = BODY
            .bytes()
            .map(|byte| match byte {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => (byte as char).to_string(),
                b' ' => "+".to_string(),
                other => format!("%{other:02X}"),
            })
            .collect();
        let push = Push::parse(format!("payload={encoded}").as_bytes(), true).expect("form");
        assert_eq!(push.branch(), Some("feat-x"));
        assert!(Push::parse(b"other=1", true).is_err());
        assert!(Push::parse(b"not json", false).is_err());
    }
}
