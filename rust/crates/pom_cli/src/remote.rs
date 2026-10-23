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
        transport: Option<pom_remote::HostTransport>,
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
    Ports {
        name: String,
        json: bool,
    },
}

pub(crate) fn parse(words: &[&str]) -> Result<HostCommand, String> {
    let (verb, rest) = words
        .split_first()
        .ok_or("host needs add, ls, rm, test or ports")?;
    let args = Args::parse(rest, &["--pom-path", "--transport", "-o", "--output"])?;
    let json = args.has("--json") || args.json()?;
    let name = |verb: &str| -> Result<String, String> {
        match args.positional.as_slice() {
            [name] => Ok(name.clone()),
            _ => Err(format!("host {verb} takes one host name")),
        }
    };
    match *verb {
        "add" => {
            args.allow(&["--pom-path", "--transport", "--json", "-o", "--output"])?;
            let [name, url] = args.positional.as_slice() else {
                return Err(
                    "usage: pom host add <name> ssh://user@host[:port] [--pom-path p] [--transport server|ssh]".into(),
                );
            };
            Ok(HostCommand::Add {
                name: name.clone(),
                url: url.clone(),
                pom_path: args.value(&["--pom-path"]).filter(|path| !path.is_empty()),
                transport: args
                    .value(&["--transport"])
                    .map(|text| pom_remote::HostTransport::parse(&text))
                    .transpose()?,
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
        "ports" => {
            args.allow(&["--json", "-o", "--output"])?;
            Ok(HostCommand::Ports {
                name: name("ports")?,
                json,
            })
        }
        other => Err(format!(
            "unknown host command {other} (add, ls, rm, test, ports)"
        )),
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
            transport,
            json,
        } => add(name, url, pom_path.clone(), *transport, *json, out),
        HostCommand::List { json } => list(*json, out),
        HostCommand::Remove { name, json } => remove(name, *json, out),
        HostCommand::Ports { name, json } => ports(name, *json, out),
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
    transport: Option<pom_remote::HostTransport>,
    json: bool,
    out: &mut dyn Write,
) -> Result<(), String> {
    let path = hosts_path()?;
    let mut hosts = Hosts::load(&path)?;
    let host = Host {
        name: name.to_string(),
        url: HostUrl::parse(url)?,
        pom_path,
        transport,
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

fn ports(name: &str, json: bool, out: &mut dyn Write) -> Result<(), String> {
    Hosts::load(&hosts_path()?)?.get(name)?;
    let records = pom_remote::recorded_ports(&pom_paths::StateDir::from_env(), name);
    if json {
        return json_line(
            out,
            &serde_json::json!({
                "schema": pom_remote::PORTS_SCHEMA,
                "host": name,
                "records": records,
            }),
        );
    }
    let rows: Vec<[String; 5]> = records
        .iter()
        .flat_map(|record| {
            record.forwards.iter().map(|forward| {
                let owner = if record.project.is_empty() {
                    format!("pom forward ({})", record.pid)
                } else {
                    record.project.clone()
                };
                let mut state = forward_state(forward.state).to_string();
                if !forward.message.is_empty() {
                    state = format!("{state}: {}", forward.message);
                }
                let here = if forward.local_port == 0 {
                    "-".to_string()
                } else {
                    forward.address()
                };
                [
                    forward.remote_port.to_string(),
                    here,
                    forward.label.clone(),
                    owner,
                    state,
                ]
            })
        })
        .collect();
    if rows.is_empty() {
        return line(
            out,
            &format!("nothing is forwarded from {name} (forwards live while the app or pom -H {name} forward runs)"),
        );
    }
    let header = [
        format!("ON {name}"),
        "HERE".to_string(),
        "WHAT".to_string(),
        "FOR".to_string(),
        "STATE".to_string(),
    ];
    let mut widths = header.clone().map(|cell| cell.len());
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.len());
        }
    }
    for row in std::iter::once(&header).chain(&rows) {
        let cells: Vec<String> = row
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        line(out, cells.join("  ").trim_end())?;
    }
    Ok(())
}

fn forward_state(state: pom_remote::ForwardState) -> &'static str {
    match state {
        pom_remote::ForwardState::Starting => "starting",
        pom_remote::ForwardState::Forwarded => "forwarded",
        pom_remote::ForwardState::Lost => "reconnecting",
        pom_remote::ForwardState::Failed => "failed",
    }
}

/// `<port>` or `<port>:<local>`.
fn parse_forward(spec: &str) -> Result<(u16, pom_remote::LocalPort), String> {
    let port = |text: &str| -> Result<u16, String> {
        text.parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| format!("{text} is not a port (1-65535)"))
    };
    match spec.split_once(':') {
        Some((remote, local)) => Ok((port(remote)?, pom_remote::LocalPort::Exact(port(local)?))),
        None => Ok((port(spec)?, pom_remote::LocalPort::Same)),
    }
}

