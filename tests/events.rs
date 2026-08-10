use pitools::github::events::DeliveryEnvelope;
use serde_json::json;

#[test]
fn pull_request_delivery_extracts_an_ordered_snapshot() {
    let payload = json!({
        "installation": {"id": 1},
        "repository": {"id": 2},
        "pull_request": {
            "id": 3,
            "number": 4,
            "title": "PR",
            "html_url": "https://github.com/acme/repo/pull/4",
            "state": "open",
            "merged": false,
            "draft": false,
            "updated_at": "2026-08-10T01:02:03Z",
            "user": {"login": "author"},
            "head": {"sha": "head", "ref": "feature"},
            "base": {"sha": "base", "ref": "main"}
        }
    });
    let envelope = DeliveryEnvelope::from_payload(
        "delivery".into(),
        "pull_request".into(),
        payload.clone(),
        serde_json::to_vec(&payload).unwrap(),
    );
    let snapshot = envelope
        .pull_request_snapshot()
        .unwrap()
        .expect("PR snapshot");
    assert_eq!(snapshot.repository_id, 2);
    assert_eq!(snapshot.number, 4);
    assert_eq!(snapshot.head_sha, "head");
}

#[test]
fn feedback_and_check_events_are_normalized_without_trusting_automation() {
    let feedback_payload = json!({
        "action": "created",
        "installation": {"id": 1},
        "repository": {"id": 2},
        "issue": {"number": 4, "pull_request": {"url": "https://api.github.com/pr"}},
        "comment": {"id": 8, "body": "fix this", "path": "src/lib.rs", "start_line": 4, "line": 5, "user": {"login": "bot", "type": "Bot"}}
    });
    let feedback = DeliveryEnvelope::from_payload(
        "feedback".into(),
        "issue_comment".into(),
        feedback_payload.clone(),
        serde_json::to_vec(&feedback_payload).unwrap(),
    )
    .feedback_record()
    .unwrap()
    .expect("feedback");
    assert_eq!(feedback.id, "issue-comment:8");
    assert!(feedback.is_automation);
    assert_eq!(feedback.path.as_deref(), Some("src/lib.rs"));
    assert_eq!(feedback.start_line, Some(4));
    assert_eq!(feedback.line, Some(5));

    let check_payload = json!({
        "action": "completed",
        "installation": {"id": 1},
        "repository": {"id": 2},
        "check_run": {"id": 9, "name": "tests", "status": "completed", "conclusion": "failure", "details_url": "https://github.com/acme/repo/actions/runs/9", "pull_requests": [{"number": 4}]}
    });
    let check = DeliveryEnvelope::from_payload(
        "check".into(),
        "check_run".into(),
        check_payload.clone(),
        serde_json::to_vec(&check_payload).unwrap(),
    )
    .check_record()
    .unwrap()
    .expect("check");
    assert_eq!(check.external_id, "9");
    assert_eq!(check.conclusion.as_deref(), Some("failure"));
}

#[test]
fn check_run_requested_action_extracts_the_actor_and_action() {
    let payload = json!({
        "action": "requested_action",
        "installation": {"id": 1},
        "repository": {"id": 2},
        "check_run": {"id": 9},
        "requested_action": {"identifier": "cancel-run"},
        "sender": {"login": "maintainer"}
    });
    let envelope = DeliveryEnvelope::from_payload(
        "control".into(),
        "check_run".into(),
        payload.clone(),
        serde_json::to_vec(&payload).unwrap(),
    );
    let control = envelope.check_run_control().unwrap().expect("control");
    assert_eq!(control.action, "cancel-run");
    assert_eq!(control.actor_login, "maintainer");
}

#[test]
fn status_deliveries_expose_the_commit_for_watchlist_matching() {
    let payload = json!({
        "installation": {"id": 1},
        "repository": {"id": 2},
        "sha": "head-sha"
    });
    let envelope = DeliveryEnvelope::from_payload(
        "status".into(),
        "status".into(),
        payload.clone(),
        serde_json::to_vec(&payload).unwrap(),
    );
    assert_eq!(envelope.head_sha(), Some("head-sha"));
}
