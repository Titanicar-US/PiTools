use chrono::Utc;

use crate::{models::*, policy::Policy};

pub fn evaluate(input: &ReadinessInput, policy: &Policy) -> ReadinessSnapshot {
    let mut reason_codes = Vec::new();

    if input.is_draft {
        reason_codes.push(ReadinessReason::Draft);
    }
    match input.mergeable {
        Some(true) => {}
        Some(false) => reason_codes.push(ReadinessReason::MergeConflict),
        None => reason_codes.push(ReadinessReason::MergeabilityUnknown),
    }
    if policy.require_current_branch && !input.branch_is_current {
        reason_codes.push(ReadinessReason::BranchOutOfDate);
    }

    let mut required_names: Vec<&str> = input
        .required_checks
        .iter()
        .filter(|check| check.required)
        .map(|check| check.name.as_str())
        .chain(policy.required_checks.iter().map(String::as_str))
        .collect();
    required_names.sort_unstable();
    required_names.dedup();
    for required_name in required_names {
        let Some(check) = input
            .required_checks
            .iter()
            .find(|check| check.name == required_name)
        else {
            reason_codes.push(ReadinessReason::RequiredCheckMissing);
            continue;
        };
        if check.required_app_id.is_some() && check.app_id != check.required_app_id {
            reason_codes.push(ReadinessReason::RequiredCheckWrongApp);
            continue;
        }
        match (&check.status, &check.conclusion) {
            (CheckStatus::Completed, Some(CheckConclusion::Success | CheckConclusion::Skipped)) => {
            }
            (CheckStatus::Completed, _) => reason_codes.push(ReadinessReason::RequiredCheckFailed),
            _ => reason_codes.push(ReadinessReason::RequiredCheckPending),
        }
    }

    if policy.require_approval && !input.approved {
        reason_codes.push(ReadinessReason::RequiredReviewMissing);
    }
    if input.required_approval_count > input.approval_count {
        if input.approval_count == 0 {
            reason_codes.push(ReadinessReason::RequiredReviewMissing);
        } else {
            reason_codes.push(ReadinessReason::RequiredReviewCountMissing);
        }
    }
    if input.last_push_approval_required && !input.latest_push_approval {
        if input.stale_approval_present {
            reason_codes.push(ReadinessReason::StaleReview);
        } else {
            reason_codes.push(ReadinessReason::LastPushApprovalMissing);
        }
    }
    if input.code_owner_review_required && !input.code_owner_review_satisfied {
        reason_codes.push(ReadinessReason::CodeOwnerReviewMissing);
    }
    if input.unresolved_feedback.iter().any(|feedback| {
        policy.is_configured_automation_actor(&feedback.actor_login) && !feedback.resolved
    }) {
        reason_codes.push(ReadinessReason::UnresolvedAutomationFeedback);
    }

    reason_codes.sort_by_key(|reason| format!("{reason:?}"));
    reason_codes.dedup();
    ReadinessSnapshot {
        ready: reason_codes.is_empty(),
        reason_codes,
        evaluated_at: Utc::now(),
        snapshot_id: uuid::Uuid::now_v7(),
    }
}