/// Blocks the stop signals in this thread and every thread it starts from now on, so one waiting thread
/// takes them and the forward is cancelled before exit.
fn block_stop_signals() -> Option<libc::sigset_t> {
    // SAFETY: the set is initialised by sigemptyset before use and only passed to libc.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::sigaddset(&mut set, signal);
        }
        (libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) == 0).then_some(set)
    }
}

/// `pom -H <host> forward <port>[:<local>]`: holds the forward in the foreground until a stop signal, opening it
/// again when the connection comes back.
pub(crate) fn hold_forward(
    name: &str,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let parsed = Args::parse(&words, &["-o", "--output"]).and_then(|args| {
        args.allow(&["--json", "-o", "--output"])?;
        let json = args.has("--json") || args.json()?;
        match args.positional.as_slice() {
            [spec] => Ok((parse_forward(spec)?, json)),
            _ => Err("usage: pom -H <host> forward <port>[:<local>] [--json]".to_string()),
        }
    });
    let ((remote_port, local), json) = match parsed {
        Ok(parsed) => parsed,
        Err(message) => {
            crate::report(err, &format!("error: {message}"));
            return 2;
        }
    };
    let host = match hosts_path().and_then(|path| Hosts::load(&path)?.get(name)) {
        Ok(host) => host,
        Err(message) => {
            crate::report(err, &format!("error: {message}"));
            return 1;
        }
    };
    let Some(signals) = block_stop_signals() else {
        crate::report(err, "error: could not take over Ctrl-C");
        return 1;
    };
    let state = pom_paths::StateDir::from_env();
    let forwarder = pom_remote::Forwarder::new(pom_remote::Ssh::system(), host).recorded_in(
        &pom_remote::ports_dir(&state, name),
        &format!("cli-{}", std::process::id()),
        "",
    );
    let request = pom_remote::ForwardRequest {
        remote_port,
        local,
        label: remote_port.to_string(),
        kind: pom_remote::ForwardKind::Manual,
    };
    let forward = match forwarder.forward(&request) {
        Ok(forward) => forward,
        Err(message) => {
            crate::report(err, &format!("error: {message}"));
            return 1;
        }
    };
    let said = if json {
        serde_json::to_value(&forward)
            .map_err(|error| error.to_string())
            .and_then(|value| json_line(out, &value))
    } else {
        let moved = if forward.moved() {
            format!(" ({} is in use here)", forward.wanted_port)
        } else {
            String::new()
        };
        line(
            out,
            &format!(
                "{name}:{remote_port} is on http://{}{moved} - Ctrl-C stops it",
                forward.address()
            ),
        )
    };
    if let Err(message) = said.and_then(|()| out.flush().map_err(|error| error.to_string())) {
        crate::report(err, &message);
    }
    let (stopped, stop) = std::sync::mpsc::channel();
    let waiter = std::thread::Builder::new()
        .name("pom-forward-signals".into())
        .spawn(move || {
            let mut signal = 0;
            // SAFETY: `signals` is the set blocked above; sigwait only writes `signal`.
            let waited = unsafe { libc::sigwait(&signals, &mut signal) };
            if stopped.send(waited).is_err() {
                eprintln!("forward: stop signal arrived after exit");
            }
        });
    if let Err(error) = waiter {
        crate::report(err, &format!("error: {error}"));
        return 1;
    }
    let every = std::time::Duration::from_secs(2);
    while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stop.recv_timeout(every) {
        forwarder.reapply();
    }
    forwarder.close();
    0
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
    let local = crate::VERSION;
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
                transport: None,
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
