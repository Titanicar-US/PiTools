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
