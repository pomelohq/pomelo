use std::collections::HashSet;

use pom_services::{ServiceExplain, ServiceTarget};
use terminal::Modifiers;
use ui::{div, label, theme, IconKind, Node, Rect, Rgba};
use workspace::{Item, ItemTick};

use crate::{hit_at, icon_button, EnvironmentContext};

pub const ITEM_ID: &str = "environment";
const ROW_H: f32 = 26.0;
const KEY_W: f32 = 210.0;
const BADGE_W: f32 = 88.0;
const REPO: u64 = 1;
const SERVICE: u64 = 2;
const BRANCH: u64 = 3;
const PROFILE: u64 = 4;
const ROW_BASE: u64 = 100;
const ROW_STRIDE: u64 = 2;
const REVEAL: u64 = 0;
const COPY: u64 = 1;

pub struct EnvItem {
    context: EnvironmentContext,
    repo: String,
    service: String,
    branch: String,
    /// Empty means the local profile.
    profile: String,
    explain: Option<ServiceExplain>,
    port: Option<u16>,
    error: Option<String>,
    revealed: HashSet<String>,
    copy: Option<String>,
    scroll: f32,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
}

impl EnvItem {
    pub fn new(context: EnvironmentContext) -> EnvItem {
        let branch = context.branch.clone();
        let mut item = EnvItem {
            context,
            repo: String::new(),
            service: String::new(),
            branch,
            profile: String::new(),
            explain: None,
            port: None,
            error: None,
            revealed: HashSet::new(),
            copy: None,
            scroll: 0.0,
            hits: Vec::new(),
            hovered: None,
        };
        item.pick_defaults();
        item.reload();
        item
    }

