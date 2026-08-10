use pitools::{models::*, policy::Policy, readiness::evaluate};

fn ready_input() -> ReadinessInput {
    ReadinessInput {
        mergeable: Some(true),
        branch_is_current: true,
        required_checks: vec![CheckSnapshot {
            name: "check".into(),
            status: CheckStatus::Completed,
            conclusion: Some(CheckConclusion::Success),
            required: true,
        }],
        approved: true,
        unresolved_feedback: Vec::new(),
        is_draft: false,
    }
}

#[test]
fn readiness_is_true_when_all_policy_gates_pass() {
    let snapshot = evaluate(&ready_input(), &Policy::default());

    assert!(snapshot.ready);
    assert!(snapshot.reason_codes.is_empty());
}

#[test]
fn readiness_explains_each_blocking_gate_and_deduplicates_reasons() {
    let mut input = ready_input();
    input.mergeable = Some(false);
    input.branch_is_current = false;
    input.approved = false;
    input.required_checks[0].status = CheckStatus::InProgress;
    input.unresolved_feedback.push(FeedbackSnapshot {
        id: "feedback-1".into(),
        actor_login: "copilot-pull-request-reviewer[bot]".into(),
        actor_type: "Bot".into(),
        body: "fix this".into(),
        resolved: false,
        is_automation: true,
    });

    let mut policy = Policy::default();
    policy
        .automation_actors
        .push("copilot-pull-request-reviewer[bot]".into());
    let snapshot = evaluate(&input, &policy);

    assert!(!snapshot.ready);
    assert!(
        snapshot
            .reason_codes
            .contains(&ReadinessReason::MergeConflict)
    );
    assert!(
        snapshot
            .reason_codes
            .contains(&ReadinessReason::BranchOutOfDate)
    );
    assert!(
        snapshot
            .reason_codes
            .contains(&ReadinessReason::RequiredCheckPending)
    );
    assert!(
        snapshot
            .reason_codes
            .contains(&ReadinessReason::RequiredReviewMissing)
    );
    assert!(
        snapshot
            .reason_codes
            .contains(&ReadinessReason::UnresolvedAutomationFeedback)
    );
    let mut deduped = snapshot.reason_codes.clone();
    deduped.sort_by_key(|reason| format!("{reason:?}"));
    deduped.dedup();
    assert_eq!(deduped, snapshot.reason_codes);
}

#[test]
fn configured_required_checks_are_not_inferred_from_github_metadata() {
    let policy = Policy {
        required_checks: vec!["required-check".into()],
        ..Policy::default()
    };

    let snapshot = evaluate(&ready_input(), &policy);

    assert!(!snapshot.ready);
    assert!(
        snapshot
            .reason_codes
            .contains(&ReadinessReason::RequiredCheckMissing)
    );
}
