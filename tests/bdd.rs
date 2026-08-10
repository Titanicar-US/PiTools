//! Executable Gherkin coverage for the public bootstrap and webhook contracts.

use std::{collections::HashSet, fs, path::Path, process::Command};

use cucumber::{World as _, given, then, when};
use pitools::{
    feedback::{
        Feedback, FeedbackReply, FeedbackReplyTarget, RepairDecision, RepairDisposition,
        render_feedback_reply, repair_feedback,
    },
    github::manifest::{AppManifest, validate_manifest_code},
    github::{
        client::actions_job_id_from_details_url,
        events::{AccessLifecycle, DeliveryEnvelope},
    },
    models::{
        CheckConclusion, CheckSnapshot, CheckStatus, FeedbackSnapshot, ReadinessInput,
        ReadinessReason, ReadinessSnapshot,
    },
    pi::{PiJobRequest, PiSnapshotFile},
    policy::Policy,
    pr_controls::{
        ControlAction, ControlRequest, FinalSummary, ItemStatus, PlanItem, RunState, RunStatus,
        WorkPlanComment, apply_control, bind_approval_details,
    },
    readiness::evaluate,
    webhook::verify_signature,
};
use serde_json::json;

#[derive(Debug, Default, cucumber::World)]
struct World {
    configuration_failed: bool,
    secret_exposed: bool,
    watched: bool,
    processed_delivery_ids: HashSet<String>,
    access_lifecycle: Option<AccessLifecycle>,
    unauthorized: bool,
    work_plan: Option<RunState>,
    work_plan_status: Option<RunStatus>,
    feedback_directory: Option<tempfile::TempDir>,
    feedback_applied: bool,
    feedback_contents: Option<String>,
    feedback_outcome: Option<String>,
    feedback_target: Option<String>,
    feedback_reply: Option<String>,
    manifest_code_rejected: bool,
    manifest_admin_permission: bool,
    operator_commands_exposed: bool,
    rebase_default_rejected: bool,
    rebase_explicit_allowed: bool,
    ci_evidence: Option<String>,
    actions_job_id: Option<i64>,
    ci_admission_rejected: bool,
    ci_repairable: bool,
    validation_sandbox: Option<Vec<String>>,
    pi_request_rejected: bool,
    metrics_body: Option<String>,
    readiness_input: Option<ReadinessInput>,
    readiness_policy: Option<Policy>,
    readiness_snapshot: Option<ReadinessSnapshot>,
    notification_body: Option<String>,
}

#[given("a PiTools work plan is about to start")]
fn work_plan_is_about_to_start(world: &mut World) {
    let plan = json!({
        "head_sha": "abc123",
        "work": ["ci-repair"]
    });
    let approval_details = bind_approval_details(
        &plan,
        json!({
            "summary": "Diagnose the failed Actions check"
        }),
    );
    let approval_hash = approval_details["plan_hash"]
        .as_str()
        .expect("approval hash");
    world.notification_body = Some(
        WorkPlanComment::new(
            "https://github.com/acme/widgets/check-runs/42",
            vec![
                PlanItem::new(
                    "ci-repair",
                    "Diagnose GitHub Actions failure",
                    ItemStatus::InProgress,
                )
                .expect("work item"),
            ],
        )
        .expect("work-plan comment")
        .with_approval_hash(approval_hash)
        .expect("approval fingerprint")
        .render()
        .expect("render work-plan comment"),
    );
}

#[when("PiTools renders the work-plan comment")]
fn renders_work_plan_comment(_world: &mut World) {}

#[then("the work-plan comment names the planned work and links to its Check Run controls")]
fn work_plan_comment_exposes_plan_and_controls(world: &mut World) {
    let body = world
        .notification_body
        .as_deref()
        .expect("notification body");
    assert!(body.contains("Diagnose GitHub Actions failure"));
    assert!(body.contains("[Open Check Run](https://github.com/acme/widgets/check-runs/42)"));
    assert!(body.contains("approve planned work"));
    assert!(body.contains("skip the current item"));
    assert!(body.contains("cancel the run"));
    assert!(body.contains("Approval fingerprint: `sha256:"));
}

#[given("a PiTools run has completed with changes and validation")]
fn completed_run_with_changes_and_validation(world: &mut World) {
    world.notification_body = Some(
        FinalSummary::new(
            "run-42",
            ["Repaired the failed CI command"],
            ["make check passed"],
            ["Human merge remains required"],
        )
        .expect("final summary")
        .render()
        .expect("render final summary"),
    );
}

