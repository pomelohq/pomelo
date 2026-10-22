//! `pom host` (the hosts in `hosts.json`) and `pom -H <host> <command>` (run pom there over ssh).

use std::io::{Read, Write};
use std::path::PathBuf;

use pom_remote::{Host, HostUrl, Hosts, Tty, EXIT_UNREACHABLE};

use crate::args::Args;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HostCommand {
    Add {
        name: String,
        url: String,
        pom_path: Option<String>,
        json: bool,
    },
    List {
        json: bool,
    },
    Remove {
        name: String,
        json: bool,
    },
    Test {
        name: String,
        json: bool,
    },
}

pub(crate) fn parse(words: &[&str]) -> Result<HostCommand, String> {
    let (verb, rest) = words
        .split_first()
        .ok_or("host needs add, ls, rm or test")?;
    let args = Args::parse(rest, &["--pom-path", "-o", "--output"])?;
    let json = args.has("--json") || args.json()?;
    let name = |verb: &str| -> Result<String, String> {
        match args.positional.as_slice() {
            [name] => Ok(name.clone()),
            _ => Err(format!("host {verb} takes one host name")),
        }
    };
    match *verb {
        "add" => {
            args.allow(&["--pom-path", "--json", "-o", "--output"])?;
            let [name, url] = args.positional.as_slice() else {
                return Err(
                    "usage: pom host add <name> ssh://user@host[:port] [--pom-path p]".into(),
                );
            };
            Ok(HostCommand::Add {
                name: name.clone(),
                url: url.clone(),
                pom_path: args.value(&["--pom-path"]).filter(|path| !path.is_empty()),
                json,
            })
        }
        "ls" | "list" => {
            args.allow(&["--json", "-o", "--output"])?;
            args.at_most(0, "host ls")?;
            Ok(HostCommand::List { json })
        }
        "rm" | "remove" => {
            args.allow(&["--json", "-o", "--output"])?;
            Ok(HostCommand::Remove {
                name: name("rm")?,
                json,
            })
        }
        "test" => {
            args.allow(&["--json", "-o", "--output"])?;
            Ok(HostCommand::Test {
                name: name("test")?,
                json,
            })
        }
        other => Err(format!("unknown host command {other} (add, ls, rm, test)")),
    }
}

fn hosts_path() -> Result<PathBuf, String> {
    Hosts::default_path().ok_or_else(|| "no home directory for ~/.config/pomelo".to_string())
}

fn line(out: &mut dyn Write, text: &str) -> Result<(), String> {
    writeln!(out, "{text}").map_err(|error| error.to_string())
}

fn json_line(out: &mut dyn Write, value: &serde_json::Value) -> Result<(), String> {
    line(out, &value.to_string())
}

/// Runs a `pom host` command; returns the exit code.
pub(crate) fn execute(command: &HostCommand, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let result = match command {
        HostCommand::Test { name, json } => return test(name, *json, out, err),
        HostCommand::Add {
            name,
            url,
            pom_path,
            json,
        } => add(name, url, pom_path.clone(), *json, out),
        HostCommand::List { json } => list(*json, out),
        HostCommand::Remove { name, json } => remove(name, *json, out),
    };
    match result {
        Ok(()) => 0,
        Err(message) => {
            crate::report(err, &format!("error: {message}"));
            1
        }
    }
}

fn add(
    name: &str,
    url: &str,
    pom_path: Option<String>,
    json: bool,
    out: &mut dyn Write,
) -> Result<(), String> {
    let path = hosts_path()?;
    let mut hosts = Hosts::load(&path)?;
    let host = Host {
        name: name.to_string(),
        url: HostUrl::parse(url)?,
        pom_path,
    };
    let replaced = hosts.insert(&host)?;
    hosts.save(&path)?;
    if json {
        return json_line(
            out,
            &serde_json::json!({"schema": pom_remote::SCHEMA, "host": host.json(), "replaced": replaced}),
        );
    }
    let verb = if replaced { "updated" } else { "added" };
    line(out, &format!("{verb} host {name} ({})", host.url.url()))?;
    line(out, &format!("  check it: pom host test {name}"))
}

