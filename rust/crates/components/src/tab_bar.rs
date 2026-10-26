use ui::{div, theme, Div, Node};

use crate::Tab;

pub const TAB_BAR_HEIGHT: f32 = 34.0;
/// Space between the bar's ends and its clusters, and between neighbours inside a cluster.
const BAR_PAD: f32 = 6.0;
const BAR_GAP: f32 = 2.0;
const BAR_BUTTON_WIDTH: f32 = 28.0;
const LEADING_DIVIDER: f32 = 1.0 + 4.0;

/// A pane's strip of tabs: buttons at the leading end set off by a divider, the tabs, buttons at the
/// trailing end. Built with [`tab_bar`].
pub struct TabBar {
    leading: Vec<Node>,
    strip: Option<Node>,
    trailing: Vec<Node>,
}

pub fn tab_bar() -> TabBar {
    TabBar {
        leading: Vec::new(),
        strip: None,
        trailing: Vec::new(),
    }
}

impl TabBar {
    /// A square button at the leading end (back, forward).
    pub fn leading(mut self, button: impl Into<Node>) -> Self {
        self.leading.push(button.into());
        self
    }

    /// The tabs, from [`tab_strip`]; an owner that scrolls them draws them over the bar's middle instead.
    pub fn strip(mut self, strip: impl Into<Node>) -> Self {
        self.strip = Some(strip.into());
        self
    }

    pub fn trailing(mut self, button: impl Into<Node>) -> Self {
        self.trailing.push(button.into());
        self
    }

    pub fn render(self) -> Div {
        let mut bar = div()
            .row()
            .h_px(TAB_BAR_HEIGHT)
            .items_center()
            .bg(theme().tab_bar_background);
        if self.leading.is_empty() {
            bar = bar.child(div().w_px(BAR_PAD));
        } else {
            bar = bar
                .child(cluster(self.leading))
                .child(crate::vertical_divider(TAB_BAR_HEIGHT))
                .child(div().w_px(LEADING_DIVIDER - 1.0));
        }
        let mut middle = div().row().flex(1.0).h_px(TAB_BAR_HEIGHT);
        if let Some(strip) = self.strip {
            middle = middle.child(strip);
        }
        bar = bar.child(middle);
        if self.trailing.is_empty() {
            bar.child(div().w_px(BAR_PAD))
        } else {
            bar.child(cluster(self.trailing))
        }
    }
}

impl From<TabBar> for Node {
    fn from(bar: TabBar) -> Node {
        bar.render().into()
    }
}

/// The tabs at their natural widths, side by side.
pub fn tab_strip(tabs: impl IntoIterator<Item = Tab>) -> Div {
    div()
        .row()
        .h_px(TAB_BAR_HEIGHT)
        .items_center()
        .gap(ui::Space::Px(BAR_GAP))
        .children(tabs.into_iter().map(Node::from))
}

/// Design-px widths of a bar's ends around its strip, for an owner that draws the strip itself.
pub fn tab_bar_edges(leading: usize, trailing: usize) -> (f32, f32) {
    (
        end_width(leading, LEADING_DIVIDER),
        end_width(trailing, 0.0),
    )
}

fn end_width(buttons: usize, divider: f32) -> f32 {
    if buttons == 0 {
        return BAR_PAD;
    }
    let count = buttons as f32;
    BAR_PAD * 2.0 + BAR_BUTTON_WIDTH * count + BAR_GAP * (count - 1.0) + divider
}

// The bar sizes its ends from BAR_BUTTON_WIDTH, so each square button sits in a slot that wide.
fn cluster(buttons: Vec<Node>) -> Div {
    div()
        .row()
        .items_center()
        .px(ui::Space::Px(BAR_PAD))
        .gap(ui::Space::Px(BAR_GAP))
        .children(buttons.into_iter().map(|button| {
            div()
                .row()
                .w_px(BAR_BUTTON_WIDTH)
                .h_px(24.0)
                .items_center()
                .justify_center()
                .child(button)
                .into()
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width_of(bar: TabBar) -> f32 {
        let node: Node = bar.render().into();
        let painted = ui::render(
            &node,
            ui::Rect::new(0.0, 0.0, 600.0, TAB_BAR_HEIGHT, ui::Rgba::TRANSPARENT),
        );
        painted
            .hits
            .iter()
            .find(|(_, id)| *id == 7)
            .map_or(0.0, |(rect, _)| rect.x + rect.w)
    }

    #[test]
    fn the_edges_match_where_the_buttons_end() {
        let button = || crate::icon_button(7, ui::IconKind::Plus);
        let trailing_start = 600.0 - tab_bar_edges(0, 2).1;
        let end = width_of(tab_bar().trailing(button()).trailing(button()));
        assert!(end <= 600.0 - BAR_PAD && end > trailing_start, "{end}");
        let (leading, _) = tab_bar_edges(2, 0);
        let end = width_of(tab_bar().leading(button()).leading(button()));
        assert!(end < leading, "{end} vs {leading}");
    }
}
