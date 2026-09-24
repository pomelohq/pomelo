/// One `{{ key | filter | ... }}` occurrence, with its byte range in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    pub start: usize,
    pub end: usize,
    pub key: &'a str,
    pub filters: Vec<&'a str>,
}

/// Scans left to right; an opening `{{` without a closing `}}` ends the scan and leaves the
/// rest as literal text.
pub fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut offset = 0;
    while let Some(open) = text[offset..].find("{{") {
        let start = offset + open;
        let Some(close) = text[start..].find("}}") else {
            break;
        };
        let end = start + close + 2;
        let mut parts = text[start + 2..start + close].split('|');
        let key = parts.next().unwrap_or_default().trim();
        out.push(Token {
            start,
            end,
            key,
            filters: parts.map(str::trim).collect(),
        });
        offset = end;
    }
    out
}

/// Replaces every token the lookup knows; unknown tokens stay verbatim so a typo is visible in
/// the output instead of silently becoming empty.
pub fn resolve(
    text: &str,
    mut lookup: impl FnMut(&str) -> Option<String>,
    filter: impl Fn(&str, String) -> String,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for token in tokens(text) {
        out.push_str(&text[cursor..token.start]);
        match lookup(token.key) {
            Some(value) => {
                let value = token
                    .filters
                    .iter()
                    .fold(value, |value, name| filter(name, value));
                out.push_str(&value);
            }
            None => out.push_str(&text[token.start..token.end]),
        }
        cursor = token.end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Distinct token keys in first-seen order.
pub fn refs(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in tokens(text) {
        if !token.key.is_empty() && !out.iter().any(|key| key == token.key) {
            out.push(token.key.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(key: &str) -> Option<String> {
        let value = match key {
            "branch" => "proj-1147",
            "service.api.server" => "http://server.api.proj-1147.localhost:8767",
            "shared.postgres" => "user:pw@localhost:5432",
            "db.main" => "acme_proj_1147",
            _ => return None,
        };
        Some(value.to_string())
    }

    fn filter(name: &str, value: String) -> String {
        match name {
            "safe" => value.replace('-', "_"),
            "ws" => value.replacen("http", "ws", 1),
            "port" => value.rsplit(':').next().unwrap_or_default().to_string(),
            _ => value,
        }
    }

    #[test]
    fn resolves_known_tokens_and_keeps_unknown() {
        let cases = [
            ("{{branch}}", "proj-1147"),
            ("{{branch|safe}}", "proj_1147"),
            ("{{ branch | safe }}", "proj_1147"),
            ("pre-{{branch}}-post", "pre-proj-1147-post"),
            (
                "{{service.api.server|ws}}",
                "ws://server.api.proj-1147.localhost:8767",
            ),
            ("{{service.api.server|port}}", "8767"),
            ("DB={{db.main}}", "DB=acme_proj_1147"),
            ("{{unknown.key}}", "{{unknown.key}}"),
            ("no tokens", "no tokens"),
            ("{{shared.postgres}}", "user:pw@localhost:5432"),
            ("{{branch}} {{unterminated", "proj-1147 {{unterminated"),
            ("{{branch|nofilter}}", "proj-1147"),
        ];
        for (input, want) in cases {
            assert_eq!(resolve(input, lookup, filter), want, "{input}");
        }
    }

    #[test]
    fn refs_are_distinct_in_order() {
        assert_eq!(
            refs("{{branch|safe}} and {{service.api.server}} and {{branch}} {{ }}"),
            ["branch", "service.api.server"]
        );
    }
}