fn list(json: bool, out: &mut dyn Write) -> Result<(), String> {
    let hosts = Hosts::load(&hosts_path()?)?;
    let all = hosts.all();
    if json {
        let entries: Vec<serde_json::Value> = all.iter().map(Host::json).collect();
        return json_line(
            out,
            &serde_json::json!({"schema": pom_remote::SCHEMA, "hosts": entries}),
        );
    }
    if all.is_empty() {
        return line(
            out,
            "no hosts; add one with: pom host add <name> ssh://user@host",
        );
    }
    let width = all.iter().map(|host| host.name.len()).max().unwrap_or(0);
    for host in &all {
        let mut text = format!("{:width$}  {}", host.name, host.url.url());
        if let Some(pom_path) = &host.pom_path {
            text.push_str(&format!("  pom: {pom_path}"));
        }
        line(out, &text)?;
    }
    Ok(())
}

fn remove(name: &str, json: bool, out: &mut dyn Write) -> Result<(), String> {
    let path = hosts_path()?;
    let mut hosts = Hosts::load(&path)?;
    if !hosts.remove(name) {
        return Err(format!("no host {name}"));
    }
    hosts.save(&path)?;
    if json {
        return json_line(
            out,
            &serde_json::json!({"schema": pom_remote::SCHEMA, "removed": name}),
        );
    }
    line(out, &format!("removed host {name}"))
}

/// Connects, asks the remote pom for its version and compares it with this one.
fn test(name: &str, json: bool, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let host = match hosts_path().and_then(|path| Hosts::load(&path)?.get(name)) {
        Ok(host) => host,
        Err(message) => {
            crate::report(err, &format!("error: {message}"));
            return 1;
        }
    };
    let local = env!("CARGO_PKG_VERSION");
    let output = pom_remote::ssh_command(&host, &["--version".to_string()], Tty::Batch)
        .stdin(std::process::Stdio::null())
        .output();
    let mut document = serde_json::json!({
        "schema": pom_remote::SCHEMA,
        "host": host.json(),
        "local_version": local,
    });
    let (code, problem) = match output {
        Err(error) => (1, Some(format!("could not run ssh: {error}"))),
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let status = output.status.code().unwrap_or(-1);
            match (status, pom_remote::parse_version(&stdout)) {
                (255, _) => (
                    EXIT_UNREACHABLE,
                    Some(format!("could not connect: {}", last_line(&stderr))),
                ),
                (0, Some(remote)) => {
                    document["reachable"] = true.into();
                    document["remote_version"] = remote.clone().into();
                    let mismatch = pom_remote::version_mismatch(local, &remote);
                    document["version_mismatch"] = mismatch.into();
                    if !json {
                        let report = line(
                            out,
                            &format!("{name}: connected, pom {remote} (this machine: {local})"),
                        );
                        if let Err(message) = report {
                            crate::report(err, &message);
                        }
                    }
                    if mismatch {
                        crate::report(
                            err,
                            &format!(
                                "warning: {name} runs pom {remote} and this machine {local}; \
                                 update one so both are on the same minor version"
                            ),
                        );
                    }
                    (0, None)
                }
                (127, _) => (
                    1,
                    Some(format!(
                        "connected, but pom is not installed on {name} (looked in {}); install it there or set --pom-path",
                        host.pom_path.as_deref().unwrap_or("~/.local/bin/pom and PATH")
                    )),
                ),
                (status, _) => (
                    1,
                    Some(format!(
                        "connected, but `pom --version` failed (exit {status}): {}",
                        last_line(&stderr)
                    )),
                ),
            }
        }
    };
    if let Some(problem) = &problem {
        document["reachable"] = (code != EXIT_UNREACHABLE).into();
        document["error"] = problem.clone().into();
        crate::report(err, &format!("error: {problem}"));
    }
    if json {
        if let Err(message) = json_line(out, &document) {
            crate::report(err, &message);
        }
    }
    code
}

fn last_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("no message from ssh")
        .to_string()
}

/// Pulls `-H <host>` / `--host <host>` / `--host=<host>` out from anywhere before a `--`.
pub(crate) fn extract_host(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let mut host = None;
    let mut rest = Vec::with_capacity(args.len());
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            rest.push(arg.clone());
            rest.extend(iter.by_ref().cloned());
            break;
        }
        if let Some(name) = arg.strip_prefix("--host=") {
            host = Some(name.to_string());
            continue;
        }
        if matches!(arg.as_str(), "-H" | "--host") {
            host = Some(
                iter.next()
                    .ok_or_else(|| format!("{arg} needs a host name"))?
                    .clone(),
            );
            continue;
        }
        rest.push(arg.clone());
    }
    if host.as_deref() == Some("") {
        return Err("-H needs a host name".into());
    }
    Ok((host, rest))
}

