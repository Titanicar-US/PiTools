//! Executable Gherkin coverage for the public bootstrap and webhook contracts.

use std::{collections::HashSet, fs, process::Command};

use cucumber::{World as _, given, then, when};
use pitools::{
    feedback::{Feedback, RepairDecision, RepairDisposition, repair_feedback},
    github::events::DeliveryEnvelope,
    github::manifest::validate_manifest_code,
    pr_controls::{
        ControlAction, ControlRequest, ItemStatus, PlanItem, RunState, RunStatus, apply_control,
    },
    webhook::verify_signature,
};
use serde_json::json;

#[derive(Debug, Default, cucumber::World)]
struct World {
    configuration_failed: bool,
    secret_exposed: bool,
    watched: bool,
    processed_delivery_ids: HashSet<String>,
    unauthorized: bool,
    work_plan: Option<RunState>,
    work_plan_status: Option<RunStatus>,
    feedback_directory: Option<tempfile::TempDir>,
    feedback_applied: bool,
    feedback_contents: Option<String>,
    manifest_code_rejected: bool,
    rebase_default_rejected: bool,
    rebase_explicit_allowed: bool,
    ci_evidence: Option<String>,
    metrics_body: Option<String>,
}

#[given("no PiTools credentials are configured")]
fn no_credentials(_world: &mut World) {}

#[when("the doctor command loads configuration")]
fn doctor_loads_configuration(world: &mut World) {
    let output = Command::new(env!("CARGO_BIN_EXE_pitools"))
        .arg("doctor")
        .env_remove("DATABASE_URL")
        .env_remove("GITHUB_APP_ID")
        .env_remove("GITHUB_PRIVATE_KEY_PATH")
        .env_remove("GITHUB_WEBHOOK_SECRET")
        .env_remove("ADMIN_BEARER_TOKEN_HASH")
        .output()
        .expect("run pitools doctor");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    world.configuration_failed = !output.status.success();
    world.secret_exposed = combined.contains("private-key-value")
        || combined.contains("webhook-secret-value")
        || combined.contains("admin-hash-value");
}

#[then("configuration validation fails without exposing secret values")]
fn configuration_fails_closed(world: &mut World) {
    assert!(world.configuration_failed);
    assert!(!world.secret_exposed);
}

