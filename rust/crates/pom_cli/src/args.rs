//! Positional words and `--flag [value]` / `-f value` flags of one subcommand.

#[derive(Debug, Default)]
pub(crate) struct Args {
    pub positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

impl Args {
    /// `valued` names the flags that take a value (`--branch x`, `--branch=x`, `-o x`).
    pub fn parse(words: &[&str], valued: &[&str]) -> Result<Args, String> {
        let mut args = Args::default();
        let mut iter = words.iter();
        while let Some(word) = iter.next() {
            if !word.starts_with('-') || *word == "-" {
                args.positional.push(word.to_string());
                continue;
            }
            let (name, inline) = match word.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (*word, None),
            };
            let value = match (valued.contains(&name), inline) {
                (true, Some(value)) => Some(value),
                (true, None) => Some(
                    iter.next()
                        .ok_or_else(|| format!("{name} needs a value"))?
                        .to_string(),
                ),
                (false, Some(_)) => return Err(format!("{name} takes no value")),
                (false, None) => None,
            };
            args.flags.push((name.to_string(), value));
        }
        Ok(args)
    }

    pub fn allow(&self, known: &[&str]) -> Result<(), String> {
        match self
            .flags
            .iter()
            .find(|(name, _)| !known.contains(&name.as_str()))
        {
            Some((name, _)) => Err(format!("unknown flag {name}")),
            None => Ok(()),
        }
    }

    pub fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(flag, _)| flag == name)
    }

    pub fn value(&self, names: &[&str]) -> Option<String> {
        self.flags
            .iter()
            .rev()
            .find(|(flag, _)| names.contains(&flag.as_str()))
            .and_then(|(_, value)| value.clone())
    }

    pub fn values(&self, name: &str) -> Vec<String> {
        self.flags
            .iter()
            .filter(|(flag, _)| flag == name)
            .filter_map(|(_, value)| value.clone())
            .collect()
    }

    pub fn json(&self) -> Result<bool, String> {
        match self.value(&["-o", "--output"]).as_deref() {
            None | Some("table") => Ok(false),
            Some("json") => Ok(true),
            Some(other) => Err(format!("unknown output format {other} (table or json)")),
        }
    }

    pub fn at_most(&self, count: usize, what: &str) -> Result<(), String> {
        if self.positional.len() > count {
            return Err(format!("{what} takes at most {count} argument(s)"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_values_and_positionals() {
        let args = Args::parse(
            &[
                "api",
                "--branch=feat",
                "-o",
                "json",
                "--yes",
                "--repo",
                "a",
                "--repo",
                "b",
            ],
            &["--branch", "-o", "--repo"],
        )
        .expect("parse");
        assert_eq!(args.positional, vec!["api"]);
        assert_eq!(args.value(&["--branch"]).as_deref(), Some("feat"));
        assert_eq!(args.json(), Ok(true));
        assert!(args.has("--yes"));
        assert_eq!(args.values("--repo"), vec!["a", "b"]);
        assert!(args.allow(&["--branch", "-o", "--yes"]).is_err());
        assert!(Args::parse(&["--branch"], &["--branch"]).is_err());
        assert!(Args::parse(&["--yes=1"], &[]).is_err());
    }
}