/// `pom -H <host> <args...>`: pom on the host over ssh, stdin and stdout passed through, stderr relayed, its
/// exit code returned. ssh's own failure becomes [`EXIT_UNREACHABLE`] (with a JSON error when asked for JSON).
pub(crate) fn forward(
    name: &str,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let json = pom_remote::json_requested(args);
    let fail = |out: &mut dyn Write, err: &mut dyn Write, code: &str, message: &str, exit: i32| {
        if json {
            let document = serde_json::json!({
                "schema": pom_remote::SCHEMA,
                "error": {"code": code, "host": name, "message": message},
            });
            if let Err(error) = json_line(out, &document) {
                crate::report(err, &error);
            }
        }
        crate::report(err, &format!("error: {message}"));
        exit
    };
    let host = match hosts_path().and_then(|path| Hosts::load(&path)?.get(name)) {
        Ok(host) => host,
        Err(message) => return fail(out, err, "unknown_host", &message, 1),
    };
    if let Err(error) = out.flush() {
        crate::report(err, &error.to_string());
    }
    let mut child = match pom_remote::ssh_command(&host, args, Tty::detect())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let message = format!("could not run ssh: {error}");
            return fail(out, err, "ssh_missing", &message, EXIT_UNREACHABLE);
        }
    };
    let mut tail = Vec::new();
    if let Some(mut stderr) = child.stderr.take() {
        let mut buffer = [0u8; 8192];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let chunk = &buffer[..read];
                    if let Err(error) = err.write_all(chunk).and_then(|()| err.flush()) {
                        eprintln!("{error}");
                    }
                    tail.extend_from_slice(chunk);
                    if tail.len() > 16 * 1024 {
                        tail.drain(..tail.len() - 8 * 1024);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    }
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            let message = format!("ssh: {error}");
            return fail(out, err, "ssh_failed", &message, EXIT_UNREACHABLE);
        }
    };
    match status.code() {
        Some(255) => {
            let reason = last_line(&String::from_utf8_lossy(&tail));
            let message =
                format!("could not reach host {name} over ssh ({reason}); check it with: pom host test {name}");
            fail(out, err, "unreachable", &message, EXIT_UNREACHABLE)
        }
        Some(code) => code,
        None => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_host_flag_comes_out_from_anywhere_before_a_double_dash() {
        assert_eq!(
            extract_host(&strings("-H dev ws ls --json")),
            Ok((Some("dev".into()), strings("ws ls --json")))
        );
        assert_eq!(
            extract_host(&strings("status -o json --host dev")),
            Ok((Some("dev".into()), strings("status -o json")))
        );
        assert_eq!(
            extract_host(&strings("-w feat --host=lab logs api/web")),
            Ok((Some("lab".into()), strings("-w feat logs api/web")))
        );
        assert_eq!(
            extract_host(&strings("run -- curl -H x")),
            Ok((None, strings("run -- curl -H x")))
        );
        assert_eq!(
            extract_host(&strings("-h status")),
            Ok((None, strings("-h status")))
        );
        assert!(extract_host(&strings("status -H")).is_err());
        assert!(extract_host(&strings("--host= status")).is_err());
    }

    #[test]
    fn host_commands_parse() {
        assert_eq!(
            parse(&[
                "add",
                "dev",
                "ssh://dev@192.0.2.10",
                "--pom-path",
                "/opt/pom"
            ]),
            Ok(HostCommand::Add {
                name: "dev".into(),
                url: "ssh://dev@192.0.2.10".into(),
                pom_path: Some("/opt/pom".into()),
                json: false,
            })
        );
        assert_eq!(
            parse(&["ls", "--json"]),
            Ok(HostCommand::List { json: true })
        );
        assert_eq!(
            parse(&["test", "dev", "-o", "json"]),
            Ok(HostCommand::Test {
                name: "dev".into(),
                json: true
            })
        );
        assert!(parse(&["add", "dev"]).is_err());
        assert!(parse(&["rm"]).is_err());
        assert!(parse(&["frob"]).is_err());
    }
}