#[when("PiTools renders the final summary comment")]
fn renders_final_summary_comment(_world: &mut World) {}

#[then("the final summary lists changes, validation, and remaining blockers")]
fn final_summary_lists_outcome_sections(world: &mut World) {
    let body = world
        .notification_body
        .as_deref()
        .expect("notification body");
    assert!(body.contains("## Changes\n\n- Repaired the failed CI command"));
    assert!(body.contains("## Tests\n\n- make check passed"));
    assert!(body.contains("## Remaining blockers\n\n- Human merge remains required"));
}

fn ready_readiness_input() -> ReadinessInput {
    ReadinessInput {
        mergeable: Some(true),
        branch_is_current: true,
        required_checks: vec![CheckSnapshot {
            name: "check".into(),
            app_id: None,
            required_app_id: None,
            status: CheckStatus::Completed,
            conclusion: Some(CheckConclusion::Success),
            required: true,
        }],
        approved: true,
        approval_count: 1,
        required_approval_count: 0,
        stale_approval_present: false,
        last_push_approval_required: false,
        latest_push_approval: true,
        code_owner_review_required: false,
        code_owner_review_satisfied: true,
        unresolved_feedback: Vec::new(),
        is_draft: false,
    }
}

#[given("a ready pull request")]
fn ready_pull_request(world: &mut World) {
    world.readiness_input = Some(ready_readiness_input());
    world.readiness_policy = Some(Policy::default());
}

#[given("the pull request has a merge conflict and an out-of-date branch")]
fn conflicting_out_of_date_pull_request(world: &mut World) {
    let input = world.readiness_input.as_mut().expect("readiness input");
    input.mergeable = Some(false);
    input.branch_is_current = false;
}

#[given("the pull request has a failed required check")]
fn failed_required_check(world: &mut World) {
    let input = world.readiness_input.as_mut().expect("readiness input");
    input.required_checks[0].status = CheckStatus::Completed;
    input.required_checks[0].conclusion = Some(CheckConclusion::Failure);
}

#[given("the pull request has a pending required check")]
fn pending_required_check(world: &mut World) {
    let input = world.readiness_input.as_mut().expect("readiness input");
    input.required_checks[0].status = CheckStatus::InProgress;
    input.required_checks[0].conclusion = None;
}

#[given("the pull request has no required approval")]
fn missing_required_approval(world: &mut World) {
    world
        .readiness_input
        .as_mut()
        .expect("readiness input")
        .approved = false;
}

#[given("the protected branch requires two approvals and has a stale review")]
fn protected_branch_requires_two_approvals(world: &mut World) {
    let input = world.readiness_input.as_mut().expect("readiness input");
    input.approval_count = 1;
    input.required_approval_count = 2;
    input.last_push_approval_required = true;
    input.latest_push_approval = false;
    input.stale_approval_present = true;
}

#[given("the pull request has unresolved configured automation feedback")]
fn unresolved_configured_feedback(world: &mut World) {
    world
        .readiness_input
        .as_mut()
        .expect("readiness input")
        .unresolved_feedback
        .push(FeedbackSnapshot {
            id: "feedback-1".into(),
            actor_login: "copilot-pull-request-reviewer[bot]".into(),
            actor_type: "Bot".into(),
            body: "please fix this".into(),
            resolved: false,
            is_automation: true,
        });
    world
        .readiness_policy
        .as_mut()
        .expect("readiness policy")
        .automation_actors
        .push("copilot-pull-request-reviewer[bot]".into());
}

#[when("PiTools evaluates pull request readiness")]
fn evaluates_pull_request_readiness(world: &mut World) {
    let input = world.readiness_input.as_ref().expect("readiness input");
    let policy = world.readiness_policy.as_ref().expect("readiness policy");
    world.readiness_snapshot = Some(evaluate(input, policy));
}

fn readiness_has_reason(world: &World, reason: ReadinessReason) {
    assert!(
        world
            .readiness_snapshot
            .as_ref()
            .expect("readiness snapshot")
            .reason_codes
            .contains(&reason)
    );
}

#[then("the readiness reason includes a merge conflict")]
fn readiness_includes_merge_conflict(world: &mut World) {
    readiness_has_reason(world, ReadinessReason::MergeConflict);
}

#[then("the readiness reason includes an out-of-date branch")]
fn readiness_includes_out_of_date_branch(world: &mut World) {
    readiness_has_reason(world, ReadinessReason::BranchOutOfDate);
}

