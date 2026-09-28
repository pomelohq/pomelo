//! A real onboarding run without a window: the form filled with `E2E_REPOS` (folders or git URLs), then
//! clone, scan and verify for real (manual setup), rendering the page to `<out>-<n>-<phase>.png` as it goes.
//! State, sessions and holders live in a scratch folder that is removed at the end.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::BufWriter;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;

use onboarding_ui::{OnboardingPage, Phase, Shared};
use workspace::Item;

fn snapshot(
    page: &mut OnboardingPage,
    out: &str,
    index: &mut usize,
    name: &str,
) -> anyhow::Result<()> {
    let (width, height) = (1000.0_f32, 1400.0_f32);
    let body = ui::Rect::new(0.0, 0.0, width, height, ui::Rgba::TRANSPARENT);
    let painted = page.paint_body(body, true).unwrap_or_default();
    let mut renderer =
        ui::UiRenderer::new_headless((width * 2.0) as u32, (height * 2.0) as u32, 2.0)?;
    renderer.render_frame(
        ui::theme().editor_background,
        &[(
            painted.rects.as_slice(),
            painted.tris.as_slice(),
            painted.texts.as_slice(),
            painted.icons.as_slice(),
            None,
        )],
    )?;
    let (w, h, rgba) = renderer.read_rgba()?;
    let path = format!("{out}-{index}-{name}.png");
    let file = std::fs::File::create(&path)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;
    println!("wrote {path}");
    *index += 1;
    Ok(())
}

