use indexmap::IndexMap;

use crate::yaml_node::{Node, NodeKind};

/// Collects every type mismatch instead of stopping at the first, so one load reports all
/// broken fields at once.
#[derive(Default)]
pub struct Decoder {
    pub errors: Vec<String>,
}

impl Decoder {
    pub fn mismatch(&mut self, node: &Node, target: &str) {
        let shown = match &node.kind {
            NodeKind::Scalar { value, .. } => format!("{} `{value}`", node.tag_name()),
            _ => node.tag_name().to_string(),
        };
        self.errors.push(format!(
            "line {}: cannot unmarshal {shown} into {target}",
            node.line
        ));
    }

    pub fn string(&mut self, node: Option<&Node>) -> String {
        let Some(node) = node else {
            return String::new();
        };
        match node.scalar_value() {
            Some(value) => value.to_string(),
            None => {
                self.mismatch(node, "string");
                String::new()
            }
        }
    }

    pub fn bool(&mut self, node: Option<&Node>) -> bool {
        self.opt_bool(node).unwrap_or(false)
    }

    pub fn opt_bool(&mut self, node: Option<&Node>) -> Option<bool> {
        let node = node.filter(|node| !node.is_null())?;
        match node.scalar_value().map(str::to_ascii_lowercase).as_deref() {
            Some("true" | "yes" | "on" | "y") => Some(true),
            Some("false" | "no" | "off" | "n") => Some(false),
            _ => {
                self.mismatch(node, "bool");
                None
            }
        }
    }

    pub fn int(&mut self, node: Option<&Node>) -> i64 {
        let Some(node) = node.filter(|node| !node.is_null()) else {
            return 0;
        };
        match node.scalar_value().and_then(|value| value.parse().ok()) {
            Some(value) => value,
            None => {
                self.mismatch(node, "int");
                0
            }
        }
    }

    pub fn opt_u16(&mut self, node: Option<&Node>) -> Option<u16> {
        let node = node.filter(|node| !node.is_null())?;
        match node.scalar_value().and_then(|value| value.parse().ok()) {
            Some(value) => Some(value),
            None => {
                self.mismatch(node, "uint16");
                None
            }
        }
    }

    pub fn strings(&mut self, node: Option<&Node>) -> Vec<String> {
        let Some(node) = node.filter(|node| !node.is_null()) else {
            return Vec::new();
        };
        let Some(items) = node.items() else {
            self.mismatch(node, "[]string");
            return Vec::new();
        };
        items.iter().map(|item| self.string(Some(item))).collect()
    }

    /// A single scalar or a sequence; an empty scalar is no entries.
    pub fn string_list(&mut self, node: Option<&Node>) -> Vec<String> {
        let Some(node) = node.filter(|node| !node.is_null()) else {
            return Vec::new();
        };
        match node.scalar_value() {
            Some("") => Vec::new(),
            Some(value) => vec![value.to_string()],
            None => self.strings(Some(node)),
        }
    }

    pub fn string_map(&mut self, node: Option<&Node>) -> IndexMap<String, String> {
        self.map(node, "map[string]string", |decoder, value| {
            decoder.string(Some(value))
        })
    }

    pub fn map<T>(
        &mut self,
        node: Option<&Node>,
        target: &str,
        mut decode: impl FnMut(&mut Self, &Node) -> T,
    ) -> IndexMap<String, T> {
        let mut out = IndexMap::new();
        let Some(node) = node.filter(|node| !node.is_null()) else {
            return out;
        };
        let Some(entries) = node.entries() else {
            self.mismatch(node, target);
            return out;
        };
        for (key, value) in entries {
            let decoded = decode(self, value);
            out.insert(key.text().to_string(), decoded);
        }
        out
    }

    /// The mapping behind a struct-typed field; `None` for null, after recording a mismatch
    /// for anything else that isn't a mapping.
    pub fn fields<'a>(&mut self, node: &'a Node, target: &str) -> Option<&'a Node> {
        if node.is_null() {
            return None;
        }
        if node.is_mapping() {
            return Some(node);
        }
        self.mismatch(node, target);
        None
    }
}
