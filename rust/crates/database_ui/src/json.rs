//! JSON read in the order it was written, for the value panel's tree and the text an editor tab opens (the
//! workspace's JSON maps sort their keys, which would reorder what the user sees and saves).

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    /// Kept as written, so large or precise numbers survive untouched.
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

pub(crate) fn parse(text: &str) -> Option<Json> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        text,
        at: 0,
    };
    let value = parser.value()?;
    parser.space();
    (parser.at == parser.bytes.len()).then_some(value)
}

/// `{ 9 keys }` / `[ 3 items ]`, what a folded object or array reads as.
pub(crate) fn summary(value: &Json) -> Option<String> {
    let plural =
        |count: usize, word: &str| format!("{count} {word}{}", if count == 1 { "" } else { "s" });
    match value {
        Json::Object(entries) => Some(format!("{{ {} }}", plural(entries.len(), "key"))),
        Json::Array(items) => Some(format!("[ {} ]", plural(items.len(), "item"))),
        _ => None,
    }
}

/// Two-space indented text in the original key order.
pub(crate) fn pretty(value: &Json) -> String {
    let mut out = String::new();
    write(value, 0, &mut out);
    out
}

fn write(value: &Json, depth: usize, out: &mut String) {
    let indent = |depth: usize| "  ".repeat(depth);
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Json::Number(number) => out.push_str(number),
        Json::String(text) => out.push_str(&quote(text)),
        Json::Array(items) if items.is_empty() => out.push_str("[]"),
        Json::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Json::Array(items) => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                out.push_str(&indent(depth + 1));
                write(item, depth + 1, out);
                out.push_str(if index + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&indent(depth));
            out.push(']');
        }
        Json::Object(entries) => {
            out.push_str("{\n");
            for (index, (key, item)) in entries.iter().enumerate() {
                out.push_str(&indent(depth + 1));
                out.push_str(&quote(key));
                out.push_str(": ");
                write(item, depth + 1, out);
                out.push_str(if index + 1 < entries.len() {
                    ",\n"
                } else {
                    "\n"
                });
            }
            out.push_str(&indent(depth));
            out.push('}');
        }
    }
}

pub(crate) fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            self.at += 1;
        }
    }

    fn eat(&mut self, literal: &str) -> bool {
        if self.text[self.at..].starts_with(literal) {
            self.at += literal.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Option<Json> {
        self.space();
        match *self.bytes.get(self.at)? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string().map(Json::String),
            b't' if self.eat("true") => Some(Json::Bool(true)),
            b'f' if self.eat("false") => Some(Json::Bool(false)),
            b'n' if self.eat("null") => Some(Json::Null),
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.at += 1;
        }
        let number = &self.text[start..self.at];
        number
            .parse::<f64>()
            .ok()
            .map(|_| Json::Number(number.to_string()))
    }

    fn string(&mut self) -> Option<String> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let rest = &self.text[self.at..];
            let mut chars = rest.chars();
            let c = chars.next()?;
            self.at += c.len_utf8();
            match c {
                '"' => return Some(out),
                '\\' => {
                    let escape = self.text[self.at..].chars().next()?;
                    self.at += escape.len_utf8();
                    match escape {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'u' => {
                            let hex = self.text.get(self.at..self.at + 4)?;
                            self.at += 4;
                            let code = u32::from_str_radix(hex, 16).ok()?;
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        other => out.push(other),
                    }
                }
                c => out.push(c),
            }
        }
    }

    fn array(&mut self) -> Option<Json> {
        self.at += 1;
        let mut items = Vec::new();
        self.space();
        if self.eat("]") {
            return Some(Json::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.space();
            if self.eat(",") {
                continue;
            }
            return self.eat("]").then_some(Json::Array(items));
        }
    }

    fn object(&mut self) -> Option<Json> {
        self.at += 1;
        let mut entries = Vec::new();
        self.space();
        if self.eat("}") {
            return Some(Json::Object(entries));
        }
        loop {
            self.space();
            if self.bytes.get(self.at) != Some(&b'"') {
                return None;
            }
            let key = self.string()?;
            self.space();
            if !self.eat(":") {
                return None;
            }
            entries.push((key, self.value()?));
            self.space();
            if self.eat(",") {
                continue;
            }
            return self.eat("}").then_some(Json::Object(entries));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_keep_their_written_order_and_pretty_round_trips() {
        let value = parse(r#"{"z":1,"a":[true,null,"x\"y"],"m":{},"n":-1.5e3}"#).expect("json");
        assert_eq!(summary(&value).as_deref(), Some("{ 4 keys }"));
        let text = pretty(&value);
        assert_eq!(
            text,
            "{\n  \"z\": 1,\n  \"a\": [\n    true,\n    null,\n    \"x\\\"y\"\n  ],\n  \"m\": {},\n  \"n\": -1.5e3\n}"
        );
        assert_eq!(parse(&text), Some(value));
    }

    #[test]
    fn broken_text_is_not_json() {
        assert_eq!(parse("{\"a\": }"), None);
        assert_eq!(parse("[1, 2"), None);
        assert_eq!(parse("12 13"), None);
        assert_eq!(parse("\"\\u00e9\""), Some(Json::String("\u{e9}".into())));
    }
}
