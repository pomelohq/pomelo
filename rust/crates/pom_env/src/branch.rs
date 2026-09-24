use sha1::{Digest, Sha1};

const DNS_LABEL_MAX: usize = 63;

/// Branch name usable in file names, database names and holder names.
pub fn branch_safe(branch: &str) -> String {
    branch.replace('/', "_")
}

pub fn branch_hash(branch: &str) -> String {
    let digest = Sha1::digest(branch.as_bytes());
    digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A valid DNS label: lowercase `[a-z0-9-]`, no leading/trailing dash, at most 63 bytes. Long
/// names keep a hash suffix so two branches sharing a long prefix never collide.
pub fn branch_host(branch: &str) -> String {
    let mapped: String = branch
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let label = mapped.trim_matches('-');
    if label.len() <= DNS_LABEL_MAX {
        return label.to_string();
    }
    let suffix = branch_hash(branch);
    let prefix = label[..DNS_LABEL_MAX - suffix.len() - 1].trim_matches('-');
    format!("{prefix}-{suffix}")
}

/// Short hostname label for a workspace: the leading ticket id (`proj-1147`) when the branch
/// starts with one, else the full host label.
pub fn workspace_label(branch: &str) -> String {
    let host = branch_host(branch);
    let letters = host.bytes().take_while(u8::is_ascii_lowercase).count();
    if letters > 0 && host.as_bytes().get(letters) == Some(&b'-') {
        let digits = host[letters + 1..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if digits > 0 {
            return host[..letters + 1 + digits].to_string();
        }
    }
    host
}

/// Key a workspace's state is filed under; the main checkout and a same-named branch worktree
/// must never share one.
pub fn ws_key(branch: &str, is_main: bool) -> String {
    if is_main {
        format!("main:{branch}")
    } else {
        format!("ws:{branch}")
    }
}

pub fn port_ws_key(branch: &str) -> String {
    format!("ws-{}", branch_safe(branch))
}

/// Deterministic fallback port for a shared service before any lease exists: FNV-1a over
/// `session/name`, folded into 20000..30000.
pub fn stable_shared_port(session: &str, name: &str) -> u16 {
    const BASE: u32 = 20000;
    const SPAN: u32 = 10000;
    let mut hash: u32 = 2_166_136_261;
    for c in session
        .chars()
        .chain(std::iter::once('/'))
        .chain(name.chars())
    {
        hash = (hash ^ u32::from(c)).wrapping_mul(16_777_619);
    }
    u16::try_from(BASE + hash % SPAN).unwrap_or(u16::MAX)
}

pub fn resolve_branch_tokens(text: &str, branch: &str) -> String {
    if !text.contains("{{branch") {
        return text.to_string();
    }
    let (safe, hash, host) = (
        branch_safe(branch),
        branch_hash(branch),
        branch_host(branch),
    );
    let replacements: [(&str, &str); 10] = [
        ("{{branch.safe}}", &safe),
        ("{{branch|safe}}", &safe),
        ("{{branch_safe}}", &safe),
        ("{{branch.hash}}", &hash),
        ("{{branch|hash}}", &hash),
        ("{{branch_hash}}", &hash),
        ("{{branch.host}}", &host),
        ("{{branch|host}}", &host),
        ("{{branch_host}}", &host),
        ("{{branch}}", branch),
    ];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        match replacements
            .iter()
            .find(|(token, _)| rest.starts_with(token))
        {
            Some((token, value)) => {
                out.push_str(value);
                rest = &rest[token.len()..];
            }
            None => {
                let next = rest.chars().next().map_or(1, char::len_utf8);
                out.push_str(&rest[..next]);
                rest = &rest[next..];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_labels() {
        for (input, want) in [
            ("proj-855", "proj-855"),
            ("PROJ-855", "proj-855"),
            ("feat/add-login", "feat-add-login"),
            ("bss_send_confirm", "bss-send-confirm"),
            ("-weird-/", "weird"),
        ] {
            assert_eq!(branch_host(input), want, "{input}");
        }
    }

    #[test]
    fn long_host_is_capped_hashed_and_distinct() {
        let long =
            "proj-1069-rent-manager-let-partners-map-their-own-custom-fields-udfs-to-acme-fields";
        let host = branch_host(long);
        assert!(host.len() <= DNS_LABEL_MAX, "{host}");
        assert!(!host.starts_with('-') && !host.ends_with('-'));
        assert!(host.ends_with(&branch_hash(long)));
        assert_ne!(host, branch_host(&format!("{long}-extra-tail")));
    }

    #[test]
    fn workspace_labels_prefer_ticket_ids() {
        assert_eq!(workspace_label("proj-1147-add-log"), "proj-1147");
        assert_eq!(workspace_label("PROJ-12/fix"), "proj-12");
        assert_eq!(workspace_label("feat/add-login"), "feat-add-login");
        assert_eq!(workspace_label("proj-x"), "proj-x");
        assert_eq!(workspace_label("main"), "main");
    }

    #[test]
    fn safe_hash_and_keys() {
        assert_eq!(branch_safe("feat/x/y"), "feat_x_y");
        assert_eq!(branch_hash("main"), "b28b7af6");
        assert_eq!(ws_key("main", true), "main:main");
        assert_eq!(ws_key("feat", false), "ws:feat");
        assert_eq!(port_ws_key("feat/x"), "ws-feat_x");
    }

    #[test]
    fn branch_tokens_only() {
        assert_eq!(
            resolve_branch_tokens("app_{{branch.safe}}_{{branch}}_{{db.x}}", "feat/x"),
            "app_feat_x_feat/x_{{db.x}}"
        );
        assert_eq!(resolve_branch_tokens("{{branch_safe}}", "a/b"), "a_b");
        assert_eq!(resolve_branch_tokens("plain", "a/b"), "plain");
    }

    #[test]
    fn stable_port_is_in_range_and_deterministic() {
        let port = stable_shared_port("acme", "postgres");
        assert!((20000..30000).contains(&port));
        assert_eq!(port, stable_shared_port("acme", "postgres"));
        assert_ne!(port, stable_shared_port("acme", "redis"));
    }
}
