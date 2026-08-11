use std::fs;

use pitools::feedback::{
    Feedback, FeedbackCommentTarget, FeedbackError, RepairDecision, RepairDisposition,
    parse_feedback_comment_target, repair_feedback,
};
use tempfile::tempdir;

const ACTOR: &str = "repair-bot[bot]";

fn suggestion(path: &str, hunk: &str) -> String {
    format!("```suggestion\n--- a/{path}\n+++ b/{path}\n{hunk}\n```")
}

fn automation(body: String) -> Feedback {
    Feedback {
        actor_login: ACTOR.into(),
        actor_type: "Bot".into(),
        is_automation: true,
        body,
        path: None,
        start_line: None,
        line: None,
    }
}

#[test]
fn accepts_an_exact_suggestion_for_preview_without_mutating() {
    let repository = tempdir().expect("temporary repository");
    fs::create_dir(repository.path().join("src")).expect("create src");
    let target = repository.path().join("src/example.rs");
    fs::write(&target, "fn before() {}\n").expect("write fixture");
    let feedback = automation(suggestion(
        "src/example.rs",
        "@@ -1,1 +1,1 @@\n-fn before() {}\n+fn after() {}",
    ));

    let outcome = repair_feedback(
        repository.path(),
        &[ACTOR.to_string()],
        &feedback,
        RepairDecision::Preview,
    )
    .expect("valid exact suggestion");

    assert_eq!(outcome.disposition, RepairDisposition::Previewed);
    assert_eq!(outcome.path.as_deref(), Some("src/example.rs"));
    assert!(!outcome.resolution_eligible);
    assert_eq!(
        fs::read_to_string(target).expect("read fixture"),
        "fn before() {}\n"
    );
}

#[test]
fn rejects_an_automation_actor_not_in_the_allowlist() {
    let repository = tempdir().expect("temporary repository");
    let feedback = automation(suggestion("file.txt", "@@ -1 +1 @@\n-old\n+new"));

    let outcome = repair_feedback(
        repository.path(),
        &["another-bot[bot]".into()],
        &feedback,
        RepairDecision::Apply,
    )
    .expect("ineligible feedback is a non-mutating outcome");

    assert_eq!(outcome.disposition, RepairDisposition::RejectedActor);
    assert!(!outcome.resolution_eligible);
}

#[test]
fn rejects_repository_path_traversal() {
    let repository = tempdir().expect("temporary repository");
    let feedback = automation(suggestion("../outside.txt", "@@ -1 +1 @@\n-old\n+new"));

    let error = repair_feedback(
        repository.path(),
        &[ACTOR.into()],
        &feedback,
        RepairDecision::Preview,
    )
    .expect_err("traversal must fail closed");

    assert!(matches!(error, FeedbackError::InvalidPath(_)));
}

#[cfg(unix)]
#[test]
fn rejects_a_symlink_target() {
    use std::os::unix::fs::symlink;

    let repository = tempdir().expect("temporary repository");
    let outside = tempdir().expect("outside directory");
    let outside_file = outside.path().join("outside.txt");
    fs::write(&outside_file, "old\n").expect("write outside fixture");
    symlink(&outside_file, repository.path().join("file.txt")).expect("create symlink");
    let feedback = automation(suggestion("file.txt", "@@ -1 +1 @@\n-old\n+new"));

    let error = repair_feedback(
        repository.path(),
        &[ACTOR.into()],
        &feedback,
        RepairDecision::Apply,
    )
    .expect_err("symlink target must fail closed");

    assert!(matches!(error, FeedbackError::InvalidPath(_)));
    assert_eq!(
        fs::read_to_string(outside_file).expect("read outside fixture"),
        "old\n"
    );
}

#[test]
fn rejects_a_malformed_hunk() {
    let repository = tempdir().expect("temporary repository");
    fs::write(repository.path().join("file.txt"), "old\n").expect("write fixture");
    let feedback = automation(suggestion("file.txt", "@@ -1,2 +1,1 @@\n-old\n+new"));

    let error = repair_feedback(
        repository.path(),
        &[ACTOR.into()],
        &feedback,
        RepairDecision::Preview,
    )
    .expect_err("hunk line counts must be exact");

    assert!(matches!(error, FeedbackError::MalformedPatch(_)));
}

