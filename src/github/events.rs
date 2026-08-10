use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::{PullRequestSnapshot, PullRequestState};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedbackRecord {
    pub id: String,
    pub actor_login: String,
    pub actor_type: String,
    pub body: String,
    pub resolved: bool,
    pub is_automation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckRecord {
    pub external_id: String,
    pub name: String,
    pub app_id: Option<i64>,
    pub status: String,
    pub conclusion: Option<String>,
    pub details_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryEnvelope {
    pub delivery_id: String,
    pub event_name: String,
    pub action: Option<String>,
    pub installation_id: Option<i64>,
    pub repository_id: Option<i64>,
    pub pull_request_number: Option<i32>,
    pub payload: Value,
    pub raw_body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRunControl {
    pub check_run_id: i64,
    pub action: String,
    pub actor_login: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessLifecycle {
    SuspendInstallation,
    ResumeInstallation,
    DeleteInstallation,
    RemoveRepositories,
    AddRepositories,
}

impl DeliveryEnvelope {
    pub fn from_payload(
        delivery_id: String,
        event_name: String,
        payload: Value,
        raw_body: Vec<u8>,
    ) -> Self {
        let action = payload
            .get("action")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let installation_id = payload.pointer("/installation/id").and_then(Value::as_i64);
        let repository_id = payload.pointer("/repository/id").and_then(Value::as_i64);
        let pull_request_number = payload
            .pointer("/pull_request/number")
            .or_else(|| payload.pointer("/issue/number"))
            .or_else(|| payload.pointer("/check_run/pull_requests/0/number"))
            .or_else(|| payload.pointer("/check_suite/pull_requests/0/number"))
            .or_else(|| payload.pointer("/workflow_run/pull_requests/0/number"))
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok());
        Self {
            delivery_id,
            event_name,
            action,
            installation_id,
            repository_id,
            pull_request_number,
            payload,
            raw_body,
        }
    }

    pub fn is_supported(&self) -> bool {
        matches!(
            self.event_name.as_str(),
            "installation"
                | "installation_repositories"
                | "pull_request"
                | "pull_request_review"
                | "pull_request_review_comment"
                | "issue_comment"
                | "check_run"
                | "check_suite"
                | "workflow_run"
                | "status"
        )
    }

    pub fn access_lifecycle(&self) -> Option<AccessLifecycle> {
        match (self.event_name.as_str(), self.action.as_deref()) {
            ("installation", Some("suspend")) => Some(AccessLifecycle::SuspendInstallation),
            ("installation", Some("unsuspend")) => Some(AccessLifecycle::ResumeInstallation),
            ("installation", Some("deleted")) => Some(AccessLifecycle::DeleteInstallation),
            ("installation_repositories", Some("removed")) => {
                Some(AccessLifecycle::RemoveRepositories)
            }
            ("installation_repositories", Some("added")) => Some(AccessLifecycle::AddRepositories),
            _ => None,
        }
    }

    pub fn head_sha(&self) -> Option<&str> {
        self.payload
            .get("sha")
            .and_then(Value::as_str)
            .or_else(|| {
                self.payload
                    .pointer("/workflow_run/head_sha")
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                self.payload
                    .pointer("/check_run/head_sha")
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                self.payload
                    .pointer("/check_suite/head_sha")
                    .and_then(Value::as_str)
            })
    }

    pub fn pull_request_snapshot(&self) -> Result<Option<PullRequestSnapshot>, EventParseError> {
        if self.event_name != "pull_request" {
            return Ok(None);
        }
        let pull_request = self
            .payload
            .get("pull_request")
            .ok_or(EventParseError::Missing("pull_request"))?;
        let repository_id = self
            .repository_id
            .ok_or(EventParseError::Missing("repository.id"))?;
        let github_id = required_i64(pull_request, "id")?;
        let number = self
            .pull_request_number
            .ok_or(EventParseError::Missing("pull_request.number"))?;
        let title = required_string(pull_request, "title")?;
        let url = pull_request
            .get("html_url")
            .or_else(|| pull_request.get("url"))
            .and_then(Value::as_str)
            .ok_or(EventParseError::Missing("pull_request.html_url"))?
            .to_owned();
        let merged = pull_request
            .get("merged")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let state = if merged {
            PullRequestState::Merged
        } else {
            match required_string(pull_request, "state")?.as_str() {
                "open" => PullRequestState::Open,
                "closed" => PullRequestState::Closed,
                other => {
                    return Err(EventParseError::Invalid {
                        field: "pull_request.state",
                        value: other.to_owned(),
                    });
                }
            }
        };
        let author_login = pull_request
            .pointer("/user/login")
            .and_then(Value::as_str)
            .ok_or(EventParseError::Missing("pull_request.user.login"))?
            .to_owned();
        let updated_at = parse_timestamp(pull_request, "updated_at")?;
        Ok(Some(PullRequestSnapshot {
            repository_id,
            github_id,
            number,
            title,
            url,
            state,
            draft: pull_request
                .get("draft")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            merged,
            head_sha: required_nested_string(pull_request, "head", "sha")?,
            base_sha: required_nested_string(pull_request, "base", "sha")?,
            head_branch: required_nested_string(pull_request, "head", "ref")?,
            base_branch: required_nested_string(pull_request, "base", "ref")?,
            author_login,
            updated_at,
        }))
    }

    pub fn check_run_control(&self) -> Result<Option<CheckRunControl>, EventParseError> {
        if self.event_name != "check_run" || self.action.as_deref() != Some("requested_action") {
            return Ok(None);
        }
        let check_run_id = self
            .payload
            .pointer("/check_run/id")
            .and_then(Value::as_i64)
            .ok_or(EventParseError::Missing("check_run.id"))?;
        let action = self
            .payload
            .pointer("/requested_action/identifier")
            .and_then(Value::as_str)
            .ok_or(EventParseError::Missing("requested_action.identifier"))?
            .to_owned();
        let actor_login = self
            .payload
            .pointer("/sender/login")
            .and_then(Value::as_str)
            .ok_or(EventParseError::Missing("sender.login"))?
            .to_owned();
        Ok(Some(CheckRunControl {
            check_run_id,
            action,
            actor_login,
        }))
    }

    pub fn feedback_record(&self) -> Result<Option<FeedbackRecord>, EventParseError> {
        let (prefix, id_path, body_path, actor_path) = match self.event_name.as_str() {
            "issue_comment" if self.payload.pointer("/issue/pull_request").is_some() => (
                "issue-comment",
                "/comment/id",
                "/comment/body",
                "/comment/user",
            ),
            "pull_request_review" => ("review", "/review/id", "/review/body", "/review/user"),
            "pull_request_review_comment" => (
                "review-comment",
                "/comment/id",
                "/comment/body",
                "/comment/user",
            ),
            _ => return Ok(None),
        };
        let id = self
            .payload
            .pointer(id_path)
            .and_then(Value::as_i64)
            .ok_or(EventParseError::Missing("feedback.id"))?;
        let body = self
            .payload
            .pointer(body_path)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let actor_login = self
            .payload
            .pointer(&format!("{actor_path}/login"))
            .and_then(Value::as_str)
            .ok_or(EventParseError::Missing("feedback.user.login"))?
            .to_owned();
        let actor_type = self
            .payload
            .pointer(&format!("{actor_path}/type"))
            .and_then(Value::as_str)
            .unwrap_or("User")
            .to_owned();
        Ok(Some(FeedbackRecord {
            id: format!("{prefix}:{id}"),
            actor_login,
            actor_type: actor_type.clone(),
            body,
            resolved: self.action.as_deref() == Some("dismissed"),
            is_automation: actor_type == "Bot",
            path: self
                .payload
                .pointer("/comment/path")
                .and_then(Value::as_str)
                .map(str::to_owned),
            start_line: self
                .payload
                .pointer("/comment/start_line")
                .and_then(Value::as_u64)
                .and_then(|line| u32::try_from(line).ok()),
            line: self
                .payload
                .pointer("/comment/line")
                .and_then(Value::as_u64)
                .and_then(|line| u32::try_from(line).ok()),
        }))
    }

    pub fn check_record(&self) -> Result<Option<CheckRecord>, EventParseError> {
        if self.event_name != "check_run" {
            return Ok(None);
        }
        let check_run = self
            .payload
            .get("check_run")
            .ok_or(EventParseError::Missing("check_run"))?;
        let external_id = check_run
            .get("id")
            .and_then(Value::as_i64)
            .ok_or(EventParseError::Missing("check_run.id"))?
            .to_string();
        let name = check_run
            .get("name")
            .and_then(Value::as_str)
            .ok_or(EventParseError::Missing("check_run.name"))?
            .to_owned();
        let status = check_run
            .get("status")
            .and_then(Value::as_str)
            .ok_or(EventParseError::Missing("check_run.status"))?
            .to_owned();
        Ok(Some(CheckRecord {
            external_id,
            name,
            app_id: check_run
                .get("app")
                .and_then(|app| app.get("id"))
                .and_then(Value::as_i64),
            status,
            conclusion: check_run
                .get("conclusion")
                .and_then(Value::as_str)
                .map(str::to_owned),
            details_url: check_run
                .get("details_url")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }))
    }
}

fn required_i64(value: &Value, field: &'static str) -> Result<i64, EventParseError> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .ok_or(EventParseError::Missing(field))
}

fn required_string(value: &Value, field: &'static str) -> Result<String, EventParseError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(EventParseError::Missing(field))
}

fn required_nested_string(
    value: &Value,
    object: &'static str,
    field: &'static str,
) -> Result<String, EventParseError> {
    value
        .get(object)
        .and_then(|value| value.get(field))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(EventParseError::Missing("pull_request.head/base field"))
}

fn parse_timestamp(value: &Value, field: &'static str) -> Result<DateTime<Utc>, EventParseError> {
    let source = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or(EventParseError::Missing(field))?;
    DateTime::parse_from_rfc3339(source)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| EventParseError::Invalid {
            field,
            value: source.to_owned(),
        })
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EventParseError {
    #[error("missing GitHub event field: {0}")]
    Missing(&'static str),
    #[error("invalid GitHub event field {field}: {value}")]
    Invalid { field: &'static str, value: String },
}
