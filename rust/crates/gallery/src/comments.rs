//! Review comments pinned to an element of a story, kept across runs so an agent can be handed the open ones.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ui::inspect::{children, Inspect, Inspected};

#[derive(Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub story: String,
    /// Child positions from the story's root down to the element, so it is found again after a restart.
    pub path: Vec<usize>,
    /// Where the element sat on its board, in board px.
    pub rect: [f32; 4],
    pub element: String,
    pub style: Vec<String>,
    pub text: String,
    #[serde(default)]
    pub resolved: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Comments {
    #[serde(default)]
    pub comments: Vec<Comment>,
}

fn store_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/state/pom/gallery-comments.json"))
}

impl Comments {
    pub fn load() -> Self {
        store_path()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = store_path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(dir) {
                eprintln!("gallery: {}: {error}", dir.display());
                return;
            }
        }
        match serde_json::to_vec_pretty(self) {
            Ok(bytes) => {
                if let Err(error) = std::fs::write(&path, bytes) {
                    eprintln!("gallery: {}: {error}", path.display());
                }
            }
            Err(error) => eprintln!("gallery: {error}"),
        }
    }

    pub fn open(&self) -> impl Iterator<Item = &Comment> {
        self.comments.iter().filter(|comment| !comment.resolved)
    }

    pub fn add(
        &mut self,
        story: &str,
        elements: &[Inspected],
        index: usize,
        origin: (f32, f32),
        text: String,
    ) -> Option<u64> {
        let element = elements.get(index)?;
        let id = self
            .comments
            .iter()
            .map(|comment| comment.id)
            .max()
            .unwrap_or(0)
            + 1;
        let rect = element.rect;
        self.comments.push(Comment {
            id,
            story: story.to_string(),
            path: path_of(elements, index),
            rect: [rect.x - origin.0, rect.y - origin.1, rect.w, rect.h],
            element: describe(element),
            style: style(element),
            text,
            resolved: false,
        });
        Some(id)
    }

    pub fn resolve(&mut self, id: u64) {
        if let Some(comment) = self.comments.iter_mut().find(|comment| comment.id == id) {
            comment.resolved = true;
        }
    }
}

pub fn path_of(elements: &[Inspected], mut index: usize) -> Vec<usize> {
    let mut path = Vec::new();
    while let Some(parent) = elements.get(index).and_then(|element| element.parent) {
        let position = children(elements, parent)
            .iter()
            .position(|child| *child == index)
            .unwrap_or(0);
        path.push(position);
        index = parent;
    }
    path.reverse();
    path
}

pub fn index_of(elements: &[Inspected], path: &[usize]) -> Option<usize> {
    let mut index = elements
        .iter()
        .position(|element| element.parent.is_none())?;
    for position in path {
        index = *children(elements, index).get(*position)?;
    }
    Some(index)
}

pub fn describe(element: &Inspected) -> String {
    match &element.what {
        Inspect::Box { row: true, .. } => "Row".to_string(),
        Inspect::Box { .. } => "Column".to_string(),
        Inspect::Text { text, .. } => format!("Label {text:?}"),
        Inspect::Icon { kind, .. } => format!("Icon {kind:?}"),
    }
}

fn color(color: ui::Rgba) -> String {
    let hex = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    let code = format!(
        "#{:02X}{:02X}{:02X}",
        hex(color.r),
        hex(color.g),
        hex(color.b)
    );
    match ui::theme_file::token_name(color) {
        Some(token) => format!("{token} ({code})"),
        None => code,
    }
}