    fn repos(&self) -> Vec<(String, Vec<String>)> {
        (self.context.config)()
            .map(|config| {
                config
                    .repos
                    .iter()
                    .map(|(name, dir)| (name.clone(), dir.services.keys().cloned().collect()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn profiles(&self) -> Vec<String> {
        let mut profiles: Vec<String> = (self.context.config)()
            .map(|config| config.environments.keys().cloned().collect())
            .unwrap_or_default();
        profiles.sort();
        profiles.insert(0, String::new());
        profiles
    }

    fn pick_defaults(&mut self) {
        let repos = self.repos();
        if !repos.iter().any(|(name, _)| *name == self.repo) {
            self.repo = repos
                .first()
                .map(|(name, _)| name.clone())
                .unwrap_or_default();
        }
        let services = repos
            .iter()
            .find(|(name, _)| *name == self.repo)
            .map(|(_, services)| services.clone())
            .unwrap_or_default();
        if !services.contains(&self.service) {
            self.service = services.first().cloned().unwrap_or_default();
        }
    }

    pub fn reload(&mut self) {
        self.revealed.clear();
        self.scroll = 0.0;
        let Some(config) = (self.context.config)() else {
            self.explain = None;
            self.error = Some("pom.yml could not be loaded".into());
            return;
        };
        if self.repo.is_empty() || self.service.is_empty() {
            self.explain = None;
            self.error = None;
            return;
        }
        let env = self.context.runner.workspace_env(&config, &self.branch);
        self.explain = env.explain_service(&self.repo, &self.service, &self.profile);
        let is_main = self
            .context
            .workspaces
            .iter()
            .any(|(branch, main)| *branch == self.branch && *main);
        self.port = self.context.runner.port(
            &config,
            &ServiceTarget {
                branch: self.branch.clone(),
                is_main,
                repo: self.repo.clone(),
                service: self.service.clone(),
            },
        );
        self.error = self
            .explain
            .is_none()
            .then(|| format!("no service {} in {}", self.service, self.repo));
    }

    fn cycle(options: &[String], current: &str) -> String {
        let at = options.iter().position(|option| option == current);
        match at {
            Some(at) => options[(at + 1) % options.len()].clone(),
            None => options.first().cloned().unwrap_or_default(),
        }
    }

    fn click(&mut self, id: u64) {
        match id {
            REPO => {
                let names: Vec<String> = self.repos().into_iter().map(|(name, _)| name).collect();
                self.repo = Self::cycle(&names, &self.repo);
                self.service.clear();
                self.pick_defaults();
                self.reload();
            }
            SERVICE => {
                let services = self
                    .repos()
                    .into_iter()
                    .find(|(name, _)| *name == self.repo)
                    .map(|(_, services)| services)
                    .unwrap_or_default();
                self.service = Self::cycle(&services, &self.service);
                self.reload();
            }
            BRANCH => {
                let branches: Vec<String> = self
                    .context
                    .workspaces
                    .iter()
                    .map(|(branch, _)| branch.clone())
                    .collect();
                self.branch = Self::cycle(&branches, &self.branch);
                self.reload();
            }
            PROFILE => {
                self.profile = Self::cycle(&self.profiles(), &self.profile);
                self.reload();
            }
            id if id >= ROW_BASE => {
                let offset = id - ROW_BASE;
                let Some(line) = self
                    .explain
                    .as_ref()
                    .and_then(|explain| explain.env.get((offset / ROW_STRIDE) as usize))
                    .cloned()
                else {
                    return;
                };
                match offset % ROW_STRIDE {
                    REVEAL => {
                        if !self.revealed.remove(&line.key) {
                            self.revealed.insert(line.key);
                        }
                    }
                    _ => self.copy = Some(line.value),
                }
            }
            _ => {}
        }
    }

    fn picker(&self, id: u64, title: &str, value: &str) -> Node {
        let colors = theme();
        div()
            .col()
            .gap(3.0)
            .child(
                label(title.to_string())
                    .size(10.0)
                    .color(colors.text_placeholder),
            )
            .child(
                div()
                    .row()
                    .h_px(24.0)
                    .px(8.0)
                    .gap(6.0)
                    .items_center()
                    .rounded(4.0)
                    .border(1.0, colors.border_variant)
                    .bg(if self.hovered == Some(id) {
                        colors.ghost_element_hover
                    } else {
                        Rgba::TRANSPARENT
                    })
                    .on_click(id)
                    .child(
                        label(if value.is_empty() {
                            "-".to_string()
                        } else {
                            value.to_string()
                        })
                        .size(12.0)
                        .color(colors.text),
                    )
                    .child(
                        ui::icon(IconKind::ChevronUpDown)
                            .size(10.0)
                            .color(colors.icon_muted),
                    ),
            )
            .into()
    }

    fn badge(text: &str, color: Rgba) -> Node {
        div()
            .row()
            .w_px(BADGE_W)
            .justify_end()
            .child(
                div()
                    .row()
                    .h_px(18.0)
                    .px(6.0)
                    .items_center()
                    .rounded(9.0)
                    .bg(Rgba::new(color.r, color.g, color.b, 0.12))
                    .child(label(text.to_string()).size(10.0).color(color)),
            )
            .into()
    }

    fn line(&self, key: &str, value: &str, badge: Node, actions: Vec<Node>, masked: bool) -> Node {
        let colors = theme();
        let shown = if masked {
            "*".repeat(12)
        } else if value.is_empty() {
            "-".to_string()
        } else {
            value.to_string()
        };
        let mut row = div()
            .row()
            .h_px(ROW_H)
            .px(20.0)
            .gap(12.0)
            .items_center()
            .child(
                div().row().w_px(KEY_W).child(
                    label(key.to_string())
                        .size(11.5)
                        .mono()
                        .color(colors.text)
                        .truncate(),
                ),
            )
            .child(
                div().row().flex(1.0).items_center().child(
                    label(shown)
                        .size(11.5)
                        .mono()
                        .color(if masked {
                            colors.text_placeholder
                        } else {
                            colors.text_muted
                        })
                        .truncate(),
                ),
            );
        for action in actions {
            row = row.child(action);
        }
        row.child(badge).into()
    }

    fn section(title: String) -> Node {
        div()
            .row()
            .px(20.0)
            .pt(14.0)
            .pb(6.0)
            .child(label(title).size(10.0).color(theme().text_muted))
            .into()
    }

    /// The service the tab shows (tests).
    pub fn explained(&self) -> Option<&ServiceExplain> {
        self.explain.as_ref()
    }
}

impl Item for EnvItem {
    fn id(&self) -> Option<String> {
        Some(ITEM_ID.into())
    }

    fn title(&self) -> String {
        "Environment".into()
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Server)
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let colors = theme();
        let scale = ui::ui_text_scale();
        let (width, height) = (body.w / scale, body.h / scale);
        let pickers = div()
            .row()
            .gap(14.0)
            .px(20.0)
            .py(12.0)
            .child(self.picker(REPO, "REPO", &self.repo))
            .child(self.picker(SERVICE, "SERVICE", &self.service))
            .child(self.picker(BRANCH, "BRANCH", &self.branch))
            .child(self.picker(
                PROFILE,
                "PROFILE",
                if self.profile.is_empty() {
                    "local"
                } else {
                    &self.profile
                },
            ));
        let mut table = div().col();
        match (&self.explain, &self.error) {
            (Some(explain), _) => {
                if !explain.databases.is_empty() {
                    table = table.child(Self::section("DATABASES - per branch".into()));
                    for (key, value) in &explain.databases {
                        table = table.child(self.line(
                            key,
                            value,
                            Self::badge("db", colors.text_accent),
                            Vec::new(),
                            false,
                        ));
                    }
                }
                table = table.child(Self::section(match self.port {
                    Some(port) => format!("ENVIRONMENT - port {port}"),
                    None => "ENVIRONMENT".into(),
                }));
                if explain.env.is_empty() {
                    table = table.child(
                        div().row().px(20.0).py(8.0).child(
                            label("no env for this service")
                                .size(12.0)
                                .color(colors.text_muted),
                        ),
                    );
                }
                let first = (self.scroll / ROW_H) as usize;
                let visible = ((height - 150.0) / ROW_H).max(1.0) as usize;
                for (index, line) in explain.env.iter().enumerate().skip(first).take(visible) {
                    let base = ROW_BASE + index as u64 * ROW_STRIDE;
                    let masked =
                        line.secret && !line.value.is_empty() && !self.revealed.contains(&line.key);
                    let mut actions = Vec::new();
                    if line.secret && !line.value.is_empty() {
                        actions.push(icon_button(
                            base + REVEAL,
                            IconKind::Eye,
                            colors.icon_muted,
                            self.hovered == Some(base + REVEAL),
                        ));
                    } else {
                        actions.push(div().w_px(22.0).into());
                    }
                    if !line.value.is_empty() {
                        actions.push(icon_button(
                            base + COPY,
                            IconKind::Copy,
                            colors.icon_muted,
                            self.hovered == Some(base + COPY),
                        ));
                    }
                    let (text, color) = match line.source.as_str() {
                        "own" => ("own", colors.text_accent),
                        source if source.starts_with("preset:") => (&source[7..], colors.success),
                        _ => ("resolved", colors.text_placeholder),
                    };
                    table = table.child(self.line(
                        &line.key,
                        &line.value,
                        Self::badge(text, color),
                        actions,
                        masked,
                    ));
                }
            }
            (None, Some(error)) => {
                table = table.child(
                    div()
                        .row()
                        .p(16.0)
                        .child(label(error.clone()).size(11.0).mono().color(colors.error)),
                );
            }
            (None, None) => {
                table = table.child(
                    div().row().justify_center().pt(40.0).child(
                        label("Pick a repo + service to inspect its resolved env")
                            .size(12.0)
                            .color(colors.text_muted),
                    ),
                );
            }
        }
        let tree: Node = div()
            .col()
            .w_px(width)
            .h_px(height)
            .bg(colors.editor_background)
            .child(pickers)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(table)
            .into();
        let painted = ui::render(&tree, body);
        self.hits = painted.hits.clone();
        Some(painted)
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        if let Some(id) = hit_at(&self.hits, x, y) {
            self.click(id);
        }
        true
    }

    fn pointer_move(&mut self, x: f32, y: f32, _modifiers: Modifiers, _focused: bool) -> bool {
        let hit = hit_at(&self.hits, x, y);
        let changed = hit != self.hovered;
        self.hovered = hit;
        changed
    }

    fn pointer_scroll(&mut self, _x: f32, _y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        let rows = self.explain.as_ref().map_or(0, |explain| explain.env.len());
        let max = (rows as f32 * ROW_H - ROW_H).max(0.0);
        let next = (self.scroll - delta_y / ui::ui_text_scale()).clamp(0.0, max);
        let moved = (next - self.scroll).abs() > 0.01;
        self.scroll = next;
        moved
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let copy = self.copy.take();
        ItemTick {
            changed: copy.is_some(),
            clipboard_store: copy,
            ..ItemTick::default()
        }
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}
