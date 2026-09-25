use std::collections::HashMap;

use saphyr_parser::{Event, Parser, ScalarStyle, Span};

#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    Scalar { value: String, plain: bool },
    Sequence(Vec<Node>),
    Mapping(Vec<(Node, Node)>),
}

/// An untyped YAML value that keeps raw scalar text, key order and source line, so config
/// fragments can be merged before any typing happens.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    pub line: usize,
}

impl Node {
    pub fn scalar(value: impl Into<String>) -> Self {
        Node {
            kind: NodeKind::Scalar {
                value: value.into(),
                plain: false,
            },
            line: 0,
        }
    }

    pub fn empty_mapping() -> Self {
        Node {
            kind: NodeKind::Mapping(Vec::new()),
            line: 0,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(&self.kind, NodeKind::Scalar { value, plain: true }
            if matches!(value.as_str(), "" | "~" | "null" | "Null" | "NULL"))
    }

    pub fn scalar_value(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Scalar { value, .. } if !self.is_null() => Some(value),
            NodeKind::Scalar { .. } => Some(""),
            _ => None,
        }
    }

    /// Scalar text for use as a map key or loose value; non-scalars read as empty, the way a
    /// raw node's `Value` does.
    pub fn text(&self) -> &str {
        self.scalar_value().unwrap_or("")
    }

    /// The scalar exactly as written (`~` stays `~`), for fields read straight off the tree.
    pub fn raw_text(&self) -> &str {
        match &self.kind {
            NodeKind::Scalar { value, .. } => value,
            _ => "",
        }
    }

