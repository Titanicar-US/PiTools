//! GitHub-facing job lifecycle notifications.

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    github::client::{
        CheckRunAction, CheckRunOutput, CheckRunRequest, CheckRunUpdate, GitHubClient,
    },
    policy::Policy,
    pr_controls::{FinalSummary, PlanItem, WorkPlanComment},
    queue::JobQueue,
    repository::{AuditEntry, Repositories},
};

pub struct WorkCoordinator {
    repositories: Repositories,
    github: GitHubClient,
}

impl WorkCoordinator {
    pub fn new(repositories: Repositories, github: GitHubClient) -> Self {
        Self {
            repositories,
            github,
        }
    }

    pub async fn start(
        &self,
        job_id: Uuid,
        owner: &str,
        repository: &str,
        pull_request_number: i32,
        head_sha: &str,
        items: Vec<PlanItem>,
    ) -> Result<WorkHandle, WorkflowError> {
        if let Some(existing) = self.repositories.existing_work_handle(job_id).await? {
            let handle = WorkHandle {
                job_id,
                check_run_id: existing.check_run_id,
                check_run_url: existing.check_run_url,
                comment_id: existing.comment_id,
            };
            self.repositories
                .link_job_check_run(
                    job_id,
                    handle.check_run_id,
                    &handle.check_run_url,
                    current_item_id(&items),
                )
                .await?;
            self.update_progress(&handle, owner, repository, items)
                .await?;
            return Ok(handle);
        }
        let check_run = self
            .github
            .create_check_run(
                owner,
                repository,
                &CheckRunRequest {
                    name: "PiTools work plan".into(),
                    head_sha: head_sha.into(),
                    status: "in_progress".into(),
                    conclusion: None,
                    details_url: None,
                    output: CheckRunOutput {
                        title: "PiTools is working".into(),
                        summary: "PiTools is preparing the displayed work plan. Use the requested actions to approve, skip the current item, or cancel the run.".into(),
                        text: None,
                    },
                    actions: vec![
                        CheckRunAction {
                            label: "Approve plan".into(),
                            description: "Approve the displayed plan and continue work".into(),
                            identifier: "approve-plan".into(),
                        },
                        CheckRunAction {
                            label: "Skip current item".into(),
                            description: "Skip this item and continue the plan".into(),
                            identifier: "skip-current-item".into(),
                        },
                        CheckRunAction {
                            label: "Cancel run".into(),
                            description: "Cancel all remaining work".into(),
                            identifier: "cancel-run".into(),
                        },
                    ],
                },
            )
            .await?;
        let check_run_url = check_run
            .html_url
            .clone()
            .ok_or(WorkflowError::MissingCheckRunUrl)?;
        let comment = WorkPlanComment::new(&check_run_url, items.clone())?;
        let body = comment.render()?;
        let github_comment = self
            .github
            .create_issue_comment(owner, repository, pull_request_number, &body)
            .await?;
        self.repositories
            .link_job_check_run(
                job_id,
                check_run.id,
                &check_run_url,
                current_item_id(&items),
            )
            .await?;
        self.repositories
            .upsert_living_comment(
                job_id,
                github_comment.id,
                crate::pr_controls::WORK_PLAN_MARKER,
                &body_hash(&body),
            )
            .await?;
        Ok(WorkHandle {
            job_id,
            check_run_id: check_run.id,
            check_run_url,
            comment_id: github_comment.id,
        })
    }

