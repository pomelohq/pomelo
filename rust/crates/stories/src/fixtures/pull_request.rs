//! A pull request as GitHub would describe it, for the tab that shows one.

/// The pull request tab in the state `view` names: `overview`, `checks`, `loading`, `none`, `error`, `empty`
/// (a draft with nothing written) or `merge-confirm`. Nothing reaches GitHub.
pub fn item(view: &str) -> pull_request_ui::PrItem {
    let target = pom_forge::PrTarget {
        repo: "api".into(),
        owner: "acme".into(),
        name: "api".into(),
        head: "feat-login".into(),
    };
    let check = |name: &str, workflow: &str, result: &str, took: (&str, &str)| pom_forge::Check {
        name: name.into(),
        workflow_name: workflow.into(),
        details_url: "https://example.com".into(),
        result: result.into(),
        started_at: took.0.into(),
        completed_at: took.1.into(),
        ..pom_forge::Check::default()
    };
    let empty = view == "empty";
    let mut pr = pom_forge::PullRequest {
        id: "PR_482".into(),
        head_ref_oid: "4f2a9c1".into(),
        number: 482,
        title: "Paginate the users index".into(),
        state: "OPEN".into(),
        is_draft: empty,
        mergeable: "MERGEABLE".into(),
        merge_state_status: if empty { "BEHIND" } else { "CLEAN" }.into(),
        head_ref_name: "feat-login".into(),
        base_ref_name: "main".into(),
        author: Some(pom_forge::Actor {
            login: "alex".into(),
            avatar_url: String::new(),
        }),
        additions: 51,
        deletions: 4,
        ..pom_forge::PullRequest::default()
    };
    if !empty {
        pr.body = "Adds page and per params to the users index, with an X-Total-Count header for the client. Closes PROJ-101.".into();
        pr.labels = vec![
            pom_forge::Label {
                name: "backend".into(),
                color: "86b6ec".into(),
            },
            pom_forge::Label {
                name: "api".into(),
                color: "8fc27a".into(),
            },
        ];
        pr.status_check_rollup = vec![
            check(
                "rspec",
                "CI / test",
                "fail",
                ("2026-10-20T09:00:00Z", "2026-10-20T09:02:41Z"),
            ),
            check(
                "rubocop",
                "CI / lint",
                "pass",
                ("2026-10-20T09:00:00Z", "2026-10-20T09:00:38Z"),
            ),
            check(
                "brakeman",
                "CI / security",
                "pass",
                ("2026-10-20T09:00:00Z", "2026-10-20T09:00:51Z"),
            ),
            check(
                "deploy-preview",
                "Preview",
                "pending",
                ("2026-10-20T09:00:00Z", ""),
            ),
        ];
        pr.checks = "fail".into();
        pr.review = "changes".into();
        pr.reviewers = vec![
            pom_forge::Reviewer {
                name: "bea".into(),
                state: "changes".into(),
            },
            pom_forge::Reviewer {
                name: "sam".into(),
                state: "pending".into(),
            },
        ];
        pr.timeline = vec![
            pom_forge::TimelineItem {
                kind: pom_forge::TimelineKind::Review,
                author: "bea".into(),
                body: "Clamp per so a client cannot ask for every row at once.".into(),
                at: "2026-10-20T09:12:00Z".into(),
                state: "CHANGES_REQUESTED".into(),
                threads: vec![pom_forge::ReviewThread {
                    id: "PRRT_1".into(),
                    path: "app/controllers/users_controller.rb".into(),
                    line: Some(7),
                    resolved: true,
                    comments: Vec::new(),
                }],
            },
            pom_forge::TimelineItem {
                kind: pom_forge::TimelineKind::Inline,
                author: "alex".into(),
                body: "Done, capped at 100 and covered in the request spec.".into(),
                at: "2026-10-20T10:03:00Z".into(),
                ..pom_forge::TimelineItem::default()
            },
            pom_forge::TimelineItem {
                kind: pom_forge::TimelineKind::Comment,
                author: "sam".into(),
                body: "Can the client read the total from a header instead?".into(),
                at: "2026-10-20T10:40:00Z".into(),
                ..pom_forge::TimelineItem::default()
            },
        ];
    }
    // The merge confirmation needs a pull request with nothing blocking it.
    if view == "merge-confirm" {
        pr.checks = "pass".into();
        pr.review = "approved".into();
        pr.review_decision = "APPROVED".into();
    }
    let mut item = pull_request_ui::PrItem::preview(target);
    match view {
        "loading" => {}
        "none" => item.show(None),
        "error" => item.show_error("No GitHub token: set GH_TOKEN or the session's github secret"),
        _ => item.show(Some(pr)),
    }
    if view == "checks" {
        item.show_checks();
    }
    item
}
