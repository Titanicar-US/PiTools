use pitools::workspace::{
    RepositoryWorkspace, rebase_push_argv, remote_head_argv, validate_staged_diff_summary,
    validate_staged_paths,
};
use secrecy::SecretString;

#[tokio::test]
async fn rejects_untrusted_clone_coordinates_before_network_access() {
    let error = RepositoryWorkspace::clone_branch(
        "acme/owner",
        "repo",
        "main",
        "0123456789abcdef",
        SecretString::from("installation-token"),
    )
    .await
    .expect_err("owner path traversal must be rejected");
    assert!(error.to_string().contains("invalid owner"));
}

#[tokio::test]
async fn rejects_invalid_branch_and_commit_coordinates_before_network_access() {
    let branch = RepositoryWorkspace::clone_branch(
        "acme",
        "repo",
        "../main",
        "0123456789abcdef",
        SecretString::from("installation-token"),
    )
    .await
    .expect_err("branch traversal must be rejected");
    assert!(branch.to_string().contains("invalid branch"));

    let sha = RepositoryWorkspace::clone_branch(
        "acme",
        "repo",
        "main",
        "not-a-sha",
        SecretString::from("installation-token"),
    )
    .await
    .expect_err("invalid SHA must be rejected");
    assert!(sha.to_string().contains("invalid commit SHA"));
}

#[test]
fn rebase_push_uses_force_with_lease_only_when_explicitly_allowed() {
    assert_eq!(
        rebase_push_argv("pitools/stack", "0123456789abcdef", false).expect("safe push"),
        vec![
            "git".to_owned(),
            "push".to_owned(),
            "origin".to_owned(),
            "HEAD:refs/heads/pitools/stack".to_owned(),
        ]
    );
    assert_eq!(
        rebase_push_argv("pitools/stack", "0123456789abcdef", true).expect("leased rewrite"),
        vec![
            "git".to_owned(),
            "push".to_owned(),
            "--force-with-lease=refs/heads/pitools/stack:0123456789abcdef".to_owned(),
            "origin".to_owned(),
            "HEAD:refs/heads/pitools/stack".to_owned(),
        ]
    );
}

#[test]
fn rebase_push_rejects_branch_argument_injection() {
    let error = rebase_push_argv("--delete", "0123456789abcdef", true)
        .expect_err("branch flags must be rejected");
    assert!(error.to_string().contains("invalid branch"));

    let ref_error = rebase_push_argv("feature:bad", "0123456789abcdef", true)
        .expect_err("git ref syntax must be rejected");
    assert!(ref_error.to_string().contains("invalid branch"));
}

#[test]
fn staged_paths_must_match_the_approved_mutation_set_exactly() {
    validate_staged_paths(&["src/lib.rs".into()], "src/lib.rs\n").expect("exact staged path set");
    assert!(validate_staged_paths(&["src/lib.rs".into()], "src/lib.rs\nsrc/secret\n").is_err());
    assert!(validate_staged_paths(&["src/lib.rs".into()], "src/secret\n").is_err());
}

#[test]
fn staged_diff_must_not_change_modes_or_create_renames() {
    validate_staged_diff_summary("file changed\n").expect("ordinary content change");
    assert!(validate_staged_diff_summary(" mode change 100644 => 100755 file").is_err());
    assert!(validate_staged_diff_summary("rename from old\nrename to new").is_err());
}

#[test]
fn remote_head_check_uses_the_exact_branch_ref() {
    assert_eq!(
        remote_head_argv("feature/repair").expect("safe branch"),
        vec![
            "git".to_owned(),
            "ls-remote".to_owned(),
            "origin".to_owned(),
            "refs/heads/feature/repair".to_owned(),
        ]
    );
}
