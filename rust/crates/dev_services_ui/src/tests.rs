use super::*;

fn texts(page: &mut DevRequestsPage) -> Vec<String> {
    let painted = page
        .paint_body(Rect::new(0.0, 0.0, 1200.0, 1600.0, Rgba::TRANSPARENT), true)
        .unwrap_or_default();
    painted.texts.iter().map(|text| text.text.clone()).collect()
}

#[test]
fn filters_narrow_the_list() {
    let page = preview_page();
    let mut state = page.shared.borrow_mut();
    assert_eq!(state.visible().count(), 5);
    click(&mut state, KIND_BASE + 2);
    assert_eq!(state.visible().count(), 1);
    click(&mut state, KIND_BASE);
    click(&mut state, STATUS_BASE + 1);
    let failing: Vec<u64> = state.visible().map(|entry| entry.seq).collect();
    assert_eq!(failing, [5, 4, 2]);
    click(&mut state, STATUS_BASE);
    state.filter.insert("login");
    let matched: Vec<u64> = state.visible().map(|entry| entry.seq).collect();
    assert_eq!(matched, [4]);
}

#[test]
fn the_side_shows_the_body_and_hides_credentials_until_shown() {
    let mut page = preview_page();
    let shown = texts(&mut page);
    assert!(shown.concat().contains("\"type\": \"invoice.paid\""));
    assert!(shown.iter().any(|text| text == MASK));
    assert!(!shown.iter().any(|text| text.contains("sk_test_secret")));
    page.click(REVEAL);
    assert!(texts(&mut page)
        .iter()
        .any(|text| text.contains("sk_test_secret")));
    page.click(TAB_BASE + 1);
    let fan_out = texts(&mut page);
    assert!(fan_out.iter().any(|text| text == "feat-login"));
    assert!(fan_out.iter().any(|text| text == "connection refused"));
    assert!(fan_out.iter().any(|text| text == "1 of 2 failed"));
    page.click(ROW_BASE + 6);
    let state = page.shared.borrow();
    assert!(!state.revealed, "a new selection hides credentials again");
    assert_eq!(
        state.tab,
        DetailTab::Request,
        "a proxied request has no fan-out tab"
    );
    assert_eq!(state.wants_payloads(), Some(6));
}

#[test]
fn copy_puts_the_formatted_body_on_the_clipboard() {
    let mut page = preview_page();
    page.click(COPY_BODY);
    let copied = page.tick(&|| None).clipboard_store.unwrap_or_default();
    assert!(
        copied.starts_with("{\n  \"id\": \"evt_1Q2xYz\",\n  \"type\""),
        "{copied}"
    );
}

#[test]
fn bodies_render_by_kind() {
    let json = Payload {
        bytes: b"{\"a\":1}".to_vec(),
        total: 7,
        complete: true,
        evicted: false,
    };
    assert_eq!(
        body_text(&json, &[]),
        Some(("{\n  \"a\": 1\n}".to_string(), "JSON"))
    );
    assert_eq!(
        pretty_json(r#"{"z":[1, {"b":"x,y{"}], "a":{}, "s":"q\"t"}"#),
        "{\n  \"z\": [\n    1,\n    {\n      \"b\": \"x,y{\"\n    }\n  ],\n  \"a\": {},\n  \"s\": \"q\\\"t\"\n}"
    );
    let form = Payload {
        bytes: b"a=1&b=2".to_vec(),
        ..json.clone()
    };
    let headers = [(
        "Content-Type".to_string(),
        "application/x-www-form-urlencoded".to_string(),
    )];
    assert_eq!(
        body_text(&form, &headers),
        Some(("a=1&b=2".to_string(), "Form"))
    );
    let binary = Payload {
        bytes: vec![0xff, 0xfe, 0x00, 0x81],
        ..json
    };
    assert_eq!(body_text(&binary, &[]), None);
    assert!(sensitive("Authorization") && sensitive("x-api-key") && sensitive("X-Auth-Token"));
    assert!(!sensitive("stripe-signature") && !sensitive("content-type"));
}

#[test]
fn the_filter_takes_typing_while_focused() {
    let mut page = preview_page();
    page.click(FILTER);
    assert!(page.wants_keystrokes());
    page.input_text("orders");
    assert_eq!(page.shared.borrow().visible().count(), 1);
    page.keystroke(&Keystroke::new("backspace", Modifiers::default()));
    assert_eq!(page.shared.borrow().filter.text(), "order");
    page.keystroke(&Keystroke::new("escape", Modifiers::default()));
    assert!(!page.wants_keystrokes());
}

#[test]
fn restart_is_handed_to_the_app_unless_a_terminal_serves_the_ports() {
    let mut page = preview_page();
    page.click(RESTART);
    assert_eq!(page.shared.borrow().requests, [Request::Restart]);
    assert!(texts(&mut page).iter().any(|text| text == "Restart"));
    page.shared.borrow_mut().served_elsewhere = true;
    assert!(!texts(&mut page).iter().any(|text| text == "Restart"));
}

#[test]
fn json_lines_split_into_the_editors_captures() {
    let captures = |line: &str| -> Vec<(String, &'static str)> {
        json_runs(line)
            .into_iter()
            .map(|(text, capture)| (text.to_string(), capture))
            .filter(|(_, capture)| !capture.is_empty())
            .collect()
    };
    assert_eq!(
        captures(r#"  "id": "evt_\"1\"","#),
        [
            (r#""id""#.to_string(), "property.json_key"),
            (":".into(), "punctuation.delimiter"),
            (r#""evt_\"1\"""#.into(), "string"),
            (",".into(), "punctuation.delimiter"),
        ]
    );
    assert_eq!(
        captures(r#"  "n": -4.5e3, "ok": true, "x": null }"#)
            .into_iter()
            .map(|(_, capture)| capture)
            .collect::<Vec<_>>(),
        [
            "property.json_key",
            "punctuation.delimiter",
            "number",
            "punctuation.delimiter",
            "property.json_key",
            "punctuation.delimiter",
            "boolean",
            "punctuation.delimiter",
            "property.json_key",
            "punctuation.delimiter",
            "constant.builtin",
            "punctuation.bracket",
        ]
    );
    assert_eq!(json_runs("    ").len(), 1, "indent is one uncolored run");
}