pub fn run(out: &str, repos: &str) -> anyhow::Result<()> {
    let scratch = std::path::PathBuf::from(format!("/tmp/poe2e-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let state_dir = pom_paths::StateDir::new(scratch.join("state"));
    let sessions_root = scratch.join("sessions");
    let binary = std::fs::canonicalize(
        std::env::var("POM_APP_BINARY").unwrap_or_else(|_| "target/debug/pomelo".into()),
    )?;
    let agents = vec![
        (pom_agent::AgentCli::Claude, true),
        (pom_agent::AgentCli::Codex, false),
        (pom_agent::AgentCli::Gemini, false),
    ];
    let shared: Shared = Rc::new(RefCell::new(onboarding_ui::Onboarding::new(
        sessions_root.clone(),
        agents,
    )));
    let mut page = OnboardingPage::new(shared.clone());
    let mut index = 0;
    {
        let mut state = shared.borrow_mut();
        state.form.name.set_text("e2e-demo");
        for repo in repos
            .split(',')
            .map(str::trim)
            .filter(|repo| !repo.is_empty())
        {
            if !state.add_repo(repo.to_string()) {
                eprintln!("not added: {repo}");
            }
        }
        state.form.first_workspace = false;
        state.form.focus = None;
    }
    snapshot(&mut page, out, &mut index, "repositories")?;
    {
        let mut state = shared.borrow_mut();
        state.form.step = onboarding_ui::Step::Setup;
        state.form.with_agent = false;
    }
    snapshot(&mut page, out, &mut index, "setup")?;
    shared.borrow_mut().form.step = onboarding_ui::Step::Review;
    snapshot(&mut page, out, &mut index, "review")?;

    let request = {
        let state = shared.borrow();
        pom_core::ScaffoldRequest {
            name: state.form.name(),
            root: sessions_root.to_string_lossy().into_owned(),
            default_branch: state.form.default_branch(),
            repos: state
                .form
                .repos
                .iter()
                .map(|repo| pom_core::RepoSpec {
                    path: repo.source.clone(),
                    alias: String::new(),
                })
                .collect(),
            skip_secrets: true,
        }
    };
    shared.borrow_mut().start_run();
    let (sender, receiver) = std::sync::mpsc::channel();
    let thread_state = state_dir.clone();
    let worker = std::thread::spawn(move || {
        let events = sender.clone();
        let result = pom_core::scaffold_session_with(
            &request,
            &thread_state,
            &mut |event| {
                if events.send(Ok(event)).is_err() {
                    eprintln!("e2e: nobody listens");
                }
            },
            &AtomicBool::new(false),
        );
        if sender.send(Err(result)).is_err() {
            eprintln!("e2e: nobody listens");
        }
    });
    let mut shown_clone = false;
    let session_dir = loop {
        match receiver.recv()? {
            Ok(event) => {
                let mut state = shared.borrow_mut();
                let Some(run) = state.run.as_mut() else {
                    continue;
                };
                match event {
                    pom_core::ScaffoldEvent::Cloning { repo, percent } => {
                        if let Some(clone) = run.clone.get_mut(repo) {
                            clone.percent = percent;
                        }
                        let halfway = percent >= 40 && !shown_clone;
                        drop(state);
                        if halfway {
                            shown_clone = true;
                            snapshot(&mut page, out, &mut index, "cloning")?;
                        }
                    }
                    pom_core::ScaffoldEvent::Cloned { repo, linked } => {
                        if let Some(clone) = run.clone.get_mut(repo) {
                            clone.percent = 100;
                            clone.linked = Some(linked);
                        }
                        if run.clone.iter().all(|clone| clone.linked.is_some()) {
                            run.enter(Phase::Scan);
                        }
                    }
                    pom_core::ScaffoldEvent::Scanned(scans) => run.scans = scans,
                }
            }
            Err(result) => break result,
        }
    };
    if let Err(error) = worker.join() {
        eprintln!("e2e: scaffold thread: {error:?}");
    }
    let session_dir = match session_dir {
        Ok(dir) => dir,
        Err(error) => {
            if let Some(run) = shared.borrow_mut().run.as_mut() {
                run.error = Some(error.clone());
            }
            snapshot(&mut page, out, &mut index, "failed")?;
            std::fs::remove_dir_all(&scratch)?;
            anyhow::bail!("scaffold: {error}");
        }
    };
    println!(
        "--- pom.yml drafted by the scan ---\n{}",
        std::fs::read_to_string(session_dir.join("pom.yml"))?
    );
    let config = pom_config::Config::load(&session_dir.join("pom.yml"))?;
    {
        let mut state = shared.borrow_mut();
        if let Some(run) = state.run.as_mut() {
            run.enter(Phase::Configure);
            run.summary = onboarding::summary_lines(&config);
            run.enter(Phase::Verify);
        }
    }
    snapshot(&mut page, out, &mut index, "configured")?;
    let runner = pom_services::ServiceRunner::new(pom_services::RunnerOptions {
        project_root: session_dir.clone(),
        session: "e2e-demo".into(),
        state: state_dir.clone(),
        holders: pom_ptyhost::SocketDir::new(scratch.join("s")),
        binary,
        docker: "docker".into(),
    });
    let path = pom_services::tool_path();
    let has_tool = |tool: &str| pom_doctor::on_path(path, tool);
    let docker_running = || pom_doctor::docker_answers(path);
    let machine = pom_doctor::Machine {
        has_tool: &has_tool,
        docker_running: &docker_running,
    };
    let installed = HashMap::new();
    let skipped: Vec<(String, String)> = std::env::var("E2E_SKIP")
        .unwrap_or_default()
        .split(',')
        .filter_map(|pair| pair.split_once('/'))
        .map(|(repo, service)| (repo.to_string(), service.to_string()))
        .collect();
    let input = onboarding::VerifyInput {
        config: &config,
        config_path: &session_dir.join("pom.yml"),
        root: &session_dir,
        secret_names: &[],
        machine: &machine,
        installed: &installed,
        skipped: &skipped,
        boot_timeout: std::time::Duration::from_secs(60),
        worker_grace: onboarding::WORKER_GRACE,
    };
    let services = onboarding::RunnerServices {
        runner: &runner,
        config: &config,
    };
    let checks = onboarding::verify(
        &input,
        &services,
        &mut |check| {
            println!(
                "check: {} {:?} {}",
                check.kind.title(),
                check.status,
                check.detail
            );
            if let Some(run) = shared.borrow_mut().run.as_mut() {
                run.record(check.clone());
            }
        },
        &AtomicBool::new(false),
    );
    let clean = checks
        .iter()
        .all(|check| check.status == onboarding::CheckStatus::Passed);
    if clean {
        {
            let mut state = shared.borrow_mut();
            if let Some(run) = state.run.as_mut() {
                run.skipped = skipped.clone();
                run.counts = onboarding::counts(&config);
                let scans = run.scans.clone();
                run.set_up = config
                    .repos
                    .iter()
                    .map(|(repo, dir)| {
                        let stack = scans
                            .iter()
                            .find(|scan| scan.name == *repo)
                            .map(|scan| scan.stack.clone())
                            .unwrap_or_default();
                        let services: Vec<String> = dir.services.keys().cloned().collect();
                        (repo.clone(), stack, services.join(", "))
                    })
                    .collect();
            }
            state.finish();
        }
        snapshot(&mut page, out, &mut index, "ready")?;
    } else {
        snapshot(&mut page, out, &mut index, "repair")?;
    }
    for holder in runner.running_holders() {
        if let Err(error) = runner.holders().kill_holder(&holder) {
            eprintln!("e2e: stop {holder}: {error}");
        }
    }
    std::fs::remove_dir_all(&scratch)?;
    println!("removed {}", scratch.display());
    Ok(())
}
