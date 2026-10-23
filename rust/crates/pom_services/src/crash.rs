//! What a crashed holder left, read the same way wherever it is shown: how it exited and the line of its
//! output that most likely says why.

/// How it exited ("exit 1", "signal: 9"), from the header its holder recorded.
pub fn crash_exit(header: &str) -> String {
    let detail = header
        .split_once(" - ")
        .map_or(header, |(_, detail)| detail)
        .trim();
    match detail.rsplit_once("exit status: ") {
        Some((_, code)) => format!("exit {}", code.trim()),
        None => detail.trim_start_matches("exited: ").to_string(),
    }
}

/// The line of a crash's output that most likely says why it died.
pub fn crash_line(output: &[u8]) -> Option<String> {
    telling_line(&strip_terminal_codes(&String::from_utf8_lossy(output)))
}

/// The last output line that names an error, else the last line with anything on it.
pub fn telling_line(output: &str) -> Option<String> {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let named = lines.iter().rev().find(|line| {
        let lower = line.to_ascii_lowercase();
        ["error", "exception", "panic", "fatal", "failed"]
            .iter()
            .any(|word| lower.contains(word))
    });
    named.or(lines.last()).map(|line| line.to_string())
}

/// Terminal output without its escape sequences (colors, cursor moves, titles).
pub fn strip_terminal_codes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if c != '\r' {
                out.push(c);
            }
            continue;
        }
        match chars.next() {
            Some('[') => {
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}
