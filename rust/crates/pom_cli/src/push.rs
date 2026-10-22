//! `pom github webhook setup` and `pom sync status`: setting up sync on push, and what it did.

use std::io::Write;

use pom_paths::StateDir;

use crate::args::Args;

const DEFAULT_LIMIT: usize = 20;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WebhookSetup {
    pub rotate: bool,
    pub json: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SyncStatus {
    pub limit: usize,
    pub json: bool,
}

pub(crate) fn parse_github(rest: &[&str]) -> Result<WebhookSetup, String> {
    let usage = "usage: pom github webhook setup [--rotate] [--json]";
    let ["webhook", "setup", flags @ ..] = rest else {
        return Err(usage.into());
    };
    let args = Args::parse(flags, &["-o", "--output"])?;
    args.allow(&["--rotate", "--json", "-o", "--output"])?;
    args.at_most(0, "github webhook setup")?;
    Ok(WebhookSetup {
        rotate: args.has("--rotate"),
        json: args.has("--json") || args.json()?,
    })
}

pub(crate) fn parse_sync(rest: &[&str]) -> Result<SyncStatus, String> {
    let usage = "usage: pom sync status [--limit n] [--json]";
    let ["status", flags @ ..] = rest else {
        return Err(usage.into());
    };
    let args = Args::parse(flags, &["-o", "--output", "--limit", "-n"])?;
    args.allow(&["--json", "-o", "--output", "--limit", "-n"])?;
    args.at_most(0, "sync status")?;
    let limit = match args.value(&["--limit", "-n"]) {
        Some(text) => text
            .parse()
            .map_err(|_| format!("--limit takes a number, not {text}"))?,
        None => DEFAULT_LIMIT,
    };
    Ok(SyncStatus {
        limit,
        json: args.has("--json") || args.json()?,
    })
}

fn write(out: &mut dyn Write, text: &str) -> Result<(), String> {
    writeln!(out, "{text}").map_err(|error| error.to_string())
}

/// Makes (or keeps) the project's webhook secret and says what to enter on GitHub.
pub(crate) fn setup(
    state: &StateDir,
    session: &str,
    request: &WebhookSetup,
    out: &mut dyn Write,
) -> Result<(), String> {
    let created = pom_push::ensure_secret(state, session, request.rotate)?;
    let network = pom_env::dev_network();
    let (url, public) = pom_push::payload_url(&network);
    if request.json {
        let document = serde_json::json!({
            "schema": "pom.github/v1",
            "project": session,
            "payload_url": url,
            "public": public,
            "relay_port": network.ports.webhook,
            "content_type": "application/json",
            "events": ["push"],
            "secret": created,
            "secret_created": created.is_some(),
        });
        return write(out, &document.to_string());
    }
    write(out, &format!("Payload URL:   {url}"))?;
    write(out, "Content type:  application/json")?;
    match &created {
        Some(secret) => write(
            out,
            &format!("Secret:        {secret}\n               (shown once; it is stored in the project's secret store)"),
        )?,
        None => write(
            out,
            "Secret:        already set (`--rotate` makes a new one; update it on GitHub too)",
        )?,
    }
    write(out, "Events:        Just the push event")?;
    if !public {
        write(
            out,
            &format!("\nGitHub must reach this URL: route a public name to the relay (port {}), for example with a\nreverse proxy or a tunnel, and set proxy.domain (`pom proxy config set domain ...`).",
                network.ports.webhook),
        )?;
    }
    write(
        out,
        "\nAdd it under the repository's Settings > Webhooks, or with:\n  gh api repos/OWNER/REPO/hooks -f name=web -F active=true -f 'events[]=push' \\\n    -f config[url]=URL -f config[content_type]=json -f config[secret]=SECRET\nWhat a push does is `sync.on_push` in pom.yml (notify by default).",
    )
}

pub(crate) fn status(
    state: &StateDir,
    request: &SyncStatus,
    out: &mut dyn Write,
) -> Result<(), String> {
    let records = pom_push::recent(state, request.limit);
    if request.json {
        let document = serde_json::json!({
            "schema": "pom.sync/v1",
            "journal": pom_push::journal_path(state),
            "events": records,
        });
        return write(out, &document.to_string());
    }
    if records.is_empty() {
        return write(
            out,
            "no push deliveries yet (see `pom github webhook setup`)",
        );
    }
    for record in records {
        let place = match (record.project.is_empty(), record.workspace.is_empty()) {
            (true, _) => record.repository.clone(),
            (false, true) => record.project.clone(),
            (false, false) => format!("{}/{} {}", record.project, record.workspace, record.repo),
        };
        let mut line = format!(
            "{}  {:<10}  {:<10}  {}",
            record.time,
            record.event,
            record.outcome.name(),
            place.trim()
        );
        if !record.branch.is_empty() && record.workspace != record.branch {
            line.push_str(&format!("  ({})", record.branch));
        }
        match record.commits {
            0 => {}
            1 => line.push_str("  1 commit"),
            count => line.push_str(&format!("  {count} commits")),
        }
        if !record.restarted.is_empty() {
            line.push_str(&format!("  restarted {}", record.restarted.join(", ")));
        }
        if !record.detail.is_empty() {
            line.push_str(&format!("  - {}", record.detail));
        }
        write(out, &line)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_commands() {
        assert_eq!(
            parse_github(&["webhook", "setup", "--json"]),
            Ok(WebhookSetup {
                rotate: false,
                json: true
            })
        );
        assert!(parse_github(&["webhook"]).is_err());
        assert!(parse_github(&["webhook", "setup", "--force"]).is_err());
        assert_eq!(
            parse_sync(&["status", "--limit", "5", "-o", "json"]),
            Ok(SyncStatus {
                limit: 5,
                json: true
            })
        );
        assert_eq!(
            parse_sync(&["status"]),
            Ok(SyncStatus {
                limit: DEFAULT_LIMIT,
                json: false
            })
        );
        assert!(parse_sync(&["status", "--limit", "x"]).is_err());
    }
}
