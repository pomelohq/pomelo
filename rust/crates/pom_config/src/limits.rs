use indexmap::IndexMap;

/// A CPU and memory cap as written in pom.yml; empty means no cap.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceLimit {
    /// Cores (`2`, `1.5`) or a percentage of one core (`200%`).
    pub cpu: String,
    /// Bytes with an optional K, M, G or T suffix (`4G`, `512M`).
    pub memory: String,
}

impl ResourceLimit {
    pub fn is_empty(&self) -> bool {
        self.cpu.trim().is_empty() && self.memory.trim().is_empty()
    }

    /// The CPU cap in percent of one core; `None` when unset or unreadable (validation reports the latter).
    pub fn cpu_percent(&self) -> Option<u32> {
        parse_cpu(&self.cpu).ok().flatten()
    }

    pub fn memory_bytes(&self) -> Option<u64> {
        parse_memory(&self.memory).ok().flatten()
    }

    pub(crate) fn check(&self, context: &str, errors: &mut Vec<String>) {
        if let Err(error) = parse_cpu(&self.cpu) {
            errors.push(format!("{context} cpu: {error}"));
        }
        if let Err(error) = parse_memory(&self.memory) {
            errors.push(format!("{context} memory: {error}"));
        }
    }
}

/// `limits:` in pom.yml: a cap for every workspace's processes, and overrides for some workspaces.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LimitsConfig {
    pub default: ResourceLimit,
    /// Branch -> its own cap; a field it leaves empty falls back to the default.
    pub workspaces: IndexMap<String, ResourceLimit>,
}

impl LimitsConfig {
    pub fn is_empty(&self) -> bool {
        self.default.is_empty() && self.workspaces.values().all(ResourceLimit::is_empty)
    }

    pub fn for_workspace(&self, branch: &str) -> ResourceLimit {
        let own = self.workspaces.get(branch).cloned().unwrap_or_default();
        let pick = |own: String, default: &String| {
            if own.trim().is_empty() {
                default.clone()
            } else {
                own
            }
        };
        ResourceLimit {
            cpu: pick(own.cpu, &self.default.cpu),
            memory: pick(own.memory, &self.default.memory),
        }
    }

    pub(crate) fn check(&self, errors: &mut Vec<String>) {
        self.default.check("limits", errors);
        for (branch, limit) in &self.workspaces {
            limit.check(&format!("limits.workspaces.{branch}"), errors);
        }
    }
}

/// `2` or `1.5` cores, or `200%`, as percent of one core; empty is no cap.
pub fn parse_cpu(text: &str) -> Result<Option<u32>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let invalid =
        || format!("{text:?} is not a CPU amount (cores like 2 or 1.5, or a percentage like 200%)");
    let percent = match text.strip_suffix('%') {
        Some(number) => number.trim().parse::<f64>().map_err(|_| invalid())?,
        None => text.parse::<f64>().map_err(|_| invalid())? * 100.0,
    };
    if !percent.is_finite() || percent < 1.0 {
        return Err(invalid());
    }
    Ok(Some(percent.round() as u32))
}

/// `4G`, `512M`, `1.5G`, `2048K` or plain bytes; binary units (`G` = 1024^3); `Gi`, `GB` read the same.
pub fn parse_memory(text: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let invalid = || format!("{text:?} is not a memory size (like 4G, 512M or a byte count)");
    let upper = text.to_ascii_uppercase();
    let unit_start = upper
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(upper.len());
    let (number, unit) = upper.split_at(unit_start);
    let scale: u64 = match unit.trim_end_matches('B').trim_end_matches('I') {
        "" => 1,
        "K" => 1 << 10,
        "M" => 1 << 20,
        "G" => 1 << 30,
        "T" => 1 << 40,
        _ => return Err(invalid()),
    };
    let value = number.trim().parse::<f64>().map_err(|_| invalid())?;
    let bytes = value * scale as f64;
    // Below a megabyte nothing useful runs, and the kernel would OOM-kill at once.
    if !bytes.is_finite() || bytes < (1 << 20) as f64 {
        return Err(invalid());
    }
    Ok(Some(bytes as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_reads_cores_and_percentages() {
        assert_eq!(parse_cpu(""), Ok(None));
        assert_eq!(parse_cpu("2"), Ok(Some(200)));
        assert_eq!(parse_cpu("1.5"), Ok(Some(150)));
        assert_eq!(parse_cpu("250%"), Ok(Some(250)));
        assert!(parse_cpu("two").is_err());
        assert!(parse_cpu("0").is_err());
    }

    #[test]
    fn memory_reads_binary_units() {
        assert_eq!(parse_memory(""), Ok(None));
        assert_eq!(parse_memory("256M"), Ok(Some(256 << 20)));
        assert_eq!(parse_memory("4G"), Ok(Some(4 << 30)));
        assert_eq!(parse_memory("4Gi"), Ok(Some(4 << 30)));
        assert_eq!(parse_memory("1.5g"), Ok(Some(3 << 29)));
        assert_eq!(parse_memory("2097152"), Ok(Some(2 << 20)));
        assert!(parse_memory("4Q").is_err());
        assert!(parse_memory("10K").is_err());
    }

    #[test]
    fn a_workspace_override_falls_back_field_by_field() {
        let mut limits = LimitsConfig {
            default: ResourceLimit {
                cpu: "2".into(),
                memory: "4G".into(),
            },
            ..LimitsConfig::default()
        };
        limits.workspaces.insert(
            "feat-heavy".into(),
            ResourceLimit {
                cpu: String::new(),
                memory: "8G".into(),
            },
        );
        let heavy = limits.for_workspace("feat-heavy");
        assert_eq!(
            (heavy.cpu_percent(), heavy.memory_bytes()),
            (Some(200), Some(8 << 30))
        );
        assert_eq!(limits.for_workspace("other"), limits.default);
    }
}
