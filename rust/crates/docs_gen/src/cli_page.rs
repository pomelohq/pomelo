use crate::{code, prose};

const INTRO: &str = "\
# pom CLI

`pom` drives the same workspaces, services and holders as the app, so a
service started in a terminal shows up in the Services panel and the other
way round. It ships inside the app at
`/Applications/Pomelo.app/Contents/MacOS/pom`; add that folder to your `PATH`
to run it in a terminal. `pom help` prints this list.
";

struct Group {
    name: String,
    commands: Vec<(String, String)>,
}

/// Reads the usage text: unindented headings, two-space-indented commands with their description after a
/// gap of two or more spaces, and deeper-indented lines continuing the description above.
fn parse(usage: &str) -> Result<(String, Vec<Group>, Vec<String>), String> {
    let mut lines = usage.lines();
    let synopsis = lines
        .next()
        .and_then(|line| line.strip_prefix("usage: "))
        .ok_or("the usage text does not start with `usage: `")?
        .to_string();
    let lines: Vec<&str> = lines.collect();
    let mut groups: Vec<Group> = Vec::new();
    let mut notes = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let indent = line.len() - line.trim_start().len();
        if line.trim().is_empty() {
            continue;
        }
        if indent == 0 {
            let opens_group = lines
                .get(index + 1)
                .is_some_and(|next| next.starts_with("  "));
            if opens_group {
                groups.push(Group {
                    name: line.to_string(),
                    commands: Vec::new(),
                });
            } else {
                notes.push(line.to_string());
            }
            continue;
        }
        let group = groups
            .last_mut()
            .ok_or("a command comes before any heading")?;
        if indent == 2 {
            let entry = line.trim();
            let (command, description) = match entry.find("  ") {
                Some(gap) => (&entry[..gap], entry[gap..].trim()),
                None => (entry, ""),
            };
            group
                .commands
                .push((command.to_string(), description.to_string()));
        } else {
            let (_, description) = group
                .commands
                .last_mut()
                .ok_or("a continuation line comes before any command")?;
            if !description.is_empty() {
                description.push(' ');
            }
            description.push_str(line.trim());
        }
    }
    Ok((synopsis, groups, notes))
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub fn render() -> Result<String, String> {
    let (synopsis, groups, notes) = parse(pom_cli::USAGE)?;
    let mut page = String::from(INTRO);
    page.push_str(&format!("\n```\n{synopsis}\n```\n\n"));
    for note in notes {
        page.push_str(&format!("{}\n", prose(&note)));
    }
    for group in groups {
        page.push_str(&format!(
            "\n## {}\n\n| Command | What it does |\n| --- | --- |\n",
            capitalized(&group.name)
        ));
        for (command, description) in group.commands {
            let description = if description.is_empty() {
                "-".to_string()
            } else {
                capitalized(&description)
            };
            page.push_str(&format!(
                "| {} | {} |\n",
                code(&format!("pom {command}")),
                prose(&description)
            ));
        }
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_usage_line_lands_in_a_group() {
        let (synopsis, groups, notes) = parse(pom_cli::USAGE).unwrap_or_default();
        assert!(synopsis.starts_with("pom "));
        assert!(groups.len() >= 4);
        let commands: usize = groups.iter().map(|group| group.commands.len()).sum();
        let indented = pom_cli::USAGE
            .lines()
            .filter(|line| line.starts_with("  ") && !line.starts_with("   "))
            .count();
        assert_eq!(commands, indented);
        assert!(!notes.is_empty());
    }

    #[test]
    fn continuation_lines_join_their_command() {
        let usage = "usage: pom x\n\ngroup\n  run <a>\n                     first\n                     second\n  ls   list\n";
        let (_, groups, _) = parse(usage).unwrap_or_default();
        assert_eq!(
            groups[0].commands,
            vec![
                ("run <a>".to_string(), "first second".to_string()),
                ("ls".to_string(), "list".to_string()),
            ]
        );
    }
}