fn style(element: &Inspected) -> Vec<String> {
    let rect = element.rect;
    let mut lines = vec![format!("size {} x {}", rect.w.round(), rect.h.round())];
    match &element.what {
        Inspect::Box {
            padding,
            gap,
            background,
            border,
            border_color,
            radius,
            ..
        } => {
            let [top, right, bottom, left] = padding;
            lines.push(format!("padding {top} {right} {bottom} {left}"));
            lines.push(format!("gap {gap}"));
            if let Some(background) = background {
                lines.push(format!("background {}", color(*background)));
            }
            if *radius > 0.0 {
                lines.push(format!("radius {radius}"));
            }
            if *border > 0.0 {
                lines.push(format!("border {border} {}", color(*border_color)));
            }
        }
        Inspect::Text {
            size,
            weight,
            color: text_color,
            font,
            ..
        } => {
            lines.push(format!("font {font:?} {size}px weight {weight}"));
            lines.push(format!("color {}", color(*text_color)));
        }
        Inspect::Icon {
            size,
            color: icon_color,
            ..
        } => {
            lines.push(format!("icon size {size}"));
            lines.push(format!("color {}", color(*icon_color)));
        }
    }
    lines
}

/// The open comments as one message an agent can act on: where each story lives, the element, and the ask.
pub fn for_agent(comments: &Comments, defs: &[stories::StoryDef], theme: &str) -> String {
    let open: Vec<&Comment> = comments.open().collect();
    let mut text = format!(
        "Review comments from the Pomelo gallery ({} open, theme {theme}).\n\
         Each names a story in the stories crate (by id), the element inside it, its style when the comment \
         was made, and what to change. Make each change in the component the story renders, then check it in \
         the gallery.\n",
        open.len()
    );
    for comment in open {
        let place = defs
            .iter()
            .find(|def| def.id == comment.story)
            .map_or(String::new(), |def| {
                format!("{} > {} > {}", def.page, def.group, def.title)
            });
        let path: Vec<String> = comment.path.iter().map(usize::to_string).collect();
        let [x, y, ..] = comment.rect;
        text.push_str(&format!(
            "\n#{} {place} (story {})\nElement: {} at {}, {} (path {})\nStyle: {}\nComment: {}\n",
            comment.id,
            comment.story,
            comment.element,
            x.round(),
            y.round(),
            if path.is_empty() {
                "root".to_string()
            } else {
                path.join("/")
            },
            comment.style.join("; "),
            comment.text.trim()
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui::{div, icon, label, IconKind, Rect, Rgba};

    fn elements() -> Vec<Inspected> {
        let node: ui::Node = div()
            .col()
            .child(label("Title"))
            .child(
                div()
                    .row()
                    .child(icon(IconKind::Check))
                    .child(label("Done")),
            )
            .into();
        let area = Rect::new(0.0, 0.0, 200.0, 100.0, Rgba::TRANSPARENT);
        ui::inspect::inspect(|| ui::render(&node, area)).1
    }

    #[test]
    fn a_path_finds_the_same_element_again() {
        let elements = elements();
        for index in 0..elements.len() {
            assert_eq!(index_of(&elements, &path_of(&elements, index)), Some(index));
        }
        let done = elements
            .iter()
            .position(
                |element| matches!(&element.what, Inspect::Text { text, .. } if text == "Done"),
            )
            .unwrap_or_default();
        assert_eq!(path_of(&elements, done), [1, 1]);
        assert_eq!(index_of(&elements, &[4]), None);
    }

    #[test]
    fn the_agent_gets_only_open_comments_with_their_element() {
        let elements = elements();
        let mut comments = Comments::default();
        let first = comments.add("story-a", &elements, 1, (0.0, 0.0), "Bolder".into());
        comments.add("story-a", &elements, 0, (0.0, 0.0), "Less padding".into());
        if let Some(first) = first {
            comments.resolve(first);
        }
        let text = for_agent(&comments, &[], "One Dark");
        assert!(text.contains("1 open"), "{text}");
        assert!(text.contains("#2"), "{text}");
        assert!(text.contains("Comment: Less padding"), "{text}");
        assert!(
            text.contains("Element: Column at 0, 0 (path root)"),
            "{text}"
        );
        assert!(!text.contains("Bolder"), "{text}");
    }
}
