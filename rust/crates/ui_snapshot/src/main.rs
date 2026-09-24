//! Render a screen headlessly to a PNG so the UI can be eyeballed without launching a window. Usage:
//!   cargo run -p ui_snapshot -- [out.png] [category_index]
//! Defaults: /tmp/pomelo-ui.png, the Appearance page (all categories expanded).

use std::io::BufWriter;

fn main() -> anyhow::Result<()> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/pomelo-ui.png".into());
    let category = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(settings_ui::APPEARANCE);

    let scale = 2.0_f32;
    let (lw, lh) = match std::env::var("MAINVIEW") {
        Ok(_) => (1200.0_f32, 780.0_f32),
        Err(_) => (920.0_f32, 760.0_f32),
    }; // logical
    let mut r = ui::UiRenderer::new_headless((lw * scale) as u32, (lh * scale) as u32, scale)?;

    if let Ok(mode) = std::env::var("MAINVIEW") {
        let sessions: Vec<workspace::Session> = ["myproject", "api", "web", "old"]
            .iter()
            .map(|name| workspace::Session {
                name: name.to_string(),
                path: format!("/projects/{name}").into(),
                running: false,
                missing: *name == "old",
            })
            .collect();
        let project = (mode != "welcome").then(|| workspace::ProjectInfo {
            name: "myproject".into(),
            branch: "main".into(),
            config_path: "/projects/myproject/pom.yml".into(),
            workspaces: vec![
                "main".into(),
                "feat-login".into(),
                "proj-101-a-very-long-branch-name-for-the-workspace-panel".into(),
            ],
            active: "feat-login".into(),
        });
        let current = project.as_ref().map(|_| 0);
        let mut app = ui::Application::new();
        let (handle, entity) = app.open_raw_window(
            ui::WindowOptions {
                width: lw,
                height: lh,
                scale,
                ..Default::default()
            },
            move |_| {
                let mut view = workspace::WorkspaceView::new(workspace::Layout {
                    project,
                    ..Default::default()
                });
                view.set_sessions(sessions, current);
                view
            },
        );
        if mode == "problem" {
            entity.update(app.app_mut(), |view, _| {
                view.set_config_problem(
                    Some((
                        "/projects/myproject/pom.yml: unmarshal errors:\n  line 4: cannot unmarshal !!str `npm i` into []string".into(),
                        Some(4),
                    )),
                    std::path::Path::new("/projects/myproject/pom.yml"),
                )
            });
        }
        let frame = app.draw(handle).expect("frame");
        let mut layers: Vec<ui::Layer> = vec![(
            frame.base.rects.as_slice(),
            frame.base.tris.as_slice(),
            frame.base.texts.as_slice(),
            frame.base.icons.as_slice(),
            None,
        )];
        for overlay in &frame.overlays {
            layers.push((
                overlay.painted.rects.as_slice(),
                overlay.painted.tris.as_slice(),
                overlay.painted.texts.as_slice(),
                overlay.painted.icons.as_slice(),
                overlay.clip.map(|c| (c.x, c.y, c.w, c.h)),
            ));
        }
        r.render_frame(ui::theme().background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    // With MAINMENU=1, render the main window with the header session switcher menu open.
    if std::env::var("MAINMENU").is_ok() {
        let mut layout = workspace::Layout {
            session_menu: true,
            ..Default::default()
        };
        if let Ok(s) = std::env::var("MENUSCROLL")
            .unwrap_or_default()
            .parse::<f32>()
        {
            layout.session_scroll = s;
        }
        let (mut rects, mut texts) = layout.build(lw, lh);
        let mut tris: Vec<ui::Tri> = Vec::new();
        let hover = std::env::var("MENUHOVER")
            .ok()
            .and_then(|v| v.parse::<u64>().ok());
        let header = layout.header(lw, None);
        rects.extend(header.rects);
        tris.extend(header.tris);
        texts.extend(header.texts);
        let menu = layout.session_menu("", hover, true);
        let tip = hover.and_then(|hv| {
            let text = workspace::session_action_tooltip(hv)?;
            let rect = menu
                .list
                .hits
                .iter()
                .find(|(_, id)| *id == hv)
                .map(|(r, _)| *r)?;
            Some(workspace::tooltip(rect, text, lw))
        });
        let mut layers: Vec<ui::Layer> = vec![
            (
                rects.as_slice(),
                tris.as_slice(),
                texts.as_slice(),
                &[],
                None,
            ),
            (
                menu.fixed.rects.as_slice(),
                menu.fixed.tris.as_slice(),
                menu.fixed.texts.as_slice(),
                &[],
                None,
            ),
            (
                menu.list.rects.as_slice(),
                menu.list.tris.as_slice(),
                menu.list.texts.as_slice(),
                &[],
                Some(menu.clip),
            ),
        ];
        if let Some(t) = &tip {
            layers.push((
                t.rects.as_slice(),
                t.tris.as_slice(),
                t.texts.as_slice(),
                &[],
                None,
            ));
        }
        r.render_frame(ui::Rgba::new(0.0, 0.0, 0.0, 1.0), &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    let mut settings = settings::Settings::default();
    if let Ok(fs) = std::env::var("FONTSIZE").unwrap_or_default().parse::<f32>() {
        settings.ui_font_size = fs;
        ui::set_ui_text_scale(fs / ui::UI_FONT_BASE);
    }
    if let Ok(t) = std::env::var("THEME") {
        if t == "light" {
            ui::set_theme(ui::one_light());
        } else {
            settings.theme = t.clone();
            ui::set_theme(ui::by_name(&t));
        }
    }
    if let Ok(f) = std::env::var("UIFONT") {
        settings.ui_font = f;
    }
    if let Ok(w) = std::env::var("UIWEIGHT").unwrap_or_default().parse::<u16>() {
        settings.ui_font_weight = w as f32;
        ui::set_ui_font_weight(w);
    }
    r.set_ui_font(&settings.ui_font);
    let expanded = vec![true; settings_ui::CATEGORY_COUNT];
    let fs_edit = std::env::var("FSEDIT").ok();
    let editing = fs_edit
        .as_deref()
        .map(|b| (settings_ui::CTRL_FONT_SIZE_EDIT, b));
    let search = std::env::var("SEARCH").unwrap_or_default();

    // With SCROLL=<px>, exercise the scissor-clipped scrolling page path.
    if let Ok(scroll) = std::env::var("SCROLL").unwrap_or_default().parse::<f32>() {
        let clip = settings_ui::content_region(lw, lh);
        let (page, total_h) = settings_ui::page(category, &settings, None, &search, lw, lh, scroll);
        let mut chrome =
            settings_ui::chrome(category, None, Some(0), &expanded, lw, lh, &search, false);
        if let Some(bar) = settings_ui::content_scrollbar(clip, total_h, scroll) {
            chrome.rects.push(bar);
        }
        r.render_layered_clip(
            (&page.rects, &page.tris, &page.texts),
            (&chrome.rects, &chrome.tris, &chrome.texts),
            Some(clip),
            None,
            ui::theme().editor_background,
        )?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }
    let base = settings_ui::panel(
        category,
        None,
        &expanded,
        &settings,
        lw,
        lh,
        editing,
        &search,
        !search.is_empty(),
    );

    // With POPOVER=1, render an open Theme dropdown to verify the floating overlay.
    // With FONTPOP=1, render the (long, scrolled) font-family dropdown to verify the scrollbar.
    if std::env::var("POPOVER").is_ok() || std::env::var("FONTPOP").is_ok() {
        let font_pop = std::env::var("FONTPOP").is_ok();
        let cid = if font_pop {
            settings_ui::CTRL_FONT_FAMILY
        } else {
            settings_ui::CTRL_THEME
        };
        let anchor = base
            .hits
            .iter()
            .find(|(_, id)| *id == cid)
            .map(|(r, _)| *r)
            .expect("control anchor");
        let fonts = if font_pop { r.font_families() } else { vec![] };
        let items = settings_ui::control_items(cid, &fonts);
        let query = std::env::var("QUERY").unwrap_or_default();
        let (current, scroll) = if font_pop {
            (
                settings.ui_font.clone(),
                if query.is_empty() { 8 } else { 0 },
            )
        } else {
            (settings.theme.clone(), 0)
        };
        let hover = std::env::var("HOVER")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|i| settings_ui::POPOVER_BASE + i);
        let pop = settings_ui::popover(anchor, &current, &items, scroll, &query, hover, lw, lh);
        r.render_layered(
            (&base.rects, &base.tris, &base.texts),
            (&pop.rects, &pop.tris, &pop.texts),
        )?;
    } else {
        r.render(&base.rects, &base.tris, &base.texts)?;
    }

    let (w, h, rgba) = r.read_rgba()?;
    let file = std::fs::File::create(&out)?;
    let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    println!("wrote {out} ({w}x{h})");
    Ok(())
}
