use std::{path::Path, time::Duration};

use pitools::{
    ci::{
        CiFailure, CiFailureInput, CiFailureKind, CiPatch, CiRepairPlan, WorktreePlan,
        redact_ci_text, validate_patch_set,
    },
    policy::ValidationCommand,
};

#[test]
fn ci_failure_classification_and_typed_plan_are_deterministic() {
    let failure = CiFailure::new(CiFailureInput {
        repository_id: 42,
        pull_request_number: 7,
        head_sha: "abc123".into(),
        workflow_name: "pull-request-tests".into(),
        job_name: "cargo test".into(),
        check_name: "tests".into(),
        details_url: Some("https://github.com/acme/repo/actions/runs/1".into()),
        log_excerpt: "test failed".into(),
    })
    .expect("valid CI failure");
    assert_eq!(failure.kind, CiFailureKind::Test);
    let worktree = WorktreePlan::new("/runner", "feature/one", "abc123").unwrap();
    let plan = CiRepairPlan::new(failure, worktree, vec![ValidationCommand::CargoTest])
        .expect("typed plan");
    assert!(plan.requires_approval);
    assert_eq!(
        plan.validation_commands[0].argv(),
        &["cargo", "test", "--locked"]
    );
}

#[test]
fn worktree_argv_is_detached_and_cannot_interpret_shell_text() {
    let worktree = WorktreePlan::new("/runner", "feature/one", "abc123").unwrap();
    let argv = worktree
        .git_worktree_add_argv(Path::new("job-1"))
        .expect("relative destination");
    assert_eq!(argv[0..4], ["git", "worktree", "add", "--detach"]);
    assert!(!argv.iter().any(|part| part == "sh" || part.contains(';')));
    assert!(
        worktree
            .git_worktree_add_argv(Path::new("/tmp/escape"))
            .is_err()
    );
}

#[tokio::test]
async fn command_runner_rejects_empty_command_lists_before_execution() {
    let error = pitools::ci::run_validation_commands(Path::new("."), &[], Duration::from_secs(1))
        .await
        .expect_err("empty command list must fail closed");
    assert!(error.to_string().contains("no validation commands"));
}

#[tokio::test]
async fn command_runner_can_feed_patch_bytes_without_a_shell() {
    let result = pitools::ci::run_bounded_argv_with_input(
        Path::new("."),
        &["cat".into()],
        Duration::from_secs(1),
        &[],
        b"patch-input",
    )
    .await
    .expect("bounded stdin command");
    assert!(result.success);
    assert_eq!(result.stdout, "patch-input");
}

#[test]
fn ci_patch_set_accepts_only_single_allowlisted_unified_files() {
    let patch = CiPatch {
        path: "src/lib.rs".into(),
        unified_diff: "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n".into(),
    };
    validate_patch_set(std::slice::from_ref(&patch), &["src/lib.rs".into()])
        .expect("allowlisted patch is valid");

    let wrong_file = CiPatch {
        path: "src/lib.rs".into(),
        unified_diff: patch.unified_diff.replace("src/lib.rs", "src/other.rs"),
    };
    assert!(validate_patch_set(&[wrong_file], &["src/lib.rs".into()]).is_err());
}

#[test]
fn ci_patch_set_rejects_traversal_and_multi_file_patches() {
    let traversal = CiPatch {
        path: "../secret".into(),
        unified_diff: String::new(),
    };
    assert!(validate_patch_set(&[traversal], &["../secret".into()]).is_err());

    let multi_file = CiPatch {
        path: "src/lib.rs".into(),
        unified_diff: "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-a\n+b\n--- a/src/other.rs\n+++ b/src/other.rs\n@@ -1 +1 @@\n-c\n+d\n".into(),
    };
    assert!(validate_patch_set(&[multi_file], &["src/lib.rs".into()]).is_err());
}

#[test]
fn redacts_common_secret_markers_from_ci_evidence() {
    let redacted = redact_ci_text(
        "Authorization: Bearer ghp_example\nTOKEN=github_pat_example\nerror: failed",
    );

    assert!(!redacted.contains("ghp_example"));
    assert!(!redacted.contains("github_pat_example"));
    assert!(redacted.contains("[REDACTED]"));
    assert!(redacted.contains("error: failed"));
}
