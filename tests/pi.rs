use pitools::pi::{PiJobRequest, PiWorker};
use std::time::Duration;
use uuid::Uuid;

fn request() -> PiJobRequest {
    PiJobRequest {
        protocol_version: "pitools.pi/v1".into(),
        job_id: Uuid::now_v7(),
        repository: "acme/repo".into(),
        snapshot_path: "snapshot.json".into(),
        allowed_paths: vec!["src/lib.rs".into()],
        failure_evidence: Some("test failed".into()),
        policy_revision: "sha256:policy".into(),
        nonce: "nonce".into(),
    }
}

#[test]
fn pi_request_rejects_absolute_and_traversal_paths() {
    let mut value = request();
    value.snapshot_path = "/etc/passwd".into();
    assert!(value.validate().is_err());
    value.snapshot_path = "snapshot.json".into();
    value.allowed_paths = vec!["../secret".into()];
    assert!(value.validate().is_err());
}

#[tokio::test]
async fn pi_worker_rejects_shell_wrappers_before_starting_them() {
    let worker = PiWorker {
        command: vec!["sh".into(), "-c".into(), "cat".into()],
        timeout: Duration::from_secs(1),
    };
    let error = worker.execute(&request()).await.expect_err("shell denied");
    assert!(error.to_string().contains("unsafe control"));
}

#[tokio::test]
async fn pi_worker_rejects_typed_patches_that_do_not_require_approval() {
    let request = request();
    let result = serde_json::json!({
        "protocolVersion": request.protocol_version,
        "jobId": request.job_id,
        "nonce": request.nonce,
        "diagnosis": "test failure",
        "confidence": 0.8,
        "proposedFiles": ["src/lib.rs"],
        "proposedPatches": [{
            "path": "src/lib.rs",
            "unifiedDiff": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n"
        }],
        "validationCommands": ["cargo test"],
        "risks": [],
        "requiresApproval": false
    });
    let worker = PiWorker {
        command: vec![
            "printf".into(),
            "%s".into(),
            serde_json::to_string(&result).unwrap(),
        ],
        timeout: Duration::from_secs(1),
    };

    let error = worker
        .execute(&request)
        .await
        .expect_err("typed mutation without approval must fail closed");
    assert!(error.to_string().contains("explicit approval"));
}
