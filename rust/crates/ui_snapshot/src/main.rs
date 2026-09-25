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

    if let Ok(which) = std::env::var("DATABASE") {
        use workspace::{Item, SidePanelView};
        let dir = std::env::temp_dir().join(format!("pom-snapshot-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("pom.yml"),
            "session: myproject\nshared_services:\n  postgres:\n    image: postgres:16\n  redis:\n    image: redis:7\nrepos:\n  api:\n    databases:\n      main: \"api_{{branch.safe}}\"\n  web:\n    databases:\n      main: \"web_{{branch.safe}}\"\n",
        )?;
        let config = std::sync::Arc::new(pom_config::Config::load(&dir.join("pom.yml"))?);
        let runner = std::sync::Arc::new(pom_services::ServiceRunner::new(
            pom_services::RunnerOptions {
                project_root: dir.clone(),
                session: "myproject".into(),
                state: pom_paths::StateDir::new(dir.join("state")),
                holders: pom_ptyhost::SocketDir::new(dir.join("s")),
                binary: "/nonexistent".into(),
                docker: "/nonexistent".into(),
            },
        ));
        let context = database_ui::DatabaseContext {
            runner,
            state: pom_paths::StateDir::new(dir.join("state")),
            config: std::sync::Arc::new(move || Some(config.clone())),
            branch: "feat-login".into(),
            waker: std::sync::Arc::new(|| {}),
        };
        let table = |schema: &str, name: &str, kind: pom_db::TableKind, count: Option<usize>| {
            pom_db::Table {
                schema: schema.into(),
                name: name.into(),
                kind,
                count,
            }
        };
        let (width, height) = if which == "panel" {
            (320.0_f32, 420.0_f32)
        } else if which == "console" {
            (900.0_f32, 280.0_f32)
        } else {
            (900.0_f32, 460.0_f32)
        };
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let databases = pom_db::list_databases(
            &pom_config::Config::load(&dir.join("pom.yml"))?,
            "feat-login",
        );
        let first = databases
            .first()
            .ok_or_else(|| anyhow::anyhow!("no database"))?;
        let mut consoles = vec![database_ui::new_console(&[], first)];
        consoles.push(database_ui::new_console(&consoles, first));
        consoles[1].id.push('b');
        context.save_consoles(&consoles);
        let painted = if which == "console" {
            use workspace::ItemFooter;
            let mut footer = database_ui::ConsoleFooter::new(context, consoles[0].clone());
            footer.show_result(Ok(pom_db::QueryResult {
                columns: ["id", "email", "role"].map(str::to_string).to_vec(),
                rows: (1..=12)
                    .map(|row| {
                        vec![
                            Some(row.to_string()),
                            Some(format!("user{row}@example.com")),
                            Some(if row % 3 == 0 { "admin" } else { "member" }.to_string()),
                        ]
                    })
                    .collect(),
                ..pom_db::QueryResult::default()
            }));
            footer.height(height);
            footer
                .paint(body)
                .ok_or_else(|| anyhow::anyhow!("no footer"))?
        } else if which == "panel" {
            let mut panel = database_ui::DatabasePanel::new(context);
            panel.set_filter("e");
            std::thread::sleep(std::time::Duration::from_millis(300));
            panel.show_tables(
                "myproject_api_feat-login",
                vec![
                    table("billing", "invoices", pom_db::TableKind::Table, None),
                    table("public", "sessions", pom_db::TableKind::Table, None),
                    table("public", "users", pom_db::TableKind::Table, None),
                    table("public", "active_users", pom_db::TableKind::View, None),
                ],
            );
            panel.show_tables(
                "redis",
                vec![
                    table("", "session", pom_db::TableKind::Keyspace, Some(42)),
                    table("", "user", pom_db::TableKind::Keyspace, Some(7)),
                ],
            );
            panel.set_hover(Some(
                workspace::side_panel_base(workspace::PaneKind::Database) + 5 * 4,
            ));
            let node = panel.render(width, height);
            ui::render(
                &ui::div()
                    .bg(ui::theme().panel_background)
                    .child(node)
                    .into(),
                body,
            )
        } else {
            let database = pom_db::list_databases(
                &pom_config::Config::load(&dir.join("pom.yml"))?,
                "feat-login",
            )
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no database"))?;
            let mut item = database_ui::TableItem::new(
                context,
                database,
                table("public", "users", pom_db::TableKind::Table, None),
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while item.is_busy() && std::time::Instant::now() < deadline {
                item.tick(&|| None);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let names = ["Ann", "Bob", "Cy", "Di", "Ed", "Flo", "Gus", "Hal"];
            let rows = (0..40)
                .map(|row| {
                    vec![
                        Some((row + 1).to_string()),
                        Some(format!(
                            "{}@example.com",
                            names[row % names.len()].to_lowercase()
                        )),
                        Some(names[row % names.len()].to_string()),
                        (row % 4 != 0)
                            .then(|| format!("2026-09-{:02} 10:{:02}:00", row % 28 + 1, row % 60)),
                        Some(if row % 3 == 0 { "admin" } else { "member" }.to_string()),
                    ]
                })
                .collect();
            item.show_page(
                pom_db::QueryResult {
                    columns: ["id", "email", "name", "last_seen_at", "role"]
                        .map(str::to_string)
                        .to_vec(),
                    rows,
                    ..pom_db::QueryResult::default()
                },
                Some(1234),
            );
            item.paint_body(body, true);
            item.pointer_down(
                330.0,
                37.0 + 27.0 + 22.0 * 2.5,
                1,
                terminal::Modifiers::default(),
            );
            item.pointer_move(
                200.0,
                37.0 + 27.0 + 22.0 * 5.5,
                terminal::Modifiers::default(),
                true,
            );
            item.paint_body(body, true)
                .ok_or_else(|| anyhow::anyhow!("no body"))?
        };
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            eprintln!("remove {}: {error}", dir.display());
        }
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(which) = std::env::var("ENVTAB") {
        use workspace::Item;
        let dir = std::env::temp_dir().join(format!("pom-snapshot-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("pom.yml"),
            "session: myproject\npresets:\n  rails:\n    env:\n      RAILS_ENV: development\n      RAILS_LOG_TO_STDOUT: \"1\"\nshared_services:\n  postgres:\n    image: postgres:16\nrepos:\n  api:\n    preset: [rails]\n    databases:\n      main: \"api_{{branch.safe}}\"\n      cache: \"api_cache_{{branch.safe}}\"\n    env:\n      DATABASE_URL: \"postgresql://{{shared.postgres.url}}/{{db.main}}\"\n      STRIPE_KEY: \"{{secret.STRIPE_KEY}}\"\n      APP_HOST: \"{{api.web.host}}\"\n    services:\n      web:\n        cmd: rails s\n",
        )?;
        let config = std::sync::Arc::new(pom_config::Config::load(&dir.join("pom.yml"))?);
        let state = pom_paths::StateDir::new(dir.join("state"));
        let store = pom_secrets::SecretStore::new(state.clone(), "myproject");
        for (name, value) in [
            ("STRIPE_KEY", "sk_test_123"),
            ("GITHUB_TOKEN", "ghp_example"),
            ("SENTRY_DSN", "https://example.invalid/1"),
        ] {
            store
                .set(name, value)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
        }
        let runner = std::sync::Arc::new(pom_services::ServiceRunner::new(
            pom_services::RunnerOptions {
                project_root: dir.clone(),
                session: "myproject".into(),
                state: state.clone(),
                holders: pom_ptyhost::SocketDir::new(dir.join("s")),
                binary: "/nonexistent".into(),
                docker: "/nonexistent".into(),
            },
        ));
        let context = environment_ui::EnvironmentContext {
            state,
            runner,
            config: std::sync::Arc::new(move || Some(config.clone())),
            workspaces: vec![("main".into(), true), ("feat-login".into(), false)],
            branch: "feat-login".into(),
        };
        let (width, height) = (820.0_f32, 420.0_f32);
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let mut item: Box<dyn Item> = if which == "secrets" {
            Box::new(environment_ui::SecretsItem::new(context))
        } else {
            Box::new(environment_ui::EnvItem::new(context))
        };
        let painted = item
            .paint_body(body, true)
            .ok_or_else(|| anyhow::anyhow!("no body"))?;
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            eprintln!("remove {}: {error}", dir.display());
        }
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(query) = std::env::var("SEARCH") {
        let root = std::env::current_dir()?;
        let mut item =
            files_ui::project_search_preview(root, &query, std::env::var("FILTERS").is_ok());
        let (width, height) = (900.0_f32, 560.0_f32);
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let painted = item
            .paint_body(body, true)
            .ok_or_else(|| anyhow::anyhow!("no body"))?;
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if let Ok(query) = std::env::var("FINDER") {
        let root = std::env::current_dir()?;
        let paths = files::project_files(&root, false);
        let recent = vec![
            "crates/files_ui/src/files_ui.rs".to_string(),
            "crates/workspace/src/workspace_view.rs".to_string(),
        ];
        let node = files_ui::file_finder_preview(paths, recent, &query);
        let (width, height) = (560.0_f32, 460.0_f32);
        let painted = ui::render(
            &ui::div().p(8.0).child(node).into(),
            ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT),
        );
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    if std::env::var("TICKETTAB").is_ok() {
        use workspace::Item;
        let (width, height) = (720.0_f32, 620.0_f32);
        let dir = std::env::temp_dir().join(format!("pom-snapshot-ticket-{}", std::process::id()));
        let mut item = jira_ui::TicketItem::new(
            pom_paths::StateDir::new(dir.join("state")),
            "myproject".into(),
            "PROJ-101".into(),
            std::sync::Arc::new(|| {}),
        );
        item.show(
            pom_jira::IssueDetail {
                key: "PROJ-101".into(),
                summary: "Flag escalated conversations in the inbox".into(),
                status: "In Progress".into(),
                url: "https://example.atlassian.net/browse/PROJ-101".into(),
                description: "## Background\n\nNothing marks a message where the sender **asks for a person**. See https://example.com/spec.\n\n## Acceptance criteria\n\n- [x] Detect a direct ask\n- [ ] Show a banner in the conversation\n- [ ] Email the team".into(),
                comments: vec![
                    pom_jira::Comment {
                        id: "1".into(),
                        author: "Ann".into(),
                        avatar: String::new(),
                        created: "2026-09-17T03:53:12.000+0700".into(),
                        body: "Should this cover *email* too?".into(),
                    },
                    pom_jira::Comment {
                        id: "2".into(),
                        author: "Bea".into(),
                        avatar: String::new(),
                        created: "2026-09-18T10:05:00.000+0700".into(),
                        body: "Yes, every channel. Use `inbox_flag`.".into(),
                    },
                ],
                web_links: vec![pom_jira::WebLink {
                    title: "Design doc".into(),
                    url: "https://example.com/design".into(),
                    icon: String::new(),
                }],
            },
            "indeterminate",
        );
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        let painted = item
            .paint_body(body, true)
            .ok_or_else(|| anyhow::anyhow!("no body"))?;
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        if dir.exists() {
            if let Err(error) = std::fs::remove_dir_all(&dir) {
                eprintln!("remove {}: {error}", dir.display());
            }
        }
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }
    if std::env::var("PRTAB").is_ok() {
        use workspace::Item;
        let (width, height) = (720.0_f32, 520.0_f32);
        let dir = std::env::temp_dir().join(format!("pom-snapshot-pr-{}", std::process::id()));
        let target = pom_forge::PrTarget {
            repo: "web".into(),
            owner: "acme".into(),
            name: "web".into(),
            head: "feat-login".into(),
        };
        let check = |name: &str, workflow: &str, result: &str| pom_forge::Check {
            name: name.into(),
            workflow_name: workflow.into(),
            details_url: "https://example.com".into(),
            result: result.into(),
            ..pom_forge::Check::default()
        };
        let mut pr = pom_forge::PullRequest {
            number: 42,
            title: "Add the login page with remember-me and rate limiting".into(),
            state: "OPEN".into(),
            head_ref_name: "feat-login".into(),
            base_ref_name: "main".into(),
            author: Some(pom_forge::Actor { login: "dev".into(), avatar_url: String::new() }),
            additions: 184,
            deletions: 23,
            body: "### Related ticket\n[PROJ-101](https://example.atlassian.net/browse/PROJ-101)\n\n### Description\nAdds the **login form** and `session` handling. See https://example.com/docs.\n\n- Remember me for *30 days*\n- Five attempts per minute per IP\n- [x] tests\n\n| Case | Result |\n|---|---|\n| valid | 200 |\n| locked | 423 |\n\n```rust\nfn login() -> bool { true }\n```".into(),
            labels: vec![
                pom_forge::Label { name: "feature".into(), color: "a2eeef".into() },
                pom_forge::Label { name: "needs-review".into(), color: "fbca04".into() },
            ],
            status_check_rollup: vec![
                check("test", "CI", "pass"),
                check("lint", "CI", "fail"),
                check("deploy-preview", "", "pending"),
            ],
            reviewers: vec![
                pom_forge::Reviewer { name: "ann".into(), state: "pending".into() },
                pom_forge::Reviewer { name: "bea".into(), state: "changes".into() },
                pom_forge::Reviewer { name: "cy".into(), state: "approved".into() },
            ],
            ..pom_forge::PullRequest::default()
        };
        pr.checks = "fail".into();
        pr.timeline = vec![
            pom_forge::TimelineItem {
                kind: pom_forge::TimelineKind::Review,
                author: "bea".into(),
                body: "The rate limit should key on the **account**, not only the IP.".into(),
                at: "2026-09-01T09:00:00Z".into(),
                state: "CHANGES_REQUESTED".into(),
                threads: vec![pom_forge::ReviewThread {
                    path: "src/auth/limit.rs".into(),
                    line: Some(42),
                    resolved: true,
                    comments: vec![
                        pom_forge::ThreadComment {
                            author: "bea".into(),
                            body: "This counter never resets.".into(),
                            at: "2026-09-01T09:00:00Z".into(),
                        },
                        pom_forge::ThreadComment {
                            author: "dev".into(),
                            body: "Fixed with a sliding window, see `Window::tick`.".into(),
                            at: "2026-09-01T12:00:00Z".into(),
                        },
                    ],
                }],
            },
            pom_forge::TimelineItem {
                kind: pom_forge::TimelineKind::Comment,
                author: "ann".into(),
                body: "QA notes are in https://example.com/qa.".into(),
                at: "2026-09-02T10:00:00Z".into(),
                ..pom_forge::TimelineItem::default()
            },
        ];
        let mut item = pull_request_ui::PrItem::new(
            pom_paths::StateDir::new(dir.join("state")),
            "myproject".into(),
            std::sync::Arc::new(|| {}),
            target,
            None,
        );
        item.show(Some(pr));
        let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
        item.paint_body(body, true);
        if std::env::var("CHECKS").is_ok() {
            item.pointer_down(110.0, 119.0, 1, terminal::Modifiers::default());
        }
        if let Some(scroll) = std::env::var("SCROLL")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
        {
            item.pointer_scroll(0.0, 0.0, -scroll, terminal::Modifiers::default());
        }
        let painted = item
            .paint_body(body, true)
            .ok_or_else(|| anyhow::anyhow!("no body"))?;
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().editor_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        if dir.exists() {
            if let Err(error) = std::fs::remove_dir_all(&dir) {
                eprintln!("remove {}: {error}", dir.display());
            }
        }
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    // With GITPANEL=<repo dir>, render the Git panel for that repository (against `main`).
    if let Ok(repo) = std::env::var("GITPANEL") {
        use workspace::SidePanelView;
        let (width, height) = (360.0_f32, 560.0_f32);
        // GITPANEL=<repo>[,<repo>...]; SELECTOR=1 opens the repository selector.
        let sources: Vec<git_ui::RepoSource> = repo
            .split(',')
            .map(|path| git_ui::RepoSource {
                name: std::path::Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                root: path.into(),
                default_branch: "main".into(),
            })
            .collect();
        let mut panel = git_ui::GitPanel::new(sources, None, std::sync::Arc::new(|| {}));
        panel.render(width, height);
        panel.wait_for_scan();
        if std::env::var("SELECTOR").is_ok() {
            panel.click(workspace::side_panel_base(workspace::PaneKind::Git) + 9_001_006);
        }
        if let Ok(hover) = std::env::var("HOVERROW") {
            if let Ok(row) = hover.parse::<u64>() {
                panel.set_hover(Some(
                    workspace::side_panel_base(workspace::PaneKind::Git) + row * 4,
                ));
            }
        }
        let node = panel.render(width, height);
        let mut r = ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
        let painted = ui::render(
            &ui::div()
                .bg(ui::theme().panel_background)
                .child(node)
                .into(),
            ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT),
        );
        let layers: Vec<ui::Layer> = vec![(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )];
        r.render_frame(ui::theme().panel_background, &layers)?;
        let (w, h, rgba) = r.read_rgba()?;
        let file = std::fs::File::create(&out)?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgba)?;
        println!("wrote {out} ({w}x{h})");
        return Ok(());
    }

    let scale = 2.0_f32;
    let (lw, lh) = match std::env::var("MAINVIEW") {
        Ok(_) => (
            std::env::var("SNAPW")
                .ok()
                .and_then(|width| width.parse().ok())
                .unwrap_or(1200.0_f32),
            780.0_f32,
        ),
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
            labels: vec![String::new(), "Login page".into(), String::new()],
            running: vec![0, 2, 0],
            tickets: vec![String::new(), String::new(), "In Progress".into()],
            ticket_categories: vec![String::new(), String::new(), "indeterminate".into()],
            prs: vec![
                None,
                Some(workspace::PrSummary {
                    count: 2,
                    severity: workspace::PrSeverity::Warn,
                }),
                Some(workspace::PrSummary {
                    count: 1,
                    severity: workspace::PrSeverity::Danger,
                }),
            ],
            missing: Vec::new(),
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
                // With DIFFFILE=<file> (and DIFFBASE=<file with its old text>), show that file's diff.
                let files_view = std::env::var("DIFFFILE").ok().map(|file| {
                    let path = std::path::PathBuf::from(&file);
                    let root = path
                        .parent()
                        .map(std::path::Path::to_path_buf)
                        .unwrap_or_default();
                    let mut files = files_ui::FilesView::new(root);
                    let base = std::env::var("DIFFBASE")
                        .ok()
                        .and_then(|base| std::fs::read_to_string(base).ok());
                    workspace::FunctionView::open_diff(&mut files, &path, base);
                    Box::new(files) as Box<dyn workspace::FunctionView>
                });
                // OPENFILES=<a,b,...>: open each file as a tab (the tab strip scrolls once they overflow).
                let files_view = files_view.or_else(|| {
                    let list = std::env::var("OPENFILES").ok()?;
                    let paths: Vec<std::path::PathBuf> =
                        list.split(',').map(std::path::PathBuf::from).collect();
                    let root = paths.first()?.parent()?.to_path_buf();
                    let mut files = files_ui::FilesView::new(root);
                    for path in &paths {
                        workspace::FunctionView::open_file_at(&mut files, path, None, None);
                    }
                    // MDPREVIEW=1: preview the last (markdown) file beside its editor.
                    if std::env::var("MDPREVIEW").is_ok() {
                        workspace::ItemInput::editor_key(
                            &mut files,
                            workspace::EditKey::OpenMarkdownPreviewToTheSide,
                            false,
                        );
                    }
                    Some(Box::new(files) as Box<dyn workspace::FunctionView>)
                });
                let mut view = workspace::WorkspaceView::new(workspace::Layout {
                    project,
                    files_view,
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
        // wscreate / wsrename: the workspace forms; wsops: creation cards (one running, one failed).
        let namer: workspaces_ui::Namer = std::sync::Arc::new(|seed: &str, _: &str| {
            Ok(pom_agent::NameSuggestion {
                name: seed.to_string(),
                slug: seed.to_string(),
            })
        });
        if mode == "wscreate" || mode == "wsticket" {
            let mut modal = workspaces_ui::CreateWorkspaceModal::new(
                vec!["api".into(), "web".into(), "mobile".into()],
                vec!["main".into(), "feat-login".into()],
                namer.clone(),
            );
            if mode == "wsticket" {
                let issue =
                    |key: &str, summary: &str, status: &str, mine: bool| pom_jira::SprintIssue {
                        key: key.into(),
                        summary: summary.into(),
                        status: status.into(),
                        mine,
                        ..pom_jira::SprintIssue::default()
                    };
                let issues = vec![
                    issue(
                        "PROJ-101",
                        "Payments page crashes on submit",
                        "In Progress",
                        true,
                    ),
                    issue("PROJ-104", "Add CSV export to reports", "To Do", false),
                    issue(
                        "PROJ-97",
                        "Login redirect loses the query string",
                        "To Do",
                        false,
                    ),
                ];
                modal = modal.with_tickets(workspaces_ui::TicketSource {
                    boards: std::sync::Arc::new(|| {
                        Ok(vec![pom_jira::Board {
                            id: 1,
                            name: "Team board".into(),
                        }])
                    }),
                    sprint: std::sync::Arc::new(move |_| Ok(issues.clone())),
                    board: None,
                    only_mine: false,
                });
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while workspace::WindowModal::busy(&modal) && std::time::Instant::now() < deadline {
                    workspace::WindowModal::tick(&mut modal);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            } else {
                workspace::WindowModal::text(&mut modal, "Fix checkout page");
                workspace::WindowModal::click(&mut modal, workspace::WINDOW_MODAL_BASE + 101);
            }
            entity.update(app.app_mut(), |view, _| {
                view.open_window_modal(Box::new(modal))
            });
        }
        if mode == "newproject" {
            let mut modal = workspaces_ui::NewProjectModal::new(
                std::path::PathBuf::from("/Users/dev/pom"),
                Box::new(|| {
                    vec![
                        std::path::PathBuf::from("/Users/dev/code/api"),
                        std::path::PathBuf::from("/Users/dev/code/web"),
                    ]
                }),
            );
            workspace::WindowModal::text(&mut modal, "myproject");
            workspace::WindowModal::click(&mut modal, workspace::WINDOW_MODAL_BASE + 6);
            workspace::WindowModal::click(&mut modal, workspace::WINDOW_MODAL_BASE + 4);
            workspace::WindowModal::text(&mut modal, "git@github.com:acme/worker.git");
            workspace::WindowModal::key(&mut modal, workspace::EditKey::Enter, false);
            workspace::WindowModal::click(&mut modal, workspace::WINDOW_MODAL_BASE + 201);
            workspace::WindowModal::text(&mut modal, "fe");
            entity.update(app.app_mut(), |view, _| {
                view.open_window_modal(Box::new(modal))
            });
        }
        if mode == "exportconfig" {
            let mut modal = workspaces_ui::ExportConfigModal::new(3);
            workspace::WindowModal::click(&mut modal, workspace::WINDOW_MODAL_BASE + 4);
            workspace::WindowModal::text(&mut modal, "secret");
            entity.update(app.app_mut(), |view, _| {
                view.open_window_modal(Box::new(modal))
            });
        }
        if mode == "importconfig" {
            let file = std::env::temp_dir().join("pom-snapshot-import.yml");
            let yaml = "session: myproject\ndefault_branch: main\nrepos:\n  api:\n    alias: be\n    services:\n      server:\n        type: backend\n        cmd: bundle exec rails s -p $PORT\n        env:\n          DATABASE_URL: \"{{db.main.url}}\"\n          API_KEY: \"{{secret.API_KEY}}\"\n  web:\n    services:\n      app:\n        cmd: pnpm dev --port $PORT\n        env:\n          API_URL: \"{{be.server.url}}\"\n";
            if let Err(error) = std::fs::write(&file, yaml) {
                eprintln!("snapshot: {error}");
            }
            let mut modal =
                workspaces_ui::ImportConfigModal::new(Box::new(move || Some(file.clone())));
            workspace::WindowModal::click(&mut modal, workspace::WINDOW_MODAL_BASE + 6);
            entity.update(app.app_mut(), |view, _| {
                view.open_window_modal(Box::new(modal))
            });
        }
        // RAIL=1: the WORKSPACES panel folded to its rail.
        if std::env::var("RAIL").is_ok() {
            entity.update(app.app_mut(), |view, _| {
                view.run_action(workspace::keymap::Action::ToggleLeftDock)
            });
        }
        if mode == "prompt" {
            entity.update(app.app_mut(), |view, _| {
                view.ask(workspace::Prompt {
                    token: 1,
                    message: "Stop all shared services?".into(),
                    detail: Some(
                        "Shared services are used by all workspaces; 12 services in other workspaces are still running."
                            .into(),
                    ),
                    buttons: vec!["Stop".into(), "Cancel".into()],
                })
            });
        }
        if mode == "wsrename" {
            let modal = workspaces_ui::RenameWorkspaceModal::new("feat-login", "Login page", namer);
            entity.update(app.app_mut(), |view, _| {
                view.open_window_modal(Box::new(modal))
            });
        }
        if mode == "wsops" {
            use workspace::{OpStatus, StageState, WorkspaceOp};
            let stages = |states: [StageState; 7]| -> Vec<(String, StageState)> {
                [
                    "Validating config and hosts",
                    "Provisioning workspace",
                    "Starting shared services and databases",
                    "Creating git worktrees (parallel)",
                    "Configuring repos (parallel)",
                    "Running setup commands (parallel)",
                    "Seeding databases (parallel)",
                ]
                .iter()
                .zip(states)
                .map(|(label, state)| (label.to_string(), state))
                .collect()
            };
            use StageState::{Done, Failed, Pending, Running};
            let ops = vec![
                WorkspaceOp {
                    id: 1,
                    branch: "fix-checkout".into(),
                    title: "Fix checkout page".into(),
                    status: OpStatus::Running,
                    stages: stages([Done, Done, Done, Running, Pending, Pending, Pending]),
                    detail: "worktree: web".into(),
                    error: String::new(),
                    retryable: true,
                    quiet: false,
                },
                WorkspaceOp {
                    id: 2,
                    branch: "proj-101".into(),
                    title: "PROJ-101 Payments".into(),
                    status: OpStatus::Failed,
                    stages: stages([Done, Done, Failed, Pending, Pending, Pending, Pending]),
                    detail: String::new(),
                    error: "shared services: docker: No such file or directory".into(),
                    retryable: true,
                    quiet: false,
                },
            ];
            entity.update(app.app_mut(), |view, _| {
                view.set_workspace_ops(ops);
                view.toggle_workspace_op(1);
            });
        }
        // Background work (the diff's hunks) settles over a few frames.
        for _ in 0..40 {
            app.draw(handle).expect("frame");
            std::thread::sleep(std::time::Duration::from_millis(25));
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
    // JIRA=1: the Integrations page as a configured project sees it.
    let jira = if std::env::var("JIRA").is_ok() {
        settings_ui::IntegrationsPage {
            session: "myproject".into(),
            keep_main_fresh: true,
            refresh_minutes: 30,
            site: "https://acme.atlassian.net".into(),
            email: "you@example.com".into(),
            token: settings_ui::TokenSource::Secret,
            status: settings_ui::ConnectionStatus::SignedIn("Sam (you@example.com)".into()),
        }
    } else {
        settings_ui::IntegrationsPage::default()
    };
    // LIVE=1: the Agent and Network pages with registration done and some proxy traffic.
    let live = std::env::var("LIVE").is_ok();
    let state = settings_ui::PageState {
        jira,
        agent: if live {
            settings_ui::AgentPage {
                mcp: settings_ui::Registration::Done,
                hooks: settings_ui::Registration::Done,
            }
        } else {
            settings_ui::AgentPage::default()
        },
        network: settings_ui::NetworkPage {
            proxy_running: live,
            webhook_running: live,
            proxy_port: 8767,
            webhook_port: 8766,
            served_elsewhere: false,
            requests: if live {
                vec![
                    settings_ui::RequestRow {
                        time: "10:42:07".into(),
                        method: "GET".into(),
                        path: "/_pom_dev/api/server/v1/me".into(),
                        profile: "local".into(),
                        target: "127.0.0.1:41822".into(),
                        status: 200,
                        ms: 12,
                    },
                    settings_ui::RequestRow {
                        time: "10:42:05".into(),
                        method: "POST".into(),
                        path: "/_pom_dev/api/server/v1/login".into(),
                        profile: "staging".into(),
                        target: "https://api.staging.example.com".into(),
                        status: 401,
                        ms: 184,
                    },
                ]
            } else {
                Vec::new()
            },
        },
        general: settings_ui::GeneralPage {
            start_at_login: false,
            version: env!("CARGO_PKG_VERSION").into(),
            updates_apply: live,
        },
        keymap: settings_ui::KeymapPage {
            rows: workspace::keymap::Action::ALL
                .iter()
                .map(|action| {
                    (
                        action.label().to_string(),
                        action.name().to_string(),
                        workspace::keymap::Keymap::defaults()
                            .binding_for(*action)
                            .unwrap_or_default(),
                    )
                })
                .collect(),
            problems: Vec::new(),
        },
    };
    let fs_edit = std::env::var("FSEDIT").ok();
    let editing = fs_edit
        .as_deref()
        .map(|b| (settings_ui::CTRL_FONT_SIZE_EDIT, b));
    let search = std::env::var("SEARCH").unwrap_or_default();

    // With SCROLL=<px>, exercise the scissor-clipped scrolling page path.
    if let Ok(scroll) = std::env::var("SCROLL").unwrap_or_default().parse::<f32>() {
        let clip = settings_ui::content_region(lw, lh);
        let (page, total_h) =
            settings_ui::page(category, &settings, &state, None, &search, lw, lh, scroll);
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
        &state,
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
