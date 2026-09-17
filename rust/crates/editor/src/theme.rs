//! User-configurable theme. Loaded from JSON (hex colors, like Zed's theme files); falls back to a built-in One Dark.
//! The renderer reads the base colors from here and maps tree-sitter capture names to `syntax` colors.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An 8-bit sRGB color, (de)serialized as `#rrggbb`.
#[derive(Clone, Copy, Debug)]
pub struct Color(pub [u8; 3]);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color([r, g, b])
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("#{:02x}{:02x}{:02x}", self.0[0], self.0[1], self.0[2]))
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let h = s.trim_start_matches('#');
        if h.len() < 6 {
            return Err(serde::de::Error::custom("expected #rrggbb"));
        }
        let p = |i| u8::from_str_radix(&h[i..i + 2], 16).map_err(serde::de::Error::custom);
        Ok(Color([p(0)?, p(2)?, p(4)?]))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub gutter: Color,
    pub tab_bar: Color,
    pub tab_border: Color,
    pub caret: Color,
    pub selection: Color,
    #[serde(default = "default_selection_alpha")]
    pub selection_alpha: f32,
    pub syntax: HashMap<String, Color>,
}

fn default_selection_alpha() -> f32 {
    0.24
}

impl Theme {
    /// Load from a JSON file; on any error, fall back to the built-in One Dark.
    pub fn load_or_default(path: &std::path::Path) -> Theme {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(Theme::one_dark)
    }

    /// Color for a tree-sitter capture name, resolved by longest dotted prefix (like Zed's HighlightMap).
    pub fn syntax_color(&self, name: &str) -> Color {
        let mut n = name;
        loop {
            if let Some(c) = self.syntax.get(n) {
                return *c;
            }
            match n.rfind('.') {
                Some(i) => n = &n[..i],
                None => return self.foreground,
            }
        }
    }

    /// Built-in Zed One Dark.
    pub fn one_dark() -> Theme {
        let mut syntax = HashMap::new();
        let mut put = |k: &str, c: Color| {
            syntax.insert(k.to_string(), c);
        };
        put("keyword", Color::rgb(180, 119, 207));
        put("preproc", Color::rgb(180, 119, 207));
        put("function", Color::rgb(115, 173, 233));
        put("constructor", Color::rgb(115, 173, 233));
        put("type", Color::rgb(110, 180, 191));
        put("operator", Color::rgb(110, 180, 191));
        put("enum", Color::rgb(110, 180, 191));
        put("string", Color::rgb(161, 193, 129));
        put("comment", Color::rgb(93, 99, 111));
        put("number", Color::rgb(191, 149, 106));
        put("boolean", Color::rgb(191, 149, 106));
        put("constant", Color::rgb(223, 193, 132));
        put("property", Color::rgb(208, 114, 119));
        put("title", Color::rgb(208, 114, 119));
        put("tag", Color::rgb(116, 173, 232));
        put("attribute", Color::rgb(116, 173, 232));
        put("label", Color::rgb(116, 173, 232));
        put("variable", Color::rgb(172, 178, 190));
        put("punctuation", Color::rgb(172, 178, 190));
        put("punctuation.bracket", Color::rgb(178, 185, 198));
        put("punctuation.delimiter", Color::rgb(178, 185, 198));
        put("variable.special", Color::rgb(191, 149, 106));
        put("string.special.symbol", Color::rgb(191, 149, 106));
        Theme {
            background: Color::rgb(40, 44, 51),
            foreground: Color::rgb(172, 178, 190),
            gutter: Color::rgb(78, 90, 95),
            tab_bar: Color::rgb(47, 52, 62),
            tab_border: Color::rgb(70, 75, 87),
            caret: Color::rgb(116, 173, 232),
            selection: Color::rgb(116, 173, 232),
            selection_alpha: 0.24,
            syntax,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::one_dark()
    }
}