    pub fn entries(&self) -> Option<&[(Node, Node)]> {
        match &self.kind {
            NodeKind::Mapping(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn items(&self) -> Option<&[Node]> {
        match &self.kind {
            NodeKind::Sequence(items) => Some(items),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.entries()?
            .iter()
            .find(|(k, _)| k.text() == key)
            .map(|(_, v)| v)
    }

    pub fn is_mapping(&self) -> bool {
        matches!(self.kind, NodeKind::Mapping(_))
    }

    #[cfg(test)]
    pub fn without_lines(&self) -> Node {
        let kind = match &self.kind {
            NodeKind::Scalar { .. } => self.kind.clone(),
            NodeKind::Sequence(items) => {
                NodeKind::Sequence(items.iter().map(Node::without_lines).collect())
            }
            NodeKind::Mapping(entries) => NodeKind::Mapping(
                entries
                    .iter()
                    .map(|(k, v)| (k.without_lines(), v.without_lines()))
                    .collect(),
            ),
        };
        Node { kind, line: 0 }
    }

    pub fn tag_name(&self) -> &'static str {
        match &self.kind {
            NodeKind::Mapping(_) => "!!map",
            NodeKind::Sequence(_) => "!!seq",
            NodeKind::Scalar { .. } if self.is_null() => "!!null",
            NodeKind::Scalar { .. } => "!!str",
        }
    }
}

/// Block-style YAML for `node` that parses back to the same tree (comments and original layout are gone).
pub fn to_yaml(node: &Node) -> String {
    let mut text = match &node.kind {
        NodeKind::Mapping(entries) if entries.is_empty() => "{}".to_string(),
        NodeKind::Sequence(items) if items.is_empty() => "[]".to_string(),
        _ => block_lines(node).join("\n"),
    };
    text.push('\n');
    text
}

enum Written {
    Inline(String),
    Literal(Vec<String>),
    Nested(Vec<String>),
}

fn written(node: &Node) -> Written {
    match &node.kind {
        NodeKind::Scalar { value, plain } => scalar_written(value, *plain),
        NodeKind::Mapping(entries) if entries.is_empty() => Written::Inline("{}".into()),
        NodeKind::Sequence(items) if items.is_empty() => Written::Inline("[]".into()),
        _ => Written::Nested(block_lines(node)),
    }
}

fn block_lines(node: &Node) -> Vec<String> {
    let mut lines = Vec::new();
    match &node.kind {
        NodeKind::Mapping(entries) => {
            for (key, value) in entries {
                let key = match written(key) {
                    Written::Inline(text) => text,
                    _ => quoted(key.text()),
                };
                match written(value) {
                    Written::Inline(text) if text.is_empty() => lines.push(format!("{key}:")),
                    Written::Inline(text) => lines.push(format!("{key}: {text}")),
                    Written::Literal(block) => {
                        lines.push(format!("{key}: {}", block[0]));
                        lines.extend(block[1..].iter().map(|line| indented(line)));
                    }
                    Written::Nested(block) => {
                        lines.push(format!("{key}:"));
                        lines.extend(block.iter().map(|line| indented(line)));
                    }
                }
            }
        }
        NodeKind::Sequence(items) => {
            for item in items {
                match written(item) {
                    Written::Inline(text) if text.is_empty() => lines.push("-".into()),
                    Written::Inline(text) => lines.push(format!("- {text}")),
                    Written::Literal(block) | Written::Nested(block) => {
                        lines.push(format!("- {}", block[0]));
                        lines.extend(block[1..].iter().map(|line| indented(line)));
                    }
                }
            }
        }
        NodeKind::Scalar { value, plain } => match scalar_written(value, *plain) {
            Written::Inline(text) => lines.push(text),
            Written::Literal(block) | Written::Nested(block) => lines.extend(block),
        },
    }
    lines
}

fn indented(line: &str) -> String {
    if line.is_empty() {
        String::new()
    } else {
        format!("  {line}")
    }
}

fn scalar_written(value: &str, plain: bool) -> Written {
    // A plain scalar was written bare in the source, so writing it bare again reads the same.
    if plain && !value.contains('\n') {
        return Written::Inline(value.to_string());
    }
    let literal_safe = value.contains('\n')
        && !value.starts_with([' ', '\t', '\n'])
        && value
            .chars()
            .all(|c| c == '\n' || c == '\t' || !c.is_control());
    if !literal_safe {
        return Written::Inline(quoted(value));
    }
    let body = value.trim_end_matches('\n');
    let trailing = value.len() - body.len();
    let header = match trailing {
        0 => "|-",
        1 => "|",
        _ => "|+",
    };
    let mut block = vec![header.to_string()];
    block.extend(body.split('\n').map(str::to_string));
    block.extend(std::iter::repeat_n(
        String::new(),
        trailing.saturating_sub(1),
    ));
    Written::Literal(block)
}

fn quoted(value: &str) -> String {
    let mut text = String::with_capacity(value.len() + 2);
    text.push('"');
    for c in value.chars() {
        match c {
            '"' => text.push_str("\\\""),
            '\\' => text.push_str("\\\\"),
            '\n' => text.push_str("\\n"),
            '\t' => text.push_str("\\t"),
            '\r' => text.push_str("\\r"),
            c if c.is_control() => text.push_str(&format!("\\u{:04x}", c as u32)),
            c => text.push(c),
        }
    }
    text.push('"');
    text
}

#[derive(Debug, Clone, PartialEq)]
pub struct YamlError {
    pub message: String,
    pub line: usize,
}

impl std::fmt::Display for YamlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

/// Parses the first document; `None` means the stream held no document (empty file).
pub fn parse(source: &str) -> Result<Option<Node>, YamlError> {
    let mut builder = Builder::default();
    for event in Parser::new_from_str(source) {
        let (event, span) = event.map_err(|error| YamlError {
            message: error.info().to_string(),
            line: error.marker().line(),
        })?;
        if builder.feed(event, span)? {
            break;
        }
    }
    Ok(builder.document)
}

enum Frame {
    Sequence(usize, usize, Vec<Node>),
    Mapping(usize, usize, Vec<(Node, Node)>, Option<Node>),
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    anchors: HashMap<usize, Node>,
    document: Option<Node>,
}

impl Builder {
    fn feed(&mut self, event: Event<'_>, span: Span) -> Result<bool, YamlError> {
        let line = span.start.line();
        match event {
            Event::Scalar(value, style, anchor, _) => {
                // The parser reports an omitted value as a zero-width `~`; keep it empty like
                // the source text.
                let omitted = span.start.index() == span.end.index();
                let node = Node {
                    kind: NodeKind::Scalar {
                        value: if omitted {
                            String::new()
                        } else {
                            value.into_owned()
                        },
                        plain: style == ScalarStyle::Plain,
                    },
                    line,
                };
                return self.complete(node, anchor);
            }
            Event::Alias(anchor) => {
                let node = self.anchors.get(&anchor).cloned().ok_or(YamlError {
                    message: "unknown anchor".into(),
                    line,
                })?;
                return self.complete(node, 0);
            }
            Event::SequenceStart(anchor, _) => {
                self.stack.push(Frame::Sequence(anchor, line, Vec::new()))
            }
            Event::MappingStart(anchor, _) => {
                self.stack
                    .push(Frame::Mapping(anchor, line, Vec::new(), None))
            }
            Event::SequenceEnd => {
                if let Some(Frame::Sequence(anchor, line, items)) = self.stack.pop() {
                    let node = Node {
                        kind: NodeKind::Sequence(items),
                        line,
                    };
                    return self.complete(node, anchor);
                }
            }
            Event::MappingEnd => {
                if let Some(Frame::Mapping(anchor, line, entries, _)) = self.stack.pop() {
                    let node = Node {
                        kind: NodeKind::Mapping(expand_merge_keys(entries)?),
                        line,
                    };
                    return self.complete(node, anchor);
                }
            }
            Event::DocumentEnd => return Ok(self.document.is_some()),
            _ => {}
        }
        Ok(false)
    }

    fn complete(&mut self, node: Node, anchor: usize) -> Result<bool, YamlError> {
        if anchor != 0 {
            self.anchors.insert(anchor, node.clone());
        }
        match self.stack.last_mut() {
            None => self.document = Some(node),
            Some(Frame::Sequence(_, _, items)) => items.push(node),
            Some(Frame::Mapping(_, _, entries, pending_key)) => match pending_key.take() {
                None => *pending_key = Some(node),
                Some(key) => {
                    if key.text() != "<<" {
                        if let Some((existing, _)) =
                            entries.iter().find(|(k, _)| k.text() == key.text())
                        {
                            return Err(YamlError {
                                message: format!(
                                    "mapping key {:?} already defined at line {}",
                                    key.text(),
                                    existing.line
                                ),
                                line: key.line,
                            });
                        }
                    }
                    entries.push((key, node));
                }
            },
        }
        Ok(false)
    }
}

fn expand_merge_keys(entries: Vec<(Node, Node)>) -> Result<Vec<(Node, Node)>, YamlError> {
    if !entries.iter().any(|(key, _)| key.text() == "<<") {
        return Ok(entries);
    }
    let mut own = Vec::new();
    let mut inherited: Vec<(Node, Node)> = Vec::new();
    for (key, value) in entries {
        if key.text() != "<<" {
            own.push((key, value));
            continue;
        }
        let sources: Vec<Node> = match value.kind {
            NodeKind::Mapping(_) => vec![value],
            NodeKind::Sequence(items) => items,
            _ => {
                return Err(YamlError {
                    message: "map merge requires map or sequence of maps as the value".into(),
                    line: value.line,
                });
            }
        };
        for source in sources {
            let NodeKind::Mapping(source_entries) = source.kind else {
                return Err(YamlError {
                    message: "map merge requires map or sequence of maps as the value".into(),
                    line: source.line,
                });
            };
            for (key, value) in source_entries {
                if !inherited.iter().any(|(k, _)| k.text() == key.text()) {
                    inherited.push((key, value));
                }
            }
        }
    }
    inherited.retain(|(key, _)| !own.iter().any(|(k, _)| k.text() == key.text()));
    inherited.extend(own);
    Ok(inherited)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_yaml_reads_back_as_the_same_tree() {
        let source = "session: shop
empty:
nothing: {}
none: []
quoted: \"true\"
tricky: \"a: b # c\"
repos:
  api:
    alias: be
    services:
      server:
        cmd: |
          bundle exec rails s
          echo done
        tail: |-
          no newline
        keep: |+
          two

        env:
          - A=1
          - name: x
            value: \"y\\tz\"
          - - nested
            - list
";
        let original = doc(source);
        let written = to_yaml(&original);
        assert_eq!(
            doc(&written).without_lines(),
            original.without_lines(),
            "{written}"
        );
        assert!(
            written.contains("cmd: |\n          bundle exec rails s\n"),
            "{written}"
        );
        assert_eq!(to_yaml(&Node::empty_mapping()), "{}\n");
    }

    fn doc(source: &str) -> Node {
        match parse(source) {
            Ok(Some(node)) => node,
            other => panic!("parse failed: {other:?}"),
        }
    }

    #[test]
    fn keeps_order_raw_scalars_and_lines() {
        let root = doc("b: 1\na: '010'\nc:\n");
        let keys: Vec<&str> = root
            .entries()
            .unwrap_or_default()
            .iter()
            .map(|(k, _)| k.text())
            .collect();
        assert_eq!(keys, ["b", "a", "c"]);
        assert_eq!(root.get("a").map(Node::text), Some("010"));
        assert!(root.get("c").is_some_and(Node::is_null));
        assert_eq!(root.get("a").map(|n| n.line), Some(2));
    }

    #[test]
    fn quoted_null_is_a_string() {
        let root = doc("a: 'null'\nb: null\n");
        assert!(!root.get("a").is_some_and(Node::is_null));
        assert!(root.get("b").is_some_and(Node::is_null));
    }

    #[test]
    fn aliases_and_merge_keys_expand() {
        let root = doc("base: &b\n  x: 1\n  y: 2\nchild:\n  <<: *b\n  y: 3\n");
        let child = root
            .get("child")
            .cloned()
            .unwrap_or_else(Node::empty_mapping);
        assert_eq!(child.get("x").map(Node::text), Some("1"));
        assert_eq!(child.get("y").map(Node::text), Some("3"));
        assert_eq!(child.entries().map(<[_]>::len), Some(2));
    }

    #[test]
    fn duplicate_keys_are_rejected() {
        let error = parse("a: 1\na: 2\n").err();
        assert_eq!(error.map(|e| e.line), Some(2));
    }

    #[test]
    fn empty_stream_has_no_document() {
        assert_eq!(parse(""), Ok(None));
        assert_eq!(parse("# only a comment\n"), Ok(None));
    }

    #[test]
    fn syntax_errors_carry_a_line() {
        let error = parse("a: [1, 2\nb: 3\n").err();
        assert!(error.is_some_and(|e| e.line >= 1));
    }
}