    pub async fn finish(
        &self,
        handle: &WorkHandle,
        owner: &str,
        repository: &str,
        pull_request_number: i32,
        summary: FinalSummary,
        succeeded: bool,
    ) -> Result<(), WorkflowError> {
        let body = summary.render()?;
        let comment = self
            .github
            .create_issue_comment(owner, repository, pull_request_number, &body)
            .await?;
        self.repositories
            .insert_final_comment(
                handle.job_id,
                comment.id,
                body.lines().next().unwrap_or("pitools:final-summary"),
                &body_hash(&body),
            )
            .await?;
        self.github
            .update_check_run(
                owner,
                repository,
                handle.check_run_id,
                &CheckRunUpdate {
                    name: None,
                    status: Some("completed".into()),
                    conclusion: Some(if succeeded { "success" } else { "failure" }.into()),
                    details_url: Some(handle.check_run_url.clone()),
                    output: Some(CheckRunOutput {
                        title: if succeeded {
                            "PiTools work completed".into()
                        } else {
                            "PiTools work stopped with blockers".into()
                        },
                        summary: body,
                        text: None,
                    }),
                    actions: Vec::new(),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn update_progress(
        &self,
        handle: &WorkHandle,
        owner: &str,
        repository: &str,
        items: Vec<PlanItem>,
    ) -> Result<(), WorkflowError> {
        self.update_progress_with_approval(handle, owner, repository, items, None)
            .await
    }

    pub async fn update_progress_with_approval(
        &self,
        handle: &WorkHandle,
        owner: &str,
        repository: &str,
        items: Vec<PlanItem>,
        approval_hash: Option<&str>,
    ) -> Result<(), WorkflowError> {
        let comment = WorkPlanComment::new(&handle.check_run_url, items)?;
        let comment = if let Some(approval_hash) = approval_hash {
            comment.with_approval_hash(approval_hash)?
        } else {
            comment
        };
        let body = comment.render()?;
        self.github
            .update_issue_comment(owner, repository, handle.comment_id, &body)
            .await?;
        self.repositories
            .upsert_living_comment(
                handle.job_id,
                handle.comment_id,
                crate::pr_controls::WORK_PLAN_MARKER,
                &body_hash(&body),
            )
            .await?;
        Ok(())
    }
}

fn current_item_id(items: &[PlanItem]) -> Option<&str> {
    items
        .iter()
        .find(|item| {
            matches!(
                item.status(),
                crate::pr_controls::ItemStatus::InProgress
                    | crate::pr_controls::ItemStatus::Pending
            )
        })
        .map(PlanItem::id)
}

pub async fn process_check_run_control(
    repositories: &Repositories,
    queue: &JobQueue,
    delivery: &crate::github::events::DeliveryEnvelope,
) -> Result<ControlDisposition, WorkflowError> {
    let Some(control) = delivery.check_run_control()? else {
        return Ok(ControlDisposition::NotAControlEvent);
    };
    let Some(context) = repositories.control_context(control.check_run_id).await? else {
        return Ok(ControlDisposition::UnknownCheckRun);
    };
    let policy = if context.policy_yaml.trim().is_empty() {
        Policy::default()
    } else {
        Policy::from_yaml(&context.policy_yaml)
            .map_err(|error| WorkflowError::Policy(error.to_string()))?
    };
    let action = match crate::pr_controls::ControlAction::parse(&control.action) {
        Ok(action) => action,
        Err(_) => {
            repositories
                .record_job_control(
                    context.job_id,
                    &delivery.delivery_id,
                    &control.action,
                    &control.actor_login,
                    None,
                    "rejected-unknown",
                )
                .await?;
            return Ok(ControlDisposition::RejectedUnknown);
        }
    };
    if !crate::pr_controls::is_authorized(
        &control.actor_login,
        &context.pr_author,
        &policy.maintainers,
    ) {
        repositories
            .record_job_control(
                context.job_id,
                &delivery.delivery_id,
                &control.action,
                &control.actor_login,
                None,
                "rejected-unauthorized",
            )
            .await?;
        return Ok(ControlDisposition::RejectedUnauthorized);
    }
    let inserted = repositories
        .record_job_control(
            context.job_id,
            &delivery.delivery_id,
            &control.action,
            &control.actor_login,
            None,
            "accepted",
        )
        .await?;
    if !inserted {
        return Ok(ControlDisposition::Duplicate);
    }
    match action {
        crate::pr_controls::ControlAction::ApprovePlan => {
            let changed = queue.approve_plan(context.job_id).await?;
            repositories
                .record_audit(AuditEntry {
                    repository_id: Some(context.repository_id),
                    pull_request_number: Some(context.pull_request_number),
                    job_id: Some(context.job_id),
                    actor_login: Some(control.actor_login.clone()),
                    event_type: "job_control".into(),
                    summary: "approve-plan requested".into(),
                    evidence: serde_json::json!({"changed": changed}),
                })
                .await?;
            Ok(if changed {
                ControlDisposition::Applied
            } else {
                ControlDisposition::RunNotActive
            })
        }
        crate::pr_controls::ControlAction::SkipCurrentItem => {
            let changed = queue.skip_current_item(context.job_id).await?;
            repositories
                .record_audit(AuditEntry {
                    repository_id: Some(context.repository_id),
                    pull_request_number: Some(context.pull_request_number),
                    job_id: Some(context.job_id),
                    actor_login: Some(control.actor_login.clone()),
                    event_type: "job_control".into(),
                    summary: "skip-current-item requested".into(),
                    evidence: serde_json::json!({"changed": changed}),
                })
                .await?;
            Ok(if changed {
                ControlDisposition::Applied
            } else {
                ControlDisposition::RunNotActive
            })
        }
        crate::pr_controls::ControlAction::CancelRun => {
            let changed = queue.request_cancel(context.job_id).await?;
            repositories
                .record_audit(AuditEntry {
                    repository_id: Some(context.repository_id),
                    pull_request_number: Some(context.pull_request_number),
                    job_id: Some(context.job_id),
                    actor_login: Some(control.actor_login.clone()),
                    event_type: "job_control".into(),
                    summary: "cancel-run requested".into(),
                    evidence: serde_json::json!({"changed": changed}),
                })
                .await?;
            Ok(if changed {
                ControlDisposition::Applied
            } else {
                ControlDisposition::RunNotActive
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlDisposition {
    NotAControlEvent,
    UnknownCheckRun,
    RejectedUnknown,
    RejectedUnauthorized,
    Duplicate,
    Applied,
    RunNotActive,
}

#[derive(Debug, Clone)]
pub struct WorkHandle {
    pub job_id: Uuid,
    pub check_run_id: i64,
    pub check_run_url: String,
    pub comment_id: i64,
}

fn body_hash(body: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(body.as_bytes())))
}

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("GitHub API error: {0}")]
    GitHub(#[from] crate::github::client::GitHubClientError),
    #[error("repository error: {0}")]
    Repository(#[from] crate::repository::RepositoryError),
    #[error("work-plan contract error: {0}")]
    Contract(#[from] crate::pr_controls::ContractError),
    #[error("policy error: {0}")]
    Policy(String),
    #[error("event parsing error: {0}")]
    Event(#[from] crate::github::events::EventParseError),
    #[error("queue error: {0}")]
    Queue(#[from] crate::queue::QueueError),
    #[error("GitHub Check Run did not return an HTTPS URL")]
    MissingCheckRunUrl,
}
