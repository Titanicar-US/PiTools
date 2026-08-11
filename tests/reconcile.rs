pub use pitools::{github, models, policy};

#[path = "../src/reconcile.rs"]
mod reconcile;

use github::{
    client::{
        Branch, BranchCommit, BranchProtection, CheckApp, CombinedStatus, CommitStatus,
        GitHubCheckRun, GitHubCommentAuthor, GitHubPullRequest, GitHubPullRequestBranch,
        PullRequestReadSet, PullRequestReview, RequiredStatusCheck, RequiredStatusChecks,
    },
    events::{CheckRecord, FeedbackRecord},
};
use models::{CheckConclusion, CheckSnapshot, CheckStatus, FeedbackSnapshot, ReadinessInput};
use policy::Policy;
use reconcile::reconcile_read_set;

fn author(login: &str, actor_type: &str) -> GitHubCommentAuthor {
    GitHubCommentAuthor {
        login: login.into(),
        actor_type: actor_type.into(),
    }
}

#[test]
fn converts_shuffled_github_reads_into_deterministic_readiness_and_records() {
    let data = PullRequestReadSet {
        pull_request: GitHubPullRequest {
            number: 7,
            draft: false,
            state: Some("open".into()),
            merged: Some(false),
            mergeable_state: Some("clean".into()),
            mergeable: Some(true),
            head: GitHubPullRequestBranch {
                sha: "head-sha".into(),
                reference: None,
                repository: None,
            },
            base: GitHubPullRequestBranch {
                sha: "old-base".into(),
                reference: Some("main".into()),
                repository: None,
            },
        },
        base_branch: Branch {
            name: "main".into(),
            protected: true,
            commit: BranchCommit {
                sha: "current-base".into(),
            },
        },
        branch_protection: BranchProtection {
            required_status_checks: Some(RequiredStatusChecks {
                strict: true,
                contexts: vec!["legacy/status".into(), "security".into()],
                checks: vec![RequiredStatusCheck {
                    context: "build".into(),
                    app_id: Some(42),
                }],
            }),
            required_pull_request_reviews: None,
        },
        reviews: vec![
            PullRequestReview {
                id: 12,
                body: Some("now approved".into()),
                state: "APPROVED".into(),
                submitted_at: Some("2026-08-10T00:00:00Z".into()),
                commit_id: None,
                user: author("reviewer", "User"),
            },
            PullRequestReview {
                id: 11,
                body: Some("please fix".into()),
                state: "CHANGES_REQUESTED".into(),
                submitted_at: Some("2026-08-09T00:00:00Z".into()),
                commit_id: None,
                user: author("reviewer", "User"),
            },
        ],
        issue_comments: vec![github::client::PullRequestComment {
            id: 22,
            body: Some("human note".into()),
            user: author("maintainer", "User"),
            path: None,
            start_line: None,
            line: None,
        }],
        review_comments: vec![github::client::PullRequestComment {
            id: 21,
            body: Some("automation note".into()),
            user: author("review-bot[bot]", "Bot"),
            path: Some("src/lib.rs".into()),
            start_line: Some(4),
            line: Some(5),
        }],
        check_runs: vec![GitHubCheckRun {
            id: 81,
            name: "build".into(),
            status: "completed".into(),
            conclusion: Some("failure".into()),
            details_url: Some("https://ci.example/check/81".into()),
            app: Some(CheckApp { id: 42 }),
            output: None,
        }],
        combined_status: CombinedStatus {
            state: "success".into(),
            sha: "head-sha".into(),
            statuses: vec![CommitStatus {
                id: 91,
                context: "legacy/status".into(),
                state: "success".into(),
                target_url: Some("https://ci.example/status/91".into()),
            }],
        },
    };
    let policy = Policy {
        automation_actors: vec!["review-bot[bot]".into()],
        ..Policy::default()
    };

    let reconciled = reconcile_read_set(&data, &policy);

    assert_eq!(
        reconciled.readiness,
        ReadinessInput {
            mergeable: Some(true),
            branch_is_current: false,
            required_checks: vec![
                CheckSnapshot {
                    name: "build".into(),
                    app_id: Some(42),
                    required_app_id: Some(42),
                    status: CheckStatus::Completed,
                    conclusion: Some(CheckConclusion::Failure),
                    required: true,
                },
                CheckSnapshot {
                    name: "legacy/status".into(),
                    app_id: None,
                    required_app_id: None,
                    status: CheckStatus::Completed,
                    conclusion: Some(CheckConclusion::Success),
                    required: true,
                },
                CheckSnapshot {
                    name: "security".into(),
                    app_id: None,
                    required_app_id: None,
                    status: CheckStatus::Unknown,
                    conclusion: None,
                    required: true,
                },
            ],
            approved: true,
            approval_count: 1,
            required_approval_count: 0,
            stale_approval_present: false,
            last_push_approval_required: false,
            latest_push_approval: true,
            code_owner_review_required: false,
            code_owner_review_satisfied: true,
            unresolved_feedback: vec![
                FeedbackSnapshot {
                    id: "issue-comment:22".into(),
                    actor_login: "maintainer".into(),
                    actor_type: "User".into(),
                    body: "human note".into(),
                    resolved: false,
                    is_automation: false,
                },
                FeedbackSnapshot {
                    id: "review-comment:21".into(),
                    actor_login: "review-bot[bot]".into(),
                    actor_type: "Bot".into(),
                    body: "automation note".into(),
                    resolved: false,
                    is_automation: true,
                },
            ],
            is_draft: false,
        }
    );
    assert_eq!(
        reconciled.feedback,
        vec![
            FeedbackRecord {
                id: "issue-comment:22".into(),
                actor_login: "maintainer".into(),
                actor_type: "User".into(),
                body: "human note".into(),
                resolved: false,
                is_automation: false,
                path: None,
                start_line: None,
                line: None,
            },
            FeedbackRecord {
                id: "review-comment:21".into(),
                actor_login: "review-bot[bot]".into(),
                actor_type: "Bot".into(),
                body: "automation note".into(),
                resolved: false,
                is_automation: true,
                path: Some("src/lib.rs".into()),
                start_line: Some(4),
                line: Some(5),
            },
            FeedbackRecord {
                id: "review:11".into(),
                actor_login: "reviewer".into(),
                actor_type: "User".into(),
                body: "please fix".into(),
                resolved: true,
                is_automation: false,
                path: None,
                start_line: None,
                line: None,
            },
            FeedbackRecord {
                id: "review:12".into(),
                actor_login: "reviewer".into(),
                actor_type: "User".into(),
                body: "now approved".into(),
                resolved: true,
                is_automation: false,
                path: None,
                start_line: None,
                line: None,
            },
        ]
    );
    assert_eq!(
        reconciled.checks,
        vec![
            CheckRecord {
                external_id: "81".into(),
                name: "build".into(),
                app_id: Some(42),
                status: "completed".into(),
                conclusion: Some("failure".into()),
                details_url: Some("https://ci.example/check/81".into()),
            },
            CheckRecord {
                external_id: "status:91".into(),
                name: "legacy/status".into(),
                app_id: None,
                status: "completed".into(),
                conclusion: Some("success".into()),
                details_url: Some("https://ci.example/status/91".into()),
            },
        ]
    );
}
