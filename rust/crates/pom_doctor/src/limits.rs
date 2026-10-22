//! Whether the workspace caps of pom.yml's `limits:` can hold on this machine.

use crate::{Finding, Severity};

/// What the machine offers for caps, as the holder spawner sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LimitSupport {
    pub linux: bool,
    /// `systemd`, `cgroup` (a delegated directory) or `none`.
    pub backend: String,
    /// The controllers the user's cgroup tree offers (`cgroup.controllers`), when it could be read.
    pub controllers: Option<String>,
    /// Whether the user's systemd manager outlives their logins.
    pub linger: bool,
}

pub fn limit_findings(configured: bool, support: &LimitSupport) -> Vec<Finding> {
    if !configured {
        return Vec::new();
    }
    if !support.linux {
        return vec![Finding::new(
            "limits.linux_only",
            Severity::Ok,
            "Workspace limits apply only on Linux",
        )
        .detail(
            "pom.yml has `limits:`; on this machine workspaces run without CPU or memory caps",
        )];
    }
    if support.backend == "none" {
        return vec![Finding::new(
            "limits.unavailable",
            Severity::Warn,
            "Workspace limits are not applied",
        )
        .detail("pom.yml has `limits:`, but there is no systemd user session to put each workspace in a cgroup (no $XDG_RUNTIME_DIR/systemd/private)")
        .fix("run pom from a login or ssh session of a systemd machine, or set POM_CGROUP_ROOT to a cgroup v2 directory delegated to you")];
    }
    let mut findings = Vec::new();
    if let Some(controllers) = &support.controllers {
        let missing: Vec<&str> = ["cpu", "memory"]
            .into_iter()
            .filter(|wanted| !controllers.split_whitespace().any(|have| have == *wanted))
            .collect();
        if !missing.is_empty() {
            findings.push(
                Finding::new(
                    "limits.controllers",
                    Severity::Warn,
                    "Workspace limits cannot be enforced",
                )
                .detail(format!(
                    "your cgroup tree lacks the {} controller(s), so the caps are set but not applied",
                    missing.join(" and ")
                ))
                .fix("delegate them to user sessions: a drop-in for user@.service with `Delegate=cpu cpuset io memory pids`, then `systemctl daemon-reload` and log in again"),
            );
        }
    }
    if support.backend == "systemd" && !support.linger {
        findings.push(
            Finding::new(
                "limits.linger",
                Severity::Warn,
                "Limited workspaces stop when you log out",
            )
            .detail("with limits, a workspace's services and agents run under your systemd user manager, which stops at your last logout unless lingering is on")
            .fix("loginctl enable-linger $USER"),
        );
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|finding| finding.id.as_str()).collect()
    }

    #[test]
    fn limits_report_what_keeps_them_from_holding() {
        let linux = |backend: &str, controllers: Option<&str>, linger: bool| LimitSupport {
            linux: true,
            backend: backend.into(),
            controllers: controllers.map(str::to_string),
            linger,
        };
        assert!(limit_findings(false, &LimitSupport::default()).is_empty());
        assert_eq!(
            ids(&limit_findings(true, &LimitSupport::default())),
            ["limits.linux_only"]
        );
        assert_eq!(
            ids(&limit_findings(true, &linux("none", None, true))),
            ["limits.unavailable"]
        );
        assert!(limit_findings(true, &linux("systemd", Some("cpu memory pids"), true)).is_empty());
        assert_eq!(
            ids(&limit_findings(
                true,
                &linux("systemd", Some("memory pids"), false)
            )),
            ["limits.controllers", "limits.linger"]
        );
        assert!(limit_findings(true, &linux("cgroup", Some("cpu memory"), false)).is_empty());
    }
}
