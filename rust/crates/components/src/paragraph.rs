use ui::{div, label, theme, LabelSize, Node, Rgba};

use crate::text_line;

/// Body copy broken into lines at word boundaries to fit a width. Built with [`paragraph`].
pub struct Paragraph {
    text: String,
    width: f32,
    size: LabelSize,
    line_height: f32,
    color: Option<Rgba>,
    centered: bool,
}

pub fn paragraph(text: impl Into<String>, width: f32) -> Paragraph {
    Paragraph {
        text: text.into(),
        width,
        size: LabelSize::Small,
        line_height: 16.0,
        color: None,
        centered: false,
    }
}

impl Paragraph {
    pub fn size(mut self, size: LabelSize, line_height: f32) -> Self {
        self.size = size;
        self.line_height = line_height;
        self
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }

    /// The lines the text breaks into at this width.
    pub fn lines(&self) -> Vec<String> {
        let fits = |candidate: &str| {
            ui::measure_text_width(candidate, self.size.px(), false, ui::ui_font_weight())
                / ui::ui_text_scale()
                <= self.width
        };
        let mut lines: Vec<String> = Vec::new();
        for word in self.text.split_whitespace() {
            match lines.last_mut() {
                Some(line) if fits(&format!("{line} {word}")) => {
                    line.push(' ');
                    line.push_str(word);
                }
                _ => lines.push(word.to_string()),
            }
        }
        lines
    }
}

impl From<Paragraph> for Node {
    fn from(paragraph: Paragraph) -> Node {
        let color = paragraph.color.unwrap_or(theme().text_muted);
        let mut column = div().col();
        if paragraph.centered {
            column = column.w_px(paragraph.width);
        }
        for line in paragraph.lines() {
            let text = label(line).size(paragraph.size).color(color);
            column = column.child(if paragraph.centered {
                div()
                    .row()
                    .h_px(paragraph.line_height)
                    .items_center()
                    .justify_center()
                    .child(text)
                    .into()
            } else {
                text_line(paragraph.line_height, text)
            });
        }
        column.into()
    }
}
