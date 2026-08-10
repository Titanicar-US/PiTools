use std::collections::{BTreeMap, BTreeSet};

use crate::{
    github::{
        client::{
            CommitStatus, GitHubCheckRun, PullRequestComment, PullRequestReadSet, PullRequestReview,
        },
        events::{CheckRecord, FeedbackRecord},
    },
    models::{CheckConclusion, CheckSnapshot, CheckStatus, FeedbackSnapshot, ReadinessInput},
    policy::Policy,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciledPullRequest {
    pub readiness: ReadinessInput,
    pub feedback: Vec<FeedbackRecord>,
    pub checks: Vec<CheckRecord>,
}

pub fn reconcile_read_set(data: &PullRequestReadSet, policy: &Policy) -> ReconciledPullRequest {
    let feedback = feedback_records(data, policy);
    let checks = check_records(data);
    let required_names = required_check_names(data);
    let mut readiness_checks: BTreeMap<String, CheckSnapshot> = checks
        .iter()
        .map(|check| {
            (
                check.name.clone(),
                CheckSnapshot {
                    name: check.name.clone(),
                    status: check_status(&check.status),
                    conclusion: check.conclusion.as_deref().map(check_conclusion),
                    required: required_names.contains(&check.name),
                },
            )
        })
        .collect();
    for required_name in required_names {
        readiness_checks
            .entry(required_name.clone())
            .or_insert(CheckSnapshot {
                name: required_name,
                status: CheckStatus::Unknown,
                conclusion: None,
                required: true,
            });
    }
    let unresolved_feedback = feedback
        .iter()
        .filter(|record| !record.resolved)
        .map(|record| FeedbackSnapshot {
            id: record.id.clone(),
            actor_login: record.actor_login.clone(),
            actor_type: record.actor_type.clone(),
            body: record.body.clone(),
            resolved: record.resolved,
            is_automation: record.is_automation,
        })
        .collect();

    ReconciledPullRequest {
        readiness: ReadinessInput {
            mergeable: data.pull_request.mergeable,
            branch_is_current: data.pull_request.base.sha == data.base_branch.commit.sha,
            required_checks: readiness_checks.into_values().collect(),
            approved: has_current_approval(&data.reviews),
            unresolved_feedback,
            is_draft: data.pull_request.draft,
        },
        feedback,
        checks,
    }
}

fn feedback_records(data: &PullRequestReadSet, policy: &Policy) -> Vec<FeedbackRecord> {
    let latest_decisions = latest_review_decisions(&data.reviews);
    let mut records = Vec::new();

    for review in &data.reviews {
        let Some(body) = nonempty_body(review.body.as_deref()) else {
            continue;
        };
        let latest_state = latest_decisions.get(&review.user.login).map(String::as_str);
        records.push(FeedbackRecord {
            id: format!("review:{}", review.id),
            actor_login: review.user.login.clone(),
            actor_type: review.user.actor_type.clone(),
            body,
            resolved: matches!(review.state.as_str(), "APPROVED" | "DISMISSED")
                || matches!(latest_state, Some("APPROVED" | "DISMISSED")),
            is_automation: is_automation(&review.user.login, &review.user.actor_type, policy),
            path: None,
            start_line: None,
            line: None,
        });
    }
    records.extend(comment_records(
        "issue-comment",
        &data.issue_comments,
        policy,
    ));
    records.extend(comment_records(
        "review-comment",
        &data.review_comments,
        policy,
    ));
    records.sort_by(|left, right| left.id.cmp(&right.id));
    records.dedup_by(|left, right| left.id == right.id);
    records
}

fn comment_records(
    prefix: &str,
    comments: &[PullRequestComment],
    policy: &Policy,
) -> Vec<FeedbackRecord> {
    comments
        .iter()
        .filter_map(|comment| {
            nonempty_body(comment.body.as_deref()).map(|body| FeedbackRecord {
                id: format!("{prefix}:{}", comment.id),
                actor_login: comment.user.login.clone(),
                actor_type: comment.user.actor_type.clone(),
                body,
                resolved: false,
                is_automation: is_automation(&comment.user.login, &comment.user.actor_type, policy),
                path: comment.path.clone(),
                start_line: comment.start_line,
                line: comment.line,
            })
        })
        .collect()
}

fn nonempty_body(body: Option<&str>) -> Option<String> {
    body.map(str::trim)
        .filter(|body| !body.is_empty())
        .map(str::to_owned)
}

fn is_automation(login: &str, actor_type: &str, policy: &Policy) -> bool {
    actor_type == "Bot" || policy.is_configured_automation_actor(login)
}

fn latest_review_decisions(reviews: &[PullRequestReview]) -> BTreeMap<String, String> {
    let mut ordered: Vec<&PullRequestReview> = reviews.iter().collect();
    ordered.sort_by(|left, right| {
        left.submitted_at
            .cmp(&right.submitted_at)
            .then(left.id.cmp(&right.id))
    });
    let mut latest = BTreeMap::new();
    for review in ordered {
        if matches!(
            review.state.as_str(),
            "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED"
        ) {
            latest.insert(review.user.login.clone(), review.state.clone());
        }
    }
    latest
}

fn has_current_approval(reviews: &[PullRequestReview]) -> bool {
    latest_review_decisions(reviews)
        .values()
        .any(|state| state == "APPROVED")
}

fn required_check_names(data: &PullRequestReadSet) -> BTreeSet<String> {
    let Some(required) = &data.branch_protection.required_status_checks else {
        return BTreeSet::new();
    };
    required
        .contexts
        .iter()
        .cloned()
        .chain(required.checks.iter().map(|check| check.context.clone()))
        .collect()
}

fn check_records(data: &PullRequestReadSet) -> Vec<CheckRecord> {
    let mut by_name = BTreeMap::new();
    let mut statuses: Vec<&CommitStatus> = data.combined_status.statuses.iter().collect();
    statuses.sort_by_key(|status| status.id);
    for status in statuses {
        let (check_status, conclusion) = commit_status_state(&status.state);
        by_name.insert(
            status.context.clone(),
            CheckRecord {
                external_id: format!("status:{}", status.id),
                name: status.context.clone(),
                status: check_status.into(),
                conclusion: conclusion.map(str::to_owned),
                details_url: status.target_url.clone(),
            },
        );
    }

    let mut check_runs: Vec<&GitHubCheckRun> = data.check_runs.iter().collect();
    check_runs.sort_by_key(|check| check.id);
    for check in check_runs {
        by_name.insert(
            check.name.clone(),
            CheckRecord {
                external_id: check.id.to_string(),
                name: check.name.clone(),
                status: check.status.clone(),
                conclusion: check.conclusion.clone(),
                details_url: check.details_url.clone(),
            },
        );
    }
    by_name.into_values().collect()
}

fn commit_status_state(state: &str) -> (&'static str, Option<&'static str>) {
    match state {
        "success" => ("completed", Some("success")),
        "failure" | "error" => ("completed", Some("failure")),
        "pending" => ("in_progress", None),
        _ => ("unknown", None),
    }
}

fn check_status(status: &str) -> CheckStatus {
    match status {
        "queued" | "pending" | "requested" | "waiting" => CheckStatus::Queued,
        "in_progress" => CheckStatus::InProgress,
        "completed" => CheckStatus::Completed,
        _ => CheckStatus::Unknown,
    }
}

fn check_conclusion(conclusion: &str) -> CheckConclusion {
    match conclusion {
        "success" => CheckConclusion::Success,
        "failure" => CheckConclusion::Failure,
        "cancelled" => CheckConclusion::Cancelled,
        "timed_out" => CheckConclusion::TimedOut,
        "neutral" => CheckConclusion::Neutral,
        "skipped" => CheckConclusion::Skipped,
        "action_required" => CheckConclusion::ActionRequired,
        "stale" => CheckConclusion::Stale,
        "startup_failure" => CheckConclusion::StartupFailure,
        _ => CheckConclusion::Unknown,
    }
}