#[test]
fn keeps_human_feedback_non_mutating() {
    let repository = tempdir().expect("temporary repository");
    let target = repository.path().join("file.txt");
    fs::write(&target, "old\n").expect("write fixture");
    let feedback = Feedback {
        actor_login: "person".into(),
        actor_type: "User".into(),
        is_automation: false,
        body: suggestion("file.txt", "@@ -1 +1 @@\n-old\n+new"),
        path: None,
        start_line: None,
        line: None,
    };

    let outcome = repair_feedback(
        repository.path(),
        &["person".into()],
        &feedback,
        RepairDecision::Apply,
    )
    .expect("human feedback is ignored safely");

    assert_eq!(outcome.disposition, RepairDisposition::HumanFeedback);
    assert!(!outcome.resolution_eligible);
    assert_eq!(fs::read_to_string(target).expect("read fixture"), "old\n");
}

#[test]
fn becomes_resolution_eligible_only_after_apply_succeeds() {
    let repository = tempdir().expect("temporary repository");
    let target = repository.path().join("file.txt");
    fs::write(&target, "old\n").expect("write fixture");
    let feedback = automation(suggestion("file.txt", "@@ -1 +1 @@\n-old\n+new"));

    let preview = repair_feedback(
        repository.path(),
        &[ACTOR.into()],
        &feedback,
        RepairDecision::Preview,
    )
    .expect("preview");
    assert!(!preview.resolution_eligible);

    let applied = repair_feedback(
        repository.path(),
        &[ACTOR.into()],
        &feedback,
        RepairDecision::Apply,
    )
    .expect("apply");

    assert_eq!(applied.disposition, RepairDisposition::Applied);
    assert!(applied.resolution_eligible);
    assert_eq!(fs::read_to_string(target).expect("read fixture"), "new\n");
}

#[test]
fn classifies_review_comments_and_pr_conversation_feedback_targets() {
    assert_eq!(
        parse_feedback_comment_target("review-comment:71").expect("review comment target"),
        FeedbackCommentTarget::ReviewComment { comment_id: 71 }
    );
    assert_eq!(
        parse_feedback_comment_target("issue-comment:72").expect("issue comment target"),
        FeedbackCommentTarget::PullRequestConversation { comment_id: 72 }
    );
    assert_eq!(
        parse_feedback_comment_target("review:73").expect("review target"),
        FeedbackCommentTarget::PullRequestConversation { comment_id: 73 }
    );
}

#[test]
fn rejects_feedback_targets_with_unknown_or_invalid_ids() {
    assert!(parse_feedback_comment_target("commit-comment:71").is_err());
    assert!(parse_feedback_comment_target("issue-comment:not-a-number").is_err());
}

#[test]
fn applies_a_bounded_github_inline_suggestion() {
    let repository = tempdir().expect("temporary repository");
    let target = repository.path().join("file.txt");
    fs::write(&target, "old\nsecond\n").expect("write fixture");
    let feedback = Feedback {
        actor_login: ACTOR.into(),
        actor_type: "Bot".into(),
        is_automation: true,
        body: "Please apply this exact replacement:\n```suggestion\nnew\n```".into(),
        path: Some("file.txt".into()),
        start_line: Some(1),
        line: Some(1),
    };

    let outcome = repair_feedback(
        repository.path(),
        &[ACTOR.into()],
        &feedback,
        RepairDecision::Apply,
    )
    .expect("inline suggestion applies");

    assert_eq!(outcome.disposition, RepairDisposition::Applied);
    assert!(outcome.resolution_eligible);
    assert_eq!(
        fs::read_to_string(target).expect("read fixture"),
        "new\nsecond\n"
    );
}

#[test]
fn rejects_an_inline_suggestion_without_review_coordinates() {
    let repository = tempdir().expect("temporary repository");
    fs::write(repository.path().join("file.txt"), "old\n").expect("write fixture");
    let feedback = Feedback {
        actor_login: ACTOR.into(),
        actor_type: "Bot".into(),
        is_automation: true,
        body: "```suggestion\nnew\n```".into(),
        path: None,
        start_line: None,
        line: None,
    };

    let error = repair_feedback(
        repository.path(),
        &[ACTOR.into()],
        &feedback,
        RepairDecision::Preview,
    )
    .expect_err("inline suggestion without coordinates must fail closed");

    assert!(matches!(error, FeedbackError::MalformedSuggestion(_)));
}
