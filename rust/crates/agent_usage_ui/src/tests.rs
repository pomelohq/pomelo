use super::*;

fn turn(workspace: &str, kind: AgentKind, day: i64, session: &str, cost: f64) -> UsageTurn {
    UsageTurn {
        workspace: workspace.into(),
        kind,
        model: "claude-opus-5-5".into(),
        day,
        session: session.into(),
        tokens: (cost * 1000.0) as u64,
        output: 10,
        cache_read: (cost * 900.0) as u64,
        cost,
    }
}

fn state() -> Shared {
    Rc::new(RefCell::new(UsageState {
        turns: vec![
            turn("feat-login", AgentKind::Main, 100, "a", 5.0),
            turn("feat-login", AgentKind::Side, 99, "b", 1.0),
            turn("main", AgentKind::Main, 100, "c", 2.0),
            turn("main", AgentKind::Main, 80, "old", 50.0),
        ],
        today: 100,
        ..UsageState::default()
    }))
}

#[test]
fn a_period_groups_its_turns_and_a_workspace_row_filters_to_it() {
    let shared = state();
    let mut page = UsagePage::new(shared.clone());
    {
        let state = shared.borrow();
        let turns = state.in_period(0, state.period);
        assert_eq!(turns.len(), 3, "the 30-day-old turn is outside 7 days");
        let groups = state.groups(&turns);
        assert_eq!(groups[0].key, "feat-login", "the costliest workspace first");
    }
    page.click(ROW_BASE);
    assert_eq!(shared.borrow().filter.as_deref(), Some("feat-login"));
    assert_eq!(shared.borrow().group, Group::Agent);
    page.click(OPEN_BASE);
    assert_eq!(
        shared.borrow_mut().requests.pop(),
        Some(Request::OpenSession {
            session: "a".into(),
            workspace: "feat-login".into()
        })
    );
    page.click(CLEAR_FILTER);
    page.click(PERIOD_BASE + 2);
    let state = shared.borrow();
    assert_eq!(state.period, 30);
    assert_eq!(state.in_period(0, 30).len(), 4);
    assert!(ui::measure(&render(&state, 1000.0, None)).1 > 400.0);
}

#[test]
fn tokens_and_costs_read_short() {
    assert_eq!(format_tokens(1_234.0), "1K");
    assert_eq!(format_tokens(4_547_117_590.0), "4.5B");
    assert_eq!(format_cost(8006.84), "$8007");
    assert_eq!(format_cost(1.456), "$1.46");
    assert_eq!(day_name(0), "Thu");
}
