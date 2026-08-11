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
    let required_names = required_check_names(data, policy);
    let mut readiness_checks: BTreeMap<String, CheckSnapshot> = checks
        .iter()
        .map(|check| {
            (
                check.name.clone(),
                CheckSnapshot {
                    name: check.name.clone(),
                    app_id: check.app_id,
                    required_app_id: required_check_app_id(data, &check.name),
                    status: check_status(&check.status),
                    conclusion: check.conclusion.as_deref().map(check_conclusion),
                    required: required_names.contains(&check.name),
                },
            )
        })
        .collect();
    for required_name in required_names {
        let required_app_id = required_check_app_id(data, &required_name);
        readiness_checks
            .entry(required_name.clone())
            .or_insert(CheckSnapshot {
                name: required_name,
                app_id: None,
                required_app_id,
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
    let (approval_count, stale_approval_present) =
        current_approval_count(&data.reviews, &data.pull_request.head.sha);
    let review_requirements = data
        .branch_protection
        .required_pull_request_reviews
        .clone()
        .unwrap_or_default();
    let code_owner_review_required = review_requirements.require_code_owner_reviews;
    let code_owner_review_satisfied = !code_owner_review_required
        || data
            .pull_request
            .mergeable_state
            .as_deref()
            .is_some_and(|state| !matches!(state, "blocked" | "unknown"));
    let latest_push_approval = latest_review_reviews(&data.reviews).values().any(|review| {
        review.state == "APPROVED"
            && review.commit_id.as_deref() == Some(data.pull_request.head.sha.as_str())
    });

    ReconciledPullRequest {
        readiness: ReadinessInput {
            mergeable: data.pull_request.mergeable,
            branch_is_current: data.pull_request.base.sha == data.base_branch.commit.sha,
            required_checks: readiness_checks.into_values().collect(),
            approved: approval_count > 0,
            approval_count,
            required_approval_count: review_requirements.required_approving_review_count,
            stale_approval_present,
            last_push_approval_required: review_requirements.require_last_push_approval,
            latest_push_approval: latest_push_approval || approval_count > 0,
            code_owner_review_required,
            code_owner_review_satisfied,
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

fn current_approval_count(reviews: &[PullRequestReview], head_sha: &str) -> (usize, bool) {
    let mut count = 0;
    let mut stale = false;
    for review in latest_review_reviews(reviews).values() {
        if review.state != "APPROVED" {
            continue;
        }
        match review.commit_id.as_deref() {
            None => count += 1,
            Some(commit) if commit == head_sha => count += 1,
            Some(_) => stale = true,
        }
    }
    (count, stale)
}

fn latest_review_reviews(reviews: &[PullRequestReview]) -> BTreeMap<String, &PullRequestReview> {
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
            latest.insert(review.user.login.clone(), review);
        }
    }
    latest
}

fn required_check_names(data: &PullRequestReadSet, policy: &Policy) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = policy.required_checks.iter().cloned().collect();
    if let Some(required) = &data.branch_protection.required_status_checks {
        names.extend(required.contexts.iter().cloned());
        names.extend(required.checks.iter().map(|check| check.context.clone()));
    }
    names
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
                app_id: None,
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
                app_id: check.app.as_ref().map(|app| app.id),
                status: check.status.clone(),
                conclusion: check.conclusion.clone(),
                details_url: check.details_url.clone(),
            },
        );
    }
    by_name.into_values().collect()
}

fn required_check_app_id(data: &PullRequestReadSet, name: &str) -> Option<i64> {
    data.branch_protection
        .required_status_checks
        .as_ref()?
        .checks
        .iter()
        .find(|check| check.context == name)
        .and_then(|check| check.app_id)
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