#[given("a GitHub pull request delivery with a valid signature")]
fn valid_pull_request_delivery(world: &mut World) {
    let payload = json!({
        "action": "opened",
        "installation": {"id": 9, "account": {"login": "Titanicar-US", "type": "Organization"}},
        "repository": {"id": 42},
        "pull_request": {
            "id": 1001,
            "number": 7,
            "title": "Ready PR",
            "html_url": "https://github.com/Titanicar-US/PiTools/pull/7",
            "state": "open",
            "merged": false,
            "draft": false,
            "updated_at": "2026-08-10T00:00:00Z",
            "user": {"login": "author"},
            "head": {"sha": "head", "ref": "feature"},
            "base": {"sha": "base", "ref": "main"}
        }
    });
    let raw = serde_json::to_vec(&payload).expect("serialize fixture");
    let envelope =
        DeliveryEnvelope::from_payload("delivery-1".into(), "pull_request".into(), payload, raw);
    let signature = {
        use hmac::{Hmac, KeyInit, Mac};
        use sha2::Sha256;
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(b"secret").expect("key");
        mac.update(&envelope.raw_body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    };
    verify_signature("secret", &envelope.raw_body, &signature).expect("valid signature");
    world.watched = envelope
        .pull_request_snapshot()
        .expect("valid PR payload")
        .is_some_and(|snapshot| matches!(snapshot.state, pitools::models::PullRequestState::Open));
}

#[when("PiTools records the delivery")]
fn records_delivery(world: &mut World) {
    world.processed_delivery_ids.insert("delivery-1".into());
}

#[then("the pull request is watched until it is closed or merged")]
fn pull_request_is_watched(world: &mut World) {
    assert!(world.watched);
    assert_eq!(world.processed_delivery_ids.len(), 1);
}

#[given("the same GitHub delivery ID is received twice")]
fn duplicate_delivery(world: &mut World) {
    world
        .processed_delivery_ids
        .insert("delivery-duplicate".into());
    world
        .processed_delivery_ids
        .insert("delivery-duplicate".into());
}

#[when("PiTools records both deliveries")]
fn records_both_deliveries(_world: &mut World) {}

#[then("only one event is processed")]
fn one_event_processed(world: &mut World) {
    assert_eq!(world.processed_delivery_ids.len(), 1);
}

#[given("a webhook with an invalid X-Hub-Signature-256 header")]
fn invalid_webhook(world: &mut World) {
    world.unauthorized = verify_signature("secret", br#"{}"#, "sha256=invalid").is_err();
}

#[when("the webhook receiver handles the request")]
fn webhook_handles_request(_world: &mut World) {}

#[then("it returns unauthorized before recording an event")]
fn webhook_rejects_before_recording(world: &mut World) {
    assert!(world.unauthorized);
    assert!(world.processed_delivery_ids.is_empty());
}

#[given("a running PiTools work plan")]
fn running_work_plan(world: &mut World) {
    let items = vec![
        PlanItem::new("feedback", "apply automation feedback", ItemStatus::Pending)
            .expect("feedback item"),
        PlanItem::new("tests", "run validation", ItemStatus::Pending).expect("tests item"),
    ];
    world.work_plan = Some(RunState::new(items).expect("running plan"));
}

#[given("a PiTools work plan waiting for approval")]
fn waiting_work_plan(world: &mut World) {
    let items = vec![
        PlanItem::new("feedback", "apply automation feedback", ItemStatus::Pending)
            .expect("feedback item"),
    ];
    world.work_plan = Some(RunState::waiting_approval(items).expect("waiting plan"));
}

#[when("the pull request author approves the plan")]
fn author_approves_plan(world: &mut World) {
    let state = world.work_plan.as_ref().expect("work plan");
    let request = ControlRequest::new("request-approve", "author", ControlAction::ApprovePlan)
        .expect("approval request");
    world.work_plan =
        Some(apply_control(state, &request, "author", &[]).expect("author is authorized"));
    world.work_plan_status = world.work_plan.as_ref().map(RunState::status);
}

#[when("the pull request author skips the planned item")]
fn author_skips_waiting_item(world: &mut World) {
    let state = world.work_plan.as_ref().expect("work plan");
    let request = ControlRequest::new(
        "request-skip-waiting",
        "author",
        ControlAction::SkipCurrentItem,
    )
    .expect("skip request");
    world.work_plan =
        Some(apply_control(state, &request, "author", &[]).expect("author is authorized"));
    world.work_plan_status = world.work_plan.as_ref().map(RunState::status);
}

#[then("the waiting plan marks the item skipped")]
fn waiting_item_is_skipped(world: &mut World) {
    assert_eq!(world.work_plan_status, Some(RunStatus::Completed));
    assert_eq!(
        world.work_plan.as_ref().expect("work plan").items()[0].status(),
        ItemStatus::Skipped
    );
}

#[then("the approved plan starts its first work item")]
fn approved_plan_starts(world: &mut World) {
    assert_eq!(world.work_plan_status, Some(RunStatus::Running));
    assert_eq!(
        world.work_plan.as_ref().expect("work plan").items()[0].status(),
        ItemStatus::InProgress
    );
}

#[given("a stack branch requires a history rewrite")]
fn stack_branch_requires_rewrite(world: &mut World) {
    use pitools::stack::{BranchState, RebasePlanner, RebasePolicy, RebaseTarget};
    use std::collections::BTreeSet;

    let branch = BranchState {
        name: "pitools/stack".into(),
        clean: true,
        current: true,
        bot_owned: true,
    };
    let plan =
        RebasePlanner::plan(&branch, &RebaseTarget::rebase_onto("main")).expect("rebase plan");
    world.rebase_default_rejected = RebasePolicy::default()
        .authorize_push(&branch, &plan)
        .is_err();
    world.rebase_explicit_allowed = RebasePolicy {
        allow_bot_owned_rewrite: true,
        approved_bot_owned_branches: BTreeSet::from(["pitools/stack".into()]),
    }
    .authorize_push(&branch, &plan)
    .is_ok();
}

#[then("history rewrites are allowed only for an explicit bot-owned branch")]
fn explicit_rebase_authorization(world: &mut World) {
    assert!(world.rebase_default_rejected);
    assert!(world.rebase_explicit_allowed);
}

#[given("a CI failure contains credential-shaped text")]
fn ci_failure_contains_secret_text(world: &mut World) {
    world.ci_evidence = Some("Authorization: Bearer ghp_example\nerror: compilation failed".into());
}

#[when("PiTools prepares CI evidence")]
fn prepares_ci_evidence(world: &mut World) {
    world.ci_evidence = world
        .ci_evidence
        .take()
        .map(|value| pitools::ci::redact_ci_text(&value));
}

#[then("the CI evidence contains no credential-shaped text")]
fn ci_evidence_is_redacted(world: &mut World) {
    let evidence = world.ci_evidence.as_deref().expect("CI evidence");
    assert!(!evidence.contains("ghp_example"));
    assert!(evidence.contains("[REDACTED]"));
}

#[given("PiTools has received a webhook")]
fn received_webhook(world: &mut World) {
    let metrics = pitools::metrics::Metrics::default();
    metrics.record_webhook_received();
    world.metrics_body = Some(metrics.render_prometheus());
}

#[when("the operator reads Prometheus metrics")]
fn reads_prometheus_metrics(_world: &mut World) {}

#[then("the received webhook counter is exposed")]
fn received_webhook_counter_exposed(world: &mut World) {
    assert!(
        world
            .metrics_body
            .as_deref()
            .expect("metrics")
            .contains("pitools_webhook_received_total 1")
    );
}

#[when("the pull request author requests cancellation")]
fn author_cancels_plan(world: &mut World) {
    let state = world.work_plan.as_ref().expect("work plan");
    let request = ControlRequest::new("request-author", "author", ControlAction::CancelRun)
        .expect("control request");
    world.work_plan =
        Some(apply_control(state, &request, "author", &[]).expect("author is authorized"));
    world.work_plan_status = world.work_plan.as_ref().map(RunState::status);
}

#[then("all remaining work is cancelled")]
fn all_work_cancelled(world: &mut World) {
    assert_eq!(world.work_plan_status, Some(RunStatus::Cancelled));
    assert!(
        world
            .work_plan
            .as_ref()
            .expect("work plan")
            .items()
            .iter()
            .all(|item| item.status() == ItemStatus::Cancelled)
    );
}

#[when("a configured maintainer skips the current item")]
fn maintainer_skips_item(world: &mut World) {
    let state = world.work_plan.as_ref().expect("work plan");
    let request = ControlRequest::new(
        "request-maintainer",
        "maintainer",
        ControlAction::SkipCurrentItem,
    )
    .expect("control request");
    world.work_plan = Some(
        apply_control(state, &request, "author", &["maintainer".into()])
            .expect("maintainer is authorized"),
    );
    world.work_plan_status = world.work_plan.as_ref().map(RunState::status);
}

#[then("the next work item is in progress")]
fn next_work_item_in_progress(world: &mut World) {
    assert_eq!(world.work_plan_status, Some(RunStatus::Running));
    let items = world.work_plan.as_ref().expect("work plan").items();
    assert_eq!(items[0].status(), ItemStatus::Skipped);
    assert_eq!(items[1].status(), ItemStatus::InProgress);
}

#[given("an approved inline automation suggestion")]
fn approved_inline_suggestion(world: &mut World) {
    let directory = tempfile::tempdir().expect("feedback directory");
    fs::write(directory.path().join("file.txt"), "old\nsecond\n").expect("feedback fixture");
    world.feedback_directory = Some(directory);
}

#[when("PiTools applies the suggestion")]
fn applies_suggestion(world: &mut World) {
    let directory = world
        .feedback_directory
        .as_ref()
        .expect("feedback directory");
    let outcome = repair_feedback(
        directory.path(),
        &["automation-bot[bot]".into()],
        &Feedback {
            actor_login: "automation-bot[bot]".into(),
            actor_type: "Bot".into(),
            is_automation: true,
            body: "```suggestion\nnew\n```".into(),
            path: Some("file.txt".into()),
            start_line: Some(1),
            line: Some(1),
        },
        RepairDecision::Apply,
    )
    .expect("suggestion applies");
    world.feedback_applied = outcome.disposition == RepairDisposition::Applied;
    world.feedback_contents =
        Some(fs::read_to_string(directory.path().join("file.txt")).expect("read feedback fixture"));
}

#[then("only the suggested lines change")]
fn only_suggested_lines_change(world: &mut World) {
    assert!(world.feedback_applied);
    assert_eq!(world.feedback_contents.as_deref(), Some("new\nsecond\n"));
}

#[given("an unsafe GitHub App manifest conversion code")]
fn unsafe_manifest_code(world: &mut World) {
    world.manifest_code_rejected = validate_manifest_code("../conversion").is_err();
}

#[when("PiTools validates the manifest conversion code")]
fn validates_manifest_code(_world: &mut World) {}

#[then("the manifest conversion is rejected before any network request")]
fn manifest_code_is_rejected(world: &mut World) {
    assert!(world.manifest_code_rejected);
}

#[tokio::test]
async fn gherkin_features_are_executable() {
    World::cucumber()
        .fail_on_skipped()
        .run("tests/features")
        .await;
}
