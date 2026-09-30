use super::*;

fn popover_texts(state: &StoreState) -> Vec<String> {
    let Some(node) = popover(state, None) else {
        return Vec::new();
    };
    let painted = ui::render(&node, Rect::new(0.0, 0.0, 500.0, 400.0, Rgba::TRANSPARENT));
    painted.texts.iter().map(|text| text.text.clone()).collect()
}

fn texts(state: &StoreState, width: f32) -> Vec<String> {
    let painted = ui::render(
        &render(state, "/store", width, None),
        Rect::new(0.0, 0.0, width, 3000.0, Rgba::TRANSPARENT),
    );
    painted.texts.iter().map(|text| text.text.clone()).collect()
}

#[test]
fn versions_read_as_what_a_new_workspace_gets() {
    let page = preview_page();
    let state = page.shared.borrow();
    let shown = texts(&state, 1200.0);
    for expected in [
        "Current on main",
        "Changed on feat-react",
        "New workspaces",
        "2 old versions",
        "feat-pay has its own copy",
        "Use Shared Copy",
        "Save to Store",
        "pnpm shares packages itself; Pomelo leaves it alone.",
        "Other projects",
    ] {
        assert!(shown.iter().any(|text| text == expected), "{expected}");
    }
    assert!(shown
        .iter()
        .any(|text| text.starts_with("Free ") && text.ends_with(" unused")));
}

#[test]
fn a_long_workspace_list_folds_into_more_and_expands() {
    let page = preview_page();
    let mut state = page.shared.borrow_mut();
    let folded = texts(&state, 1200.0);
    let more = folded
        .iter()
        .find(|text| text.starts_with('+') && text.ends_with(" more"))
        .cloned()
        .expect("a +N more chip");
    assert!(!folded.iter().any(|text| text == "spike-cache"));
    let narrow = texts(&state, 900.0);
    let hidden = |text: &str| {
        text.trim_start_matches('+')
            .trim_end_matches(" more")
            .parse::<usize>()
            .unwrap_or(0)
    };
    let narrow_more = narrow
        .iter()
        .find(|text| text.starts_with('+') && text.ends_with(" more"))
        .expect("still folded");
    assert!(
        hidden(narrow_more) >= hidden(&more),
        "a narrower tab shows fewer chips"
    );
    click(&mut state, MORE_BASE);
    let open = texts(&state, 1200.0);
    assert!(open.iter().any(|text| text == "spike-cache"));
    assert!(open.iter().any(|text| text == "Show less"));
    click(&mut state, MORE_BASE);
    assert!(state.expanded.is_empty());
}

#[test]
fn buttons_hand_their_request_to_the_app_and_wait_while_busy() {
    let page = preview_page();
    let mut state = page.shared.borrow_mut();
    let requests = actions(&state);
    assert_eq!(
        requests,
        [
            Request::FreeOld { repo: "api".into() },
            Request::Relink {
                repo: "web".into(),
                workspace: "feat-pay".into(),
                path: PathBuf::from("/work/feat-pay/api"),
            },
            Request::Keep {
                repo: "admin".into(),
                path: PathBuf::from("/work/main/api"),
            },
            Request::FreeOld {
                repo: "admin".into()
            },
            Request::FreeOthers,
        ]
    );
    click(&mut state, ACTION_BASE + 1);
    assert!(state.requests.is_empty(), "a swap asks first");
    assert_eq!(state.confirm, Some(requests[1].clone()));
    let shown = popover_texts(&state);
    assert!(shown
        .iter()
        .any(|text| text == "Use the shared copy in feat-pay?"));
    assert!(shown
        .iter()
        .any(|text| text.starts_with("Its services are running")));
    click(&mut state, CONFIRM_SWAP);
    click(&mut state, FREE_UNUSED);
    assert_eq!(state.requests, [requests[1].clone(), Request::FreeUnused]);
    assert_eq!(state.confirm, None);
    state.requests.clear();
    state.busy = Some(Request::Refresh);
    click(&mut state, ACTION_BASE);
    click(&mut state, REFRESH);
    assert!(state.requests.is_empty());
}

#[test]
fn ages_read_naturally() {
    assert_eq!(ago(1000, 990), "just now");
    assert_eq!(ago(10_000, 10_000 - 300), "5 min ago");
    assert_eq!(ago(100_000, 100_000 - 7200), "2 h ago");
    assert_eq!(ago(300_000, 300_000 - 100_000), "yesterday");
    assert_eq!(ago(1_000_000, 1_000_000 - 5 * 86_400), "5 days ago");
}

#[test]
fn unmeasured_sizes_shimmer_and_the_header_counts_them() {
    let page = preview_page();
    let mut state = page.shared.borrow_mut();
    state.measuring = 2;
    state.measure_total = 5;
    let shown = texts(&state, 1200.0);
    assert!(shown
        .iter()
        .any(|text| text.starts_with("measuring 3 of 5")));
    assert!(
        !shown.iter().any(|text| text.ends_with(" unused")),
        "Free waits for the sizes"
    );
    assert!(state.animating());
    state.measuring = 0;
    state.finish(Some("Freed 2.1 GB".into()), false, Some("k1".into()));
    assert!(texts(&state, 1200.0)
        .iter()
        .any(|text| text == "Freed 2.1 GB"));
    state.clock += NOTE_FOR + 1.0;
    assert!(!state.animating());
    assert!(!texts(&state, 1200.0)
        .iter()
        .any(|text| text == "Freed 2.1 GB"));
}

#[test]
fn optimize_keeps_then_moves_then_frees_and_asks_first() {
    let page = preview_page();
    let mut state = page.shared.borrow_mut();
    let steps = plan(state.overview.as_ref().unwrap());
    assert_eq!(
        steps,
        [
            Request::Keep {
                repo: "admin".into(),
                path: PathBuf::from("/work/main/api"),
            },
            Request::Relink {
                repo: "web".into(),
                workspace: "feat-pay".into(),
                path: PathBuf::from("/work/feat-pay/api"),
            },
            Request::FreeUnused,
        ]
    );
    click(&mut state, OPTIMIZE);
    assert!(state.requests.is_empty(), "optimize shows its plan first");
    let shown = popover_texts(&state);
    assert!(shown
        .iter()
        .any(|text| text.starts_with("Optimize: 1 install kept")));
    assert!(shown
        .iter()
        .any(|text| text.starts_with("Services are stopped and started again in feat-pay")));
    click(&mut state, CONFIRM_OPTIMIZE);
    assert_eq!(state.requests, [Request::Optimize]);
    state.requests.clear();
    state.busy = Some(steps[1].clone());
    state.progress = Some((2, 3));
    assert!(texts(&state, 1200.0)
        .iter()
        .any(|text| text == "Optimizing 2 of 3 - Swapping feat-pay to the shared copy"));
}
