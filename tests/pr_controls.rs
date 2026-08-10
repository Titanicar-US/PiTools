#[path = "../src/pr_controls.rs"]
mod pr_controls;

use serde_json::json;

use pr_controls::{
    ControlAction, ControlError, ControlRequest, FINAL_SUMMARY_MARKER_PREFIX, FinalSummary,
    ItemStatus, PlanItem, RunState, RunStatus, WORK_PLAN_MARKER, WorkPlanComment, apply_control,
    approval_fingerprint, bind_approval_details, is_authorized, plan_is_approved,
};

#[test]
fn living_comment_renders_bounded_markdown_with_stable_marker_and_check_run_link() {
    let failed = PlanItem::new("verify", "Verify result", ItemStatus::Failed).unwrap();
    assert_eq!(failed.id(), "verify");
    let comment = WorkPlanComment::new(
        "https://github.com/acme/widgets/runs/42",
        vec![
            PlanItem::new("inspect", "Inspect <unsafe> input", ItemStatus::Completed).unwrap(),
            PlanItem::new("repair", "Repair **check**", ItemStatus::InProgress).unwrap(),
            failed,
        ],
    )
    .unwrap();

    let rendered = comment.render().unwrap();

    assert!(rendered.starts_with(WORK_PLAN_MARKER));
    assert!(rendered.contains("[Open Check Run](https://github.com/acme/widgets/runs/42)"));
    assert!(rendered.contains("`inspect` — completed — Inspect &lt;unsafe&gt; input"));
    assert!(rendered.contains(r"`repair` — in progress — Repair \*\*check\*\*"));
    assert!(rendered.contains("`verify` — failed — Verify result"));
    assert!(rendered.len() <= pr_controls::MAX_GITHUB_MARKDOWN_BYTES);
    assert_eq!(comment.render().unwrap(), rendered);
}

#[test]
fn approval_fingerprint_is_visible_and_invalidated_by_proposal_changes() {
    let plan = json!({
        "head_sha": "abc123",
        "proposed_patches": [{"path": "src/lib.rs", "patch": "@@ -1 +1 @@"}]
    });
    let details = bind_approval_details(
        &plan,
        json!({
            "summary": "Apply the typed patch after validation"
        }),
    );
    let hash = details["plan_hash"].as_str().expect("plan hash");
    assert_eq!(hash, approval_fingerprint(&plan, &details));

    let comment = WorkPlanComment::new(
        "https://github.com/acme/widgets/runs/42",
        vec![PlanItem::new("repair", "Apply typed patch", ItemStatus::Pending).unwrap()],
    )
    .unwrap()
    .with_approval_hash(hash)
    .unwrap();
    assert!(comment.render().unwrap().contains(&format!("`{hash}`")));

    let approved = json!({
        "head_sha": "abc123",
        "proposed_patches": [{"path": "src/lib.rs", "patch": "@@ -1 +1 @@"}],
        "approval_details": details,
        "approved": true,
        "approved_hash": hash,
    });
    assert!(plan_is_approved(&approved));

    let changed = json!({
        "head_sha": "abc123",
        "proposed_patches": [{"path": "src/main.rs", "patch": "@@ -1 +1 @@"}],
        "approval_details": approved["approval_details"].clone(),
        "approved": true,
        "approved_hash": hash,
    });
    assert!(!plan_is_approved(&changed));
}

#[test]
fn final_summary_has_run_specific_marker_and_required_sections() {
    let summary = FinalSummary::new(
        "run-42",
        vec!["Added deterministic controls"],
        vec!["cargo test --test pr_controls: passed"],
        vec!["Parent must export the module"],
    )
    .unwrap();

    let rendered = summary.render().unwrap();

    assert!(rendered.starts_with(&format!("{FINAL_SUMMARY_MARKER_PREFIX}run-42 -->")));
    assert!(rendered.contains("## Changes\n\n- Added deterministic controls"));
    assert!(rendered.contains("## Tests\n\n- cargo test --test pr_controls: passed"));
    assert!(rendered.contains("## Remaining blockers\n\n- Parent must export the module"));
    assert_eq!(summary.render().unwrap(), rendered);
}

#[test]
fn rendering_rejects_secret_like_text_and_invalid_urls() {
    assert!(PlanItem::new("leak", "github_pat_secret-value", ItemStatus::Pending).is_err());
    assert!(WorkPlanComment::new("javascript:alert(1)", vec![]).is_err());
}

#[test]
fn authorization_is_limited_to_pr_author_or_configured_maintainers() {
    let maintainers = vec!["release-captain".to_owned(), "Alice".to_owned()];

    assert!(is_authorized("pr-author", "PR-Author", &maintainers));
    assert!(is_authorized("alice", "someone-else", &maintainers));
    assert!(!is_authorized("contributor", "pr-author", &maintainers));
    assert!(!is_authorized("", "", &maintainers));
}