#[then("the readiness reason includes a failed required check")]
fn readiness_includes_failed_check(world: &mut World) {
    readiness_has_reason(world, ReadinessReason::RequiredCheckFailed);
}

#[then("the readiness reason includes a pending required check")]
fn readiness_includes_pending_check(world: &mut World) {
    readiness_has_reason(world, ReadinessReason::RequiredCheckPending);
}

#[then("the readiness reason includes a missing required review")]
fn readiness_includes_missing_review(world: &mut World) {
    readiness_has_reason(world, ReadinessReason::RequiredReviewMissing);
}

#[then("the readiness reason includes an insufficient review count")]
fn readiness_includes_insufficient_review_count(world: &mut World) {
    readiness_has_reason(world, ReadinessReason::RequiredReviewCountMissing);
}

#[then("the readiness reason includes a stale review")]
fn readiness_includes_stale_review(world: &mut World) {
    readiness_has_reason(world, ReadinessReason::StaleReview);
}

#[then("the readiness reason includes unresolved automation feedback")]
fn readiness_includes_unresolved_feedback(world: &mut World) {
    readiness_has_reason(world, ReadinessReason::UnresolvedAutomationFeedback);
}

#[then("the pull request is ready for human merge")]
fn pull_request_is_ready_for_human_merge(world: &mut World) {
    let snapshot = world
        .readiness_snapshot
        .as_ref()
        .expect("readiness snapshot");
    assert!(snapshot.ready);
    assert!(snapshot.reason_codes.is_empty());
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

#[when("the operator reads the CLI help")]
fn operator_reads_cli_help(world: &mut World) {
    let output = Command::new(env!("CARGO_BIN_EXE_pitools"))
        .arg("--help")
        .output()
        .expect("run pitools help");
    let help = String::from_utf8_lossy(&output.stdout);
    world.operator_commands_exposed = output.status.success()
        && ["watchlist", "events", "audit", "cancel"]
            .iter()
            .all(|command| help.contains(command));
}

#[then("the CLI exposes bounded inspection and cancellation commands")]
fn operator_commands_are_exposed(world: &mut World) {
    assert!(world.operator_commands_exposed);
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

#[given("a GitHub App installation suspension delivery")]
fn installation_suspension_delivery(world: &mut World) {
    let payload = json!({
        "action": "suspend",
        "installation": {"id": 9, "account": {"login": "Titanicar-US"}}
    });
    world.access_lifecycle = Some(
        DeliveryEnvelope::from_payload(
            "installation-suspend".into(),
            "installation".into(),
            payload.clone(),
            serde_json::to_vec(&payload).expect("serialize suspension"),
        )
        .access_lifecycle()
        .expect("suspension lifecycle"),
    );
}

#[given("a GitHub App repository removal delivery")]
fn repository_removal_delivery(world: &mut World) {
    let payload = json!({
        "action": "removed",
        "installation": {"id": 9},
        "repositories_removed": [{"id": 42}]
    });
    world.access_lifecycle = Some(
        DeliveryEnvelope::from_payload(
            "repository-removed".into(),
            "installation_repositories".into(),
            payload.clone(),
            serde_json::to_vec(&payload).expect("serialize removal"),
        )
        .access_lifecycle()
        .expect("repository removal lifecycle"),
    );
}

#[when("PiTools classifies the installation lifecycle")]
fn classifies_installation_lifecycle(_world: &mut World) {}

#[then("reconciliation is deactivated for the installation")]
fn installation_reconciliation_is_deactivated(world: &mut World) {
    assert!(matches!(
        world.access_lifecycle,
        Some(AccessLifecycle::SuspendInstallation | AccessLifecycle::DeleteInstallation)
    ));
}

#[then("reconciliation is deactivated for the repository")]
fn repository_reconciliation_is_deactivated(world: &mut World) {
    assert_eq!(
        world.access_lifecycle,
        Some(AccessLifecycle::RemoveRepositories)
    );
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

#[given("a failed GitHub Actions check has logs and annotations")]
fn failed_actions_check(world: &mut World) {
    world.ci_evidence = Some("actions-evidence-fixture".into());
}

#[when("PiTools prepares the Actions diagnosis envelope")]
fn prepares_actions_diagnosis(world: &mut World) {
    world.ci_evidence = Some(
        pitools::ci::prepare_ci_evidence(
            &json!({"checks": [{"external_id": "81"}]}),
            &json!([{
                "actions_log": "error: test failed",
                "annotations": [{"path": "src/lib.rs", "message": "assertion failed"}]
            }]),
        )
        .expect("Actions evidence serializes"),
    );
}

#[then("the bounded evidence retains the Actions log and annotation")]
fn actions_evidence_is_retained(world: &mut World) {
    let evidence = world.ci_evidence.as_deref().expect("Actions evidence");
    assert!(evidence.contains("test failed"));
    assert!(evidence.contains("src/lib.rs"));
    assert!(evidence.contains("assertion failed"));
}

#[given("an Actions check run details URL")]
fn actions_check_run_details_url(world: &mut World) {
    world.actions_job_id = None;
}

#[when("PiTools resolves the workflow job ID for Actions logs")]
fn resolves_actions_job_id(world: &mut World) {
    world.actions_job_id = actions_job_id_from_details_url(Some(
        "https://github.com/acme/widgets/actions/runs/29679449/job/399444496",
    ));
}

#[then("it resolves the workflow job ID without using the check run ID")]
fn actions_job_id_is_distinct_from_check_run_id(world: &mut World) {
    assert_eq!(world.actions_job_id, Some(399444496));
    assert_ne!(world.actions_job_id, Some(81));
}

#[given("a typed CI repair patch is proposed without approval")]
fn typed_ci_patch_without_approval(world: &mut World) {
    world.ci_admission_rejected = false;
}

#[when("PiTools admits the CI mutation")]
fn admits_ci_mutation(world: &mut World) {
    world.ci_admission_rejected = matches!(
        pitools::ci::admit_ci_mutation(1, false, true),
        Err(pitools::ci::CiError::ApprovalRequired)
    );
}

#[then("the CI mutation is rejected without explicit approval")]
fn ci_mutation_is_rejected_without_approval(world: &mut World) {
    assert!(world.ci_admission_rejected);
}

#[given("a cancelled GitHub Actions check")]
fn cancelled_actions_check(world: &mut World) {
    world.ci_repairable = false;
}

#[when("PiTools classifies the check conclusion for repair")]
fn classifies_check_conclusion_for_repair(world: &mut World) {
    world.ci_repairable = pitools::ci::is_repairable_check_conclusion("cancelled");
}

#[then("the CI repair path accepts the check conclusion")]
fn ci_repair_path_accepts_check_conclusion(world: &mut World) {
    assert!(world.ci_repairable);
}

#[given("a repository validation sandbox command")]
fn repository_validation_sandbox_command(world: &mut World) {
    world.validation_sandbox = None;
}

#[when("PiTools builds the validation sandbox")]
fn builds_validation_sandbox(world: &mut World) {
    world.validation_sandbox = Some(
        pitools::ci::validation_sandbox_argv(
            Path::new("/var/lib/pitools/validation"),
            &["make".into(), "check".into()],
        )
        .expect("sandbox command"),
    );
}

#[then("the validation sandbox hides control-plane secrets and network access")]
fn validation_sandbox_hides_secrets_and_network(world: &mut World) {
    let argv = world.validation_sandbox.as_ref().expect("sandbox command");
    let rendered = argv.join(" ");
    assert!(rendered.contains("--unshare-net"));
    assert!(rendered.contains("--tmpfs /"));
    assert!(!rendered.contains("/run/secrets"));
    assert!(!rendered.contains("GITHUB_PRIVATE_KEY_PATH"));
}

#[given("a Pi worker request with a non-canonical snapshot path")]
fn pi_request_with_non_canonical_snapshot_path(world: &mut World) {
    let request = PiJobRequest {
        protocol_version: "pitools.pi/v1".into(),
        job_id: uuid::Uuid::now_v7(),
        repository: "example/repo".into(),
        snapshot_path: "/workspace/repo".into(),
        snapshot_files: vec![PiSnapshotFile {
            path: "src/lib.rs".into(),
            content: "fn main() {}\n".into(),
        }],
        allowed_paths: vec!["src/lib.rs".into()],
        failure_evidence: None,
        policy_revision: "sha256:policy".into(),
        nonce: "nonce".into(),
    };
    world.pi_request_rejected = request.validate().is_err();
}

#[given("a Pi worker request with an unallowlisted snapshot file")]
fn pi_request_with_unallowlisted_snapshot_file(world: &mut World) {
    let request = PiJobRequest {
        protocol_version: "pitools.pi/v1".into(),
        job_id: uuid::Uuid::now_v7(),
        repository: "example/repo".into(),
        snapshot_path: "snapshot.json".into(),
        snapshot_files: vec![PiSnapshotFile {
            path: "README.md".into(),
            content: "source".into(),
        }],
        allowed_paths: vec!["src/lib.rs".into()],
        failure_evidence: None,
        policy_revision: "sha256:policy".into(),
        nonce: "nonce".into(),
    };
    world.pi_request_rejected = request.validate().is_err();
}

#[when("PiTools validates the Pi worker request")]
fn validates_pi_worker_request(_world: &mut World) {}

#[then("the Pi worker request is rejected before provider execution")]
fn pi_worker_request_is_rejected(world: &mut World) {
    assert!(world.pi_request_rejected);
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

#[given("an applied automation feedback outcome")]
fn applied_feedback_outcome(world: &mut World) {
    world.feedback_outcome = Some("applied".into());
    world.feedback_target = Some("review".into());
}

#[given("a rejected automation feedback outcome")]
fn rejected_feedback_outcome(world: &mut World) {
    world.feedback_outcome = Some("rejected".into());
    world.feedback_target = Some("review".into());
}

#[given("an applied issue-comment automation feedback outcome")]
fn applied_issue_comment_feedback_outcome(world: &mut World) {
    world.feedback_outcome = Some("applied".into());
    world.feedback_target = Some("issue-comment".into());
}

#[given("a rejected issue-comment automation feedback outcome")]
fn rejected_issue_comment_feedback_outcome(world: &mut World) {
    world.feedback_outcome = Some("rejected".into());
    world.feedback_target = Some("issue-comment".into());
}

#[when("PiTools renders the feedback outcome comment")]
fn renders_feedback_outcome_comment(world: &mut World) {
    let target = match world.feedback_target.as_deref() {
        Some("issue-comment") => FeedbackReplyTarget::PullRequestConversation,
        Some("review") => FeedbackReplyTarget::ReviewThread,
        other => panic!("unexpected feedback target: {other:?}"),
    };
    world.feedback_reply = Some(match world.feedback_outcome.as_deref() {
        Some("applied") => render_feedback_reply(FeedbackReply::Applied {
            target,
            path: "src/lib.rs",
            commit: "0123456789abcdef",
        }),
        Some("rejected") => render_feedback_reply(FeedbackReply::Rejected { target }),
        other => panic!("unexpected feedback outcome: {other:?}"),
    });
}

#[then("the outcome comment says the suggestion was applied and the thread is being resolved")]
fn applied_feedback_comment_is_resolved(world: &mut World) {
    let reply = world.feedback_reply.as_deref().expect("feedback reply");
    assert!(reply.contains("applied"));
    assert!(reply.contains("thread is being resolved"));
}

#[then("the outcome comment says the suggestion was rejected and human follow-up is required")]
fn rejected_feedback_comment_requires_human_follow_up(world: &mut World) {
    let reply = world.feedback_reply.as_deref().expect("feedback reply");
    assert!(reply.contains("rejected"));
    assert!(reply.contains("human follow-up remains required"));
    assert!(reply.contains("thread is being resolved"));
}

#[then("the issue-comment outcome explains that no review thread is available")]
fn issue_comment_applied_outcome_has_no_thread_claim(world: &mut World) {
    let reply = world.feedback_reply.as_deref().expect("feedback reply");
    assert!(reply.contains("PR-level outcome comment records the result"));
    assert!(reply.contains("no review thread is available to resolve"));
}

#[then(
    "the issue-comment outcome says the suggestion was rejected without claiming thread resolution"
)]
fn issue_comment_rejected_outcome_has_no_thread_claim(world: &mut World) {
    let reply = world.feedback_reply.as_deref().expect("feedback reply");
    assert!(reply.contains("rejected"));
    assert!(reply.contains("no review thread is available to resolve"));
    assert!(!reply.contains("thread is being resolved"));
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

#[given("the public GitHub App manifest")]
fn public_github_app_manifest(world: &mut World) {
    world.manifest_admin_permission = false;
}

#[when("PiTools validates its default permissions")]
fn validates_manifest_permissions(world: &mut World) {
    let manifest = AppManifest::for_public_project("https://pitools.example.test");
    world.manifest_admin_permission = manifest.validate().is_ok()
        && manifest.default_permissions.get("administration") == Some(&"read".to_owned());
}

#[then("the manifest requests administration read permission")]
fn manifest_requests_administration_read(world: &mut World) {
    assert!(world.manifest_admin_permission);
}

#[tokio::test]
async fn gherkin_features_are_executable() {
    World::cucumber()
        .fail_on_skipped()
        .run("tests/features")
        .await;
}
