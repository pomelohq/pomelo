//! Workspaces from the app: the create and rename forms, and the queue that runs creations and deletions in
//! the background while the WORKSPACES panel shows their progress.

mod forms;
mod ops;
mod ticket_picker;
mod tickets;

use std::sync::Arc;

pub use forms::{CreateWorkspace, CreateWorkspaceModal, RenameWorkspace, RenameWorkspaceModal};
pub use ops::{apply_event, Finished, OpContext, OpKind, OpQueue};
pub use tickets::{TicketSource, TicketStatuses};

/// Asks for a display name and branch slug from a seed slug and a description (Claude in the app).
pub type Namer = Arc<dyn Fn(&str, &str) -> Result<pom_agent::NameSuggestion, String> + Send + Sync>;

/// Lowercase letters and digits, every other run of characters one `-`, none at the ends.
pub fn slugify(text: &str) -> String {
    let mut slug = String::new();
    for c in text.to_lowercase().chars() {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_string()
}

/// `proj-101-fix-login` -> `PROJ-101 Fix login`: a leading ticket key stays one uppercase word.
pub fn humanize_branch(branch: &str) -> String {
    let words: Vec<&str> = branch
        .split(['-', '_', '/'])
        .filter(|word| !word.is_empty())
        .collect();
    let (key, rest) = match words.as_slice() {
        [letters, digits, rest @ ..]
            if letters.chars().all(|c| c.is_ascii_alphabetic())
                && digits.chars().all(|c| c.is_ascii_digit()) =>
        {
            (Some(format!("{}-{digits}", letters.to_uppercase())), rest)
        }
        _ => (None, words.as_slice()),
    };
    let sentence = rest.join(" ");
    let mut chars = sentence.chars();
    let sentence: String = chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    match key {
        Some(key) if sentence.is_empty() => key,
        Some(key) => format!("{key} {sentence}"),
        None => sentence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_keep_letters_and_digits() {
        assert_eq!(slugify("Fix the Login page!"), "fix-the-login-page");
        assert_eq!(slugify("  PROJ-101  "), "proj-101");
        assert_eq!(slugify("--"), "");
        assert_eq!(slugify("Cafe noi"), "cafe-noi");
    }

    #[test]
    fn branches_read_as_titles() {
        assert_eq!(humanize_branch("proj-101-fix-login"), "PROJ-101 Fix login");
        assert_eq!(humanize_branch("proj-101"), "PROJ-101");
        assert_eq!(humanize_branch("feat/new-sidebar"), "Feat new sidebar");
        assert_eq!(humanize_branch(""), "");
    }
}