#[test]
fn control_parser_accepts_approval_and_run_controls_only() {
    assert_eq!(
        ControlAction::parse("approve-plan").unwrap(),
        ControlAction::ApprovePlan
    );
    assert_eq!(
        ControlAction::parse("skip-current-item").unwrap(),
        ControlAction::SkipCurrentItem
    );
    assert_eq!(
        ControlAction::parse("cancel-run").unwrap(),
        ControlAction::CancelRun
    );
    assert_eq!(
        ControlAction::parse("rerun-with-admin-token"),
        Err(ControlError::UnknownAction)
    );
}

#[test]
fn skip_marks_current_item_and_advances_without_mutating_original_state() {
    let state = RunState::new(vec![
        PlanItem::new("one", "First", ItemStatus::Pending).unwrap(),
        PlanItem::new("two", "Second", ItemStatus::Pending).unwrap(),
    ])
    .unwrap();
    let request =
        ControlRequest::new("delivery-1", "pr-author", ControlAction::SkipCurrentItem).unwrap();

    let updated = apply_control(&state, &request, "pr-author", &[]).unwrap();

    assert_eq!(state.items()[0].status(), ItemStatus::InProgress);
    assert_eq!(updated.items()[0].status(), ItemStatus::Skipped);
    assert_eq!(updated.items()[1].status(), ItemStatus::InProgress);
    assert_eq!(updated.status(), RunStatus::Running);
}

#[test]
fn cancel_is_terminal_and_duplicate_or_unauthorized_requests_are_rejected() {
    let state = RunState::new(vec![
        PlanItem::new("one", "First", ItemStatus::Pending).unwrap(),
        PlanItem::new("two", "Second", ItemStatus::Pending).unwrap(),
    ])
    .unwrap();
    let cancel = ControlRequest::new("delivery-2", "maintainer", ControlAction::CancelRun).unwrap();
    let maintainers = vec!["maintainer".to_owned()];

    let cancelled = apply_control(&state, &cancel, "pr-author", &maintainers).unwrap();

    assert_eq!(cancelled.status(), RunStatus::Cancelled);
    assert!(
        cancelled
            .items()
            .iter()
            .all(|item| item.status() == ItemStatus::Cancelled)
    );
    assert_eq!(
        apply_control(&cancelled, &cancel, "pr-author", &maintainers),
        Err(ControlError::DuplicateAction)
    );

    let unauthorized =
        ControlRequest::new("delivery-3", "stranger", ControlAction::SkipCurrentItem).unwrap();
    assert_eq!(
        apply_control(&state, &unauthorized, "pr-author", &maintainers),
        Err(ControlError::Unauthorized)
    );
}

#[test]
fn approval_moves_a_waiting_plan_into_running_state() {
    let state = RunState::waiting_approval(vec![
        PlanItem::new("one", "First", ItemStatus::Pending).unwrap(),
    ])
    .unwrap();
    let request =
        ControlRequest::new("delivery-approve", "pr-author", ControlAction::ApprovePlan).unwrap();

    let updated = apply_control(&state, &request, "pr-author", &[]).unwrap();

    assert_eq!(state.status(), RunStatus::WaitingApproval);
    assert_eq!(updated.status(), RunStatus::Running);
    assert_eq!(updated.items()[0].status(), ItemStatus::InProgress);
}

#[test]
fn skip_marks_the_planned_item_without_authorizing_remaining_work() {
    let state = RunState::waiting_approval(vec![
        PlanItem::new("one", "First", ItemStatus::Pending).unwrap(),
        PlanItem::new("two", "Second", ItemStatus::Pending).unwrap(),
    ])
    .unwrap();
    let request =
        ControlRequest::new("waiting-skip", "pr-author", ControlAction::SkipCurrentItem).unwrap();

    let updated = apply_control(&state, &request, "pr-author", &[]).unwrap();

    assert_eq!(state.status(), RunStatus::WaitingApproval);
    assert_eq!(updated.status(), RunStatus::WaitingApproval);
    assert_eq!(updated.items()[0].status(), ItemStatus::Skipped);
    assert_eq!(updated.items()[1].status(), ItemStatus::Pending);
}

#[test]
fn cancel_marks_all_pending_items_in_a_waiting_plan() {
    let state = RunState::waiting_approval(vec![
        PlanItem::new("one", "First", ItemStatus::Pending).unwrap(),
        PlanItem::new("two", "Second", ItemStatus::Pending).unwrap(),
    ])
    .unwrap();
    let request =
        ControlRequest::new("waiting-cancel", "pr-author", ControlAction::CancelRun).unwrap();

    let updated = apply_control(&state, &request, "pr-author", &[]).unwrap();

    assert_eq!(updated.status(), RunStatus::Cancelled);
    assert!(
        updated
            .items()
            .iter()
            .all(|item| item.status() == ItemStatus::Cancelled)
    );
}
