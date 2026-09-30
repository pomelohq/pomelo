use super::*;

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
        .any(|text| text.starts_with("Free ") && text.ends_with(" Unused")));
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
    click(&mut state, FREE_UNUSED);
    assert_eq!(state.requests, [requests[1].clone(), Request::FreeUnused]);
    state.requests.clear();
    state.busy = Some("Working...".into());
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
