use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestKey {
    pub repository_id: i64,
    pub number: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestSnapshot {
    pub repository_id: i64,
    pub github_id: i64,
    pub number: i32,
    pub title: String,
    pub url: String,
    pub state: PullRequestState,
    pub draft: bool,
    pub merged: bool,
    pub head_sha: String,
    pub base_sha: String,
    pub head_branch: String,
    pub base_branch: String,
    pub author_login: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckSnapshot {
    pub name: String,
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Queued,
    InProgress,
    Completed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckConclusion {
    Success,
    Failure,
    Cancelled,
    TimedOut,
    Neutral,
    Skipped,
    ActionRequired,
    Stale,
    StartupFailure,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedbackSnapshot {
    pub id: String,
    pub actor_login: String,
    pub actor_type: String,
    pub body: String,
    pub resolved: bool,
    pub is_automation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadinessInput {
    pub mergeable: Option<bool>,
    pub branch_is_current: bool,
    pub required_checks: Vec<CheckSnapshot>,
    pub approved: bool,
    pub unresolved_feedback: Vec<FeedbackSnapshot>,
    pub is_draft: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadinessSnapshot {
    pub ready: bool,
    pub reason_codes: Vec<ReadinessReason>,
    pub evaluated_at: DateTime<Utc>,
    pub snapshot_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessReason {
    Draft,
    MergeabilityUnknown,
    MergeConflict,
    BranchOutOfDate,
    RequiredCheckPending,
    RequiredCheckFailed,
    RequiredCheckMissing,
    RequiredReviewMissing,
    UnresolvedAutomationFeedback,
}
