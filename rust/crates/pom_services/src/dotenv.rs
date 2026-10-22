//! Reads a `.env` file the way the dotenv loaders of Ruby and Node do, so the values the test profile
//! exports are the ones the test framework would have loaded itself.

/// `KEY=value` pairs in file order. `$VAR` and `${VAR}` (and `${VAR:-default}`) expand from earlier keys
/// of the same file, then from `outer`; single-quoted values stay literal.
pub fn parse(text: &str, outer: &dyn Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (line, after) = split_line(rest);
        rest = after;
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        {
            continue;
        }
        let value = value.trim_start();
        let lookup = |name: &str| {
            pairs
                .iter()
                .rev()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .or_else(|| outer(name))
        };
        let parsed = match value.chars().next() {
            Some(quote @ ('"' | '\'' | '`')) => {
                let (body, consumed) = quoted(&value[1..], rest, quote);
                rest = consumed;
                if quote == '\'' {
                    body
                } else {
                    expand(&unescape(&body), &lookup)
                }
            }
            _ => {
                let bare = match value.find(" #") {
                    Some(comment) => &value[..comment],
                    None => value,
                };
                expand(bare.trim_end(), &lookup)
            }
        };
        pairs.push((key.to_string(), parsed));
    }
    pairs
}

fn split_line(text: &str) -> (&str, &str) {
    match text.find('\n') {
        Some(end) => (text[..end].trim_end_matches('\r'), &text[end + 1..]),
        None => (text, ""),
    }
}

/// The value up to its closing quote, which may sit on a later line; returns the text left after that line.
fn quoted<'a>(first: &'a str, mut rest: &'a str, quote: char) -> (String, &'a str) {
    let mut body = String::new();
    let mut line = first;
    loop {
        let mut escaped = false;
        for (index, c) in line.char_indices() {
            if c == quote && !escaped {
                body.push_str(&line[..index]);
                return (body, rest);
            }
            escaped = c == '\\' && !escaped && quote != '\'';
        }
        body.push_str(line);
        if rest.is_empty() {
            return (body, rest);
        }
        body.push('\n');
        let (next, after) = split_line(rest);
        line = next;
        rest = after;
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            // Kept escaped so `expand` leaves the dollar literal.
            Some('$') => out.push_str("\\$"),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn expand(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < text.len() {
        let c = bytes[index];
        if c == b'\\' && bytes.get(index + 1) == Some(&b'$') {
            out.push('$');
            index += 2;
            continue;
        }
        if c != b'$' {
            let next = text[index..]
                .char_indices()
                .nth(1)
                .map_or(text.len(), |(offset, _)| index + offset);
            out.push_str(&text[index..next]);
            index = next;
            continue;
        }
        if bytes.get(index + 1) == Some(&b'{') {
            if let Some(close) = text[index..].find('}') {
                let inner = &text[index + 2..index + close];
                let (name, default) = match inner.split_once(":-") {
                    Some((name, default)) => (name, Some(default)),
                    None => (inner, None),
                };
                let value = lookup(name).filter(|value| !value.is_empty() || default.is_none());
                out.push_str(&value.unwrap_or_else(|| default.unwrap_or_default().to_string()));
                index += close + 1;
                continue;
            }
        }
        let name_len = text[index + 1..]
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        if name_len == 0 {
            out.push('$');
            index += 1;
            continue;
        }
        out.push_str(&lookup(&text[index + 1..index + 1 + name_len]).unwrap_or_default());
        index += 1 + name_len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Vec<(String, String)> {
        parse(text, &|name| {
            (name == "HOME").then(|| "/home/me".to_string())
        })
    }

    fn pair(key: &str, value: &str) -> (String, String) {
        (key.to_string(), value.to_string())
    }

    #[test]
    fn values_quotes_comments_and_expansion() {
        let text = "# comment\n\nexport A=1\nB = two words # trailing\nC=\"x\\ny $A\"\nD='lit $A'\nE=${A}-$HOME-${MISSING:-dflt}\nF=\"line1\nline2\"\nG=a#b\nH=\\$A\nnot a pair\nI=\"esc \\$A\"\r\n";
        assert_eq!(
            read(text),
            vec![
                pair("A", "1"),
                pair("B", "two words"),
                pair("C", "x\ny 1"),
                pair("D", "lit $A"),
                pair("E", "1-/home/me-dflt"),
                pair("F", "line1\nline2"),
                pair("G", "a#b"),
                pair("H", "$A"),
                pair("I", "esc $A"),
            ]
        );
    }

    #[test]
    fn a_later_key_wins_in_expansion() {
        assert_eq!(read("A=1\nA=2\nB=$A")[2], pair("B", "2"));
        assert_eq!(read("A=\"unterminated"), vec![pair("A", "unterminated")]);
        assert_eq!(read("A=$"), vec![pair("A", "$")]);
    }
}
