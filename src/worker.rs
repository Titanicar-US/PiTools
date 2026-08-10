//! Durable worker orchestration for GitHub reconciliation jobs.
//!
//! The worker owns the transition from a persisted webhook/job to a fresh
//! GitHub read. Mutating repair jobs are admitted only through typed plans;
//! unknown or incomplete plans are left as explicit failures rather than
//! being reported as successful work.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use serde_json::json;
use uuid::Uuid;

use crate::{
    ci::{
        CiMutationAdmission, admit_ci_mutation, is_repairable_check_conclusion, prepare_ci_evidence,
    },
    github::{
        auth::{GitHubAppAuth, InstallationTokenScope},
        client::{GitHubClient, actions_job_id_from_details_url},
    },
    models::{PullRequestSnapshot, PullRequestState},
    pi::{NatsPiWorker, PiJobRequest, PiJobResult},
    policy::Policy,
    pr_controls::{FinalSummary, ItemStatus, PlanItem},
    queue::{JobKind, JobQueue, JobSpec, LeasedJob},
    readiness,
    reconcile::reconcile_read_set,
    repository::{PullRequestContext, Repositories, RepositoryContext},
    stack::{StackPlanner, StackPullRequest},
    workflow::WorkCoordinator,
    workspace::{LeaseGuard, RepositoryWorkspace},
};

const POLL_INTERVAL: Duration = Duration::from_secs(5);

struct FeedbackRepairContext<'a> {
    repository: &'a RepositoryContext,
    pull_request: &'a PullRequestContext,
    policy: &'a Policy,
    lease: &'a LeaseGuard,
}

struct CiRepairContext<'a> {
    repository: &'a RepositoryContext,
    pull_request: &'a PullRequestContext,
    policy: &'a Policy,
    lease: &'a LeaseGuard,
}

enum FeedbackTarget {
    ReviewComment { comment_id: i64, thread_id: String },
    PullRequestConversation,
}

impl FeedbackTarget {
    fn reply_target(&self) -> crate::feedback::FeedbackReplyTarget {
        match self {
            Self::ReviewComment { .. } => crate::feedback::FeedbackReplyTarget::ReviewThread,
            Self::PullRequestConversation => {
                crate::feedback::FeedbackReplyTarget::PullRequestConversation
            }
        }
    }
}

#[derive(Clone)]
pub struct WorkerRuntime {
    queue: JobQueue,
    repositories: Repositories,
    github_auth: GitHubAppAuth,
    pi_worker: Option<NatsPiWorker>,
}

impl WorkerRuntime {
    pub fn new(
        queue: JobQueue,
        repositories: Repositories,
        github_auth: GitHubAppAuth,
        pi_worker: Option<NatsPiWorker>,
    ) -> Self {
        Self {
            queue,
            repositories,
            github_auth,
            pi_worker,
        }
    }

    pub async fn run(self) -> Result<()> {
        let worker_id = format!("worker-{}", Uuid::now_v7());
        tracing::info!(worker_id = %worker_id, "worker started; polling durable job queue");
        loop {
            if let Some(job) = self.queue.lease_next(&worker_id).await? {
                self.run_job(&worker_id, job).await;
            } else {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }

    async fn run_job(&self, worker_id: &str, job: LeasedJob) {
        let lease_lost = Arc::new(AtomicBool::new(false));
        let heartbeat_lost = Arc::clone(&lease_lost);
        let heartbeat_queue = self.queue.clone();
        let heartbeat_worker = worker_id.to_owned();
        let heartbeat_job = job.id;
        let heartbeat_token = job.lease_token;
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                match heartbeat_queue
                    .renew_lease(heartbeat_job, &heartbeat_worker, heartbeat_token)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        heartbeat_lost.store(true, Ordering::Release);
                        tracing::error!(job_id = %heartbeat_job, "job lease was lost");
                        break;
                    }
                    Err(error) => {
                        tracing::error!(job_id = %heartbeat_job, error = %error, "job lease heartbeat failed");
                    }
                }
            }
        });
        let result = self.execute_job(worker_id, &job).await;
        heartbeat.abort();
        if lease_lost.load(Ordering::Acquire) {
            tracing::error!(job_id = %job.id, "discarding job completion after lease loss");
            return;
        }
        match result {
            Ok(result) => {
                if result
                    .get("waiting_approval")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                {
                    tracing::info!(job_id = %job.id, "job is waiting for explicit approval");
                    return;
                }
                if let Err(error) = self
                    .queue
                    .mark_succeeded(job.id, worker_id, job.lease_token, result)
                    .await
                {
                    tracing::error!(job_id = %job.id, error = %error, "failed to mark job succeeded");
                }
            }
            Err(error) => {
                tracing::error!(job_id = %job.id, kind = %job.kind, error = %error, "job failed");
                if let Err(mark_error) = self
                    .queue
                    .mark_failed(
                        job.id,
                        worker_id,
                        job.lease_token,
                        json!({"error": error.to_string()}),
                    )
                    .await
                {
                    tracing::error!(job_id = %job.id, error = %mark_error, "failed to mark job failed");
                }
            }
        }
    }

    async fn execute_job(
        &self,
        worker_id: &str,
        job: &LeasedJob,
    ) -> Result<serde_json::Value, WorkerError> {
        self.ensure_lease(worker_id, job).await?;
        let repository = self
            .repositories
            .repository_context(job.repository_id)
            .await?
            .ok_or(WorkerError::RepositoryNotFound(job.repository_id))?;
        let pull_request = self
            .repositories
            .pull_request_context(job.repository_id, job.pull_request_number)
            .await?
            .ok_or(WorkerError::PullRequestNotFound {
                repository_id: job.repository_id,
                number: job.pull_request_number,
            })?;
        let mut permissions = BTreeMap::new();
        permissions.insert("actions".into(), "read".into());
        permissions.insert("checks".into(), "write".into());
        permissions.insert("contents".into(), "write".into());
        permissions.insert("issues".into(), "write".into());
        permissions.insert("metadata".into(), "read".into());
        permissions.insert("pull_requests".into(), "write".into());
        permissions.insert("statuses".into(), "read".into());
        let installation_token = self
            .github_auth
            .installation_token_with_scope(
                repository.installation_id,
                &InstallationTokenScope {
                    permissions,
                    repositories: vec![repository.name.clone()],
                },
            )
            .await?;
        let github = GitHubClient::new(self.github_auth.api_base(), installation_token.clone())?;
        let policy = self.load_policy(&github, &repository).await?;
        self.ensure_lease(worker_id, job).await?;
        let lease_guard = LeaseGuard::new(
            self.queue.clone(),
            job.id,
            worker_id.to_owned(),
            job.lease_token,
        );
        let coordinator = WorkCoordinator::new(self.repositories.clone(), github.clone());
        let handle = if job.kind != "reconcile" {
            let plan_items = plan_items_for_job(job, ItemStatus::InProgress)?;
            self.ensure_lease(worker_id, job).await?;
            Some(
                coordinator
                    .start(
                        job.id,
                        &repository.owner,
                        &repository.name,
                        job.pull_request_number,
                        &pull_request.head_sha,
                        plan_items,
                    )
                    .await?,
            )
        } else {
            None
        };

        let outcome = match job.kind.as_str() {
            "reconcile" => {
                self.reconcile(&github, &repository, &pull_request, &policy, job)
                    .await
            }
            "feedback_repair" => {
                let context = FeedbackRepairContext {
                    repository: &repository,
                    pull_request: &pull_request,
                    policy: &policy,
                    lease: &lease_guard,
                };
                if policy.require_approval
                    && !plan_is_approved(&job.plan)
                    && plan_has_remaining_feedback(&job.plan)
                {
                    Ok(waiting_for_approval(
                        "automation feedback repair is ready for review",
                        "approve the plan to apply the deterministic suggestion",
                    ))
                } else {
                    self.feedback_repair(
                        &github,
                        &context,
                        &job.plan,
                        installation_token.token.clone(),
                    )
                    .await
                }
            }
            "stack_rebase" => {
                if plan_item_is_skipped(&job.plan, "stack_rebase") {
                    Ok(skipped_item_result("stack rebase"))
                } else if policy.require_approval && !plan_is_approved(&job.plan) {
                    Ok(waiting_for_approval(
                        "a deterministic stack update is ready for review",
                        "approve the plan to apply safe base updates",
                    ))
                } else {
                    self.stack_rebase(
                        &github,
                        &repository,
                        &policy,
                        &lease_guard,
                        &job.plan,
                        installation_token.token.clone(),
                    )
                    .await
                }
            }
            "ci_repair" => {
                if plan_item_is_skipped(&job.plan, "ci_repair") {
                    Ok(skipped_item_result("CI repair"))
                } else {
                    let context = CiRepairContext {
                        repository: &repository,
                        pull_request: &pull_request,
                        policy: &policy,
                        lease: &lease_guard,
                    };
                    self.ci_repair(&github, &context, job, installation_token.token.clone())
                        .await
                }
            }
            other => Err(WorkerError::UnknownJobKind(other.to_owned())),
        };

        match outcome {
            Ok(result) => {
                if result
                    .get("waiting_approval")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                {
                    self.ensure_lease(worker_id, job).await?;
                    if let Some(handle) = handle.as_ref() {
                        coordinator
                            .update_progress(
                                handle,
                                &repository.owner,
                                &repository.name,
                                plan_items_for_job(job, ItemStatus::Pending)?,
                            )
                            .await?;
                    }
                    let details = result
                        .get("approval_details")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    if !self
                        .queue
                        .mark_waiting_approval(job.id, worker_id, job.lease_token, details)
                        .await?
                    {
                        return Err(WorkerError::LeaseLost);
                    }
                    return Ok(result);
                }
                self.ensure_lease(worker_id, job).await?;
                let completed = result
                    .get("completed")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true);
                if let Some(handle) = handle.as_ref() {
                    let summary = FinalSummary::new(
                        job.id.to_string(),
                        result_lines(&result, "changes"),
                        result_lines(&result, "tests"),
                        result_lines(&result, "remaining_blockers"),
                    )?;
                    coordinator
                        .finish(
                            handle,
                            &repository.owner,
                            &repository.name,
                            job.pull_request_number,
                            summary,
                            completed,
                        )
                        .await?;
                }
                Ok(result)
            }
            Err(error) => {
                if handle.is_none() {
                    return Err(error);
                }
                self.ensure_lease(worker_id, job).await?;
                let summary = FinalSummary::new(
                    job.id.to_string(),
                    std::iter::empty::<String>(),
                    std::iter::empty::<String>(),
                    [error.to_string()],
                )?;
                coordinator
                    .finish(
                        handle.as_ref().expect("error path has a work handle"),
                        &repository.owner,
                        &repository.name,
                        job.pull_request_number,
                        summary,
                        false,
                    )
                    .await?;
                Err(error)
            }
        }
    }

    async fn reconcile(
        &self,
        github: &GitHubClient,
        repository: &RepositoryContext,
        pull_request: &PullRequestContext,
        policy: &Policy,
        job: &LeasedJob,
    ) -> Result<serde_json::Value, WorkerError> {
        let data = github
            .read_pull_request(&repository.owner, &repository.name, job.pull_request_number)
            .await?;
        let mut reconciled = reconcile_read_set(&data, policy);
        for feedback in &mut reconciled.feedback {
            if self
                .repositories
                .feedback_resolution_eligible(&feedback.id)
                .await?
            {
                feedback.resolved = true;
                if let Some(snapshot) = reconciled
                    .readiness
                    .unresolved_feedback
                    .iter_mut()
                    .find(|snapshot| snapshot.id == feedback.id)
                {
                    snapshot.resolved = true;
                }
            }
        }
        let snapshot = PullRequestSnapshot {
            repository_id: pull_request.repository_id,
            github_id: pull_request.github_id,
            number: pull_request.number,
            title: pull_request.title.clone(),
            url: pull_request.url.clone(),
            state: if data.pull_request.merged.unwrap_or(false) {
                PullRequestState::Merged
            } else if data.pull_request.state.as_deref() == Some("closed") {
                PullRequestState::Closed
            } else {
                PullRequestState::Open
            },
            draft: data.pull_request.draft,
            merged: data.pull_request.merged.unwrap_or(false),
            head_sha: data.pull_request.head.sha.clone(),
            base_sha: data.pull_request.base.sha.clone(),
            head_branch: data
                .pull_request
                .head
                .reference
                .clone()
                .unwrap_or_else(|| pull_request.head_branch.clone()),
            base_branch: data
                .pull_request
                .base
                .reference
                .clone()
                .unwrap_or_else(|| pull_request.base_branch.clone()),
            author_login: pull_request.author_login.clone(),
            updated_at: chrono::Utc::now(),
        };
        let readiness = readiness::evaluate(&reconciled.readiness, policy);
        self.repositories
            .save_reconciliation(
                &snapshot,
                &readiness,
                &reconciled.feedback,
                &reconciled.checks,
            )
            .await?;
        self.enqueue_follow_up_jobs(repository, pull_request, policy, &reconciled)
            .await?;
        Ok(json!({
            "completed": true,
            "ready": readiness.ready,
            "reason_codes": readiness.reason_codes,
            "changes": ["updated pull request, feedback, checks, and readiness state"],
            "tests": ["GitHub read set fetched and readiness evaluated"],
            "feedback_count": reconciled.feedback.len(),
            "check_count": reconciled.checks.len(),
        }))
    }

    async fn feedback_repair(
        &self,
        github: &GitHubClient,
        context: &FeedbackRepairContext<'_>,
        plan: &serde_json::Value,
        token: secrecy::SecretString,
    ) -> Result<serde_json::Value, WorkerError> {
        let Some(feedback) = plan
            .get("feedback")
            .and_then(serde_json::Value::as_array)
            .and_then(|items| {
                items.iter().enumerate().find_map(|(index, item)| {
                    (!plan_item_is_skipped(plan, &format!("feedback-{index}")))
                        .then_some(item.clone())
                })
            })
        else {
            return Ok(json!({
                "completed": true,
                "changes": ["skipped all planned feedback items"],
                "tests": [],
                "remaining_blockers": [],
            }));
        };
        let feedback: crate::github::events::FeedbackRecord = serde_json::from_value(feedback)
            .map_err(|error| WorkerError::PlanSerialization(error.to_string()))?;
        let current_pull_request = github
            .get_pull_request(
                &context.repository.owner,
                &context.repository.name,
                context.pull_request.number,
            )
            .await?;
        let expected_repository =
            format!("{}/{}", context.repository.owner, context.repository.name);
        let head_repository = current_pull_request
            .head
            .repository
            .as_ref()
            .and_then(|repository| repository.full_name.as_deref())
            .ok_or_else(|| {
                WorkerError::MutationAdmissionRequired(
                    "pull request head repository identity is unavailable".into(),
                )
            })?;
        if head_repository != expected_repository {
            return Err(WorkerError::MutationAdmissionRequired(
                "fork pull request repairs are disabled until head-repository write scope is configured".into(),
            ));
        }
        if current_pull_request.head.sha != context.pull_request.head_sha
            || current_pull_request.head.reference.as_deref()
                != Some(context.pull_request.head_branch.as_str())
        {
            return Err(WorkerError::MutationAdmissionRequired(
                "pull request head changed since the repair plan was created".into(),
            ));
        }
        let comment_target = crate::feedback::parse_feedback_comment_target(&feedback.id)
            .map_err(|error| WorkerError::PlanSerialization(error.to_string()))?;
        let target = match comment_target {
            crate::feedback::FeedbackCommentTarget::ReviewComment { comment_id } => {
                let thread_id = github
                    .review_thread_id_for_comment(
                        &context.repository.owner,
                        &context.repository.name,
                        context.pull_request.number,
                        comment_id,
                    )
                    .await?
                    .ok_or(WorkerError::ReviewThreadNotFound(comment_id))?;
                FeedbackTarget::ReviewComment {
                    comment_id,
                    thread_id,
                }
            }
            crate::feedback::FeedbackCommentTarget::PullRequestConversation { .. } => {
                FeedbackTarget::PullRequestConversation
            }
        };
        let workspace = RepositoryWorkspace::clone_branch(
            &context.repository.owner,
            &context.repository.name,
            &context.pull_request.head_branch,
            &context.pull_request.head_sha,
            token,
        )
        .await?;
        let feedback_id = feedback.id.clone();
        context.lease.ensure().await?;
        let outcome = match workspace
            .apply_feedback(
                context.lease,
                &context.policy.automation_actors,
                &crate::feedback::Feedback {
                    actor_login: feedback.actor_login,
                    actor_type: feedback.actor_type,
                    is_automation: feedback.is_automation,
                    body: feedback.body,
                    path: feedback.path,
                    start_line: feedback.start_line,
                    line: feedback.line,
                },
                &context.pull_request.head_branch,
                &context.pull_request.head_sha,
                &context.policy.validation_commands,
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) if is_deterministic_feedback_rejection(&error) => {
                self.publish_feedback_outcome(
                    github,
                    context,
                    &target,
                    &crate::feedback::render_feedback_reply(
                        crate::feedback::FeedbackReply::Rejected {
                            target: target.reply_target(),
                        },
                    ),
                )
                .await?;
                self.repositories
                    .mark_feedback_rejected(&feedback_id)
                    .await?;
                return Ok(json!({
                    "completed": true,
                    "changes": [match target {
                        FeedbackTarget::ReviewComment { .. } => "rejected automated feedback after deterministic validation".to_owned(),
                        FeedbackTarget::PullRequestConversation => "rejected automated feedback and posted a PR-level outcome comment".to_owned(),
                    }],
                    "tests": [],
                    "remaining_blockers": ["human follow-up remains required for the rejected automation suggestion"],
                    "feedback_disposition": "rejected",
                    "resolution_eligible": false,
                }));
            }
            Err(error) => return Err(error.into()),
        };
        self.publish_feedback_outcome(
            github,
            context,
            &target,
            &crate::feedback::render_feedback_reply(crate::feedback::FeedbackReply::Applied {
                target: target.reply_target(),
                path: &outcome.path,
                commit: &outcome.commit,
            }),
        )
        .await?;
        self.repositories
            .mark_feedback_resolved(&feedback_id)
            .await?;
        let change = match target {
            FeedbackTarget::ReviewComment { .. } => {
                format!("applied automated feedback to {}", outcome.path)
            }
            FeedbackTarget::PullRequestConversation => format!(
                "applied automated feedback to {} and posted a PR-level outcome comment",
                outcome.path
            ),
        };
        Ok(json!({
            "completed": true,
            "changes": [change],
            "tests": ["configured validation commands passed before push"],
            "commit": outcome.commit,
            "resolution_eligible": true,
        }))
    }

    async fn publish_feedback_outcome(
        &self,
        github: &GitHubClient,
        context: &FeedbackRepairContext<'_>,
        target: &FeedbackTarget,
        body: &str,
    ) -> Result<(), WorkerError> {
        context.lease.ensure().await?;
        match target {
            FeedbackTarget::ReviewComment { comment_id, .. } => {
                github
                    .reply_to_review_comment(
                        &context.repository.owner,
                        &context.repository.name,
                        context.pull_request.number,
                        *comment_id,
                        body,
                    )
                    .await?;
            }
            FeedbackTarget::PullRequestConversation => {
                github
                    .create_issue_comment(
                        &context.repository.owner,
                        &context.repository.name,
                        context.pull_request.number,
                        body,
                    )
                    .await?;
            }
        }
        context.lease.ensure().await?;
        if let FeedbackTarget::ReviewComment { thread_id, .. } = target {
            github.resolve_review_thread(thread_id).await?;
        }
        Ok(())
    }

    async fn ci_repair(
        &self,
        github: &GitHubClient,
        context: &CiRepairContext<'_>,
        job: &LeasedJob,
        token: secrecy::SecretString,
    ) -> Result<serde_json::Value, WorkerError> {
        let pi_result = if let Some(value) = job.plan.get("pi_result") {
            serde_json::from_value::<PiJobResult>(value.clone())
                .map_err(|error| WorkerError::PlanSerialization(error.to_string()))?
        } else {
            self.pi_diagnosis(github, context.repository, context.policy, job)
                .await?
        };
        if pi_result.proposed_patches.is_empty() {
            return Ok(json!({
                "completed": false,
                "changes": ["received a bounded CI diagnosis"],
                "tests": [],
                "remaining_blockers": [
                    "Pi did not return a typed patch; no CI mutation was attempted"
                ],
                "diagnosis": pi_result.diagnosis,
                "confidence": pi_result.confidence,
                "proposed_files": pi_result.proposed_files,
                "validation_commands": pi_result.validation_commands,
                "risks": pi_result.risks,
                "requires_approval": true,
            }));
        }
        match admit_ci_mutation(
            pi_result.proposed_patches.len(),
            pi_result.requires_approval,
            plan_is_approved(&job.plan),
        )
        .map_err(|error| WorkerError::MutationAdmissionRequired(error.to_string()))?
        {
            CiMutationAdmission::NoMutation => {
                return Err(WorkerError::MutationAdmissionRequired(
                    "CI mutation admission was requested without a typed patch".into(),
                ));
            }
            CiMutationAdmission::WaitingApproval => {
                let serialized = serde_json::to_value(&pi_result)
                    .map_err(|error| WorkerError::PiSerialization(error.to_string()))?;
                return Ok(json!({
                    "completed": false,
                    "waiting_approval": true,
                    "approval_details": {"pi_result": serialized},
                    "changes": ["received a typed CI repair proposal"],
                    "tests": [],
                    "remaining_blockers": [
                        "an authorized reviewer must approve the typed patch before it is applied"
                    ],
                    "diagnosis": pi_result.diagnosis,
                    "confidence": pi_result.confidence,
                    "proposed_files": pi_result.proposed_files,
                    "validation_commands": pi_result.validation_commands,
                    "risks": pi_result.risks,
                    "requires_approval": true,
                }));
            }
            CiMutationAdmission::Admitted => {}
        }

        let current_pull_request = github
            .get_pull_request(
                &context.repository.owner,
                &context.repository.name,
                context.pull_request.number,
            )
            .await?;
        let expected_repository =
            format!("{}/{}", context.repository.owner, context.repository.name);
        let head_repository = current_pull_request
            .head
            .repository
            .as_ref()
            .and_then(|repository| repository.full_name.as_deref())
            .ok_or_else(|| {
                WorkerError::MutationAdmissionRequired(
                    "pull request head repository identity is unavailable".into(),
                )
            })?;
        if head_repository != expected_repository {
            return Err(WorkerError::MutationAdmissionRequired(
                "fork pull request repairs are disabled until head-repository write scope is configured".into(),
            ));
        }
        if current_pull_request.head.sha != context.pull_request.head_sha
            || current_pull_request.head.reference.as_deref()
                != Some(context.pull_request.head_branch.as_str())
        {
            return Err(WorkerError::MutationAdmissionRequired(
                "pull request head changed since the repair plan was created".into(),
            ));
        }
        let workspace = RepositoryWorkspace::clone_branch(
            &context.repository.owner,
            &context.repository.name,
            &context.pull_request.head_branch,
            &context.pull_request.head_sha,
            token,
        )
        .await?;
        context.lease.ensure().await?;
        let outcome = workspace
            .apply_ci_repair(
                context.lease,
                &pi_result.proposed_patches,
                &context.policy.repair_allowed_paths,
                &context.pull_request.head_branch,
                &context.pull_request.head_sha,
                &context.policy.validation_commands,
            )
            .await?;
        Ok(json!({
            "completed": true,
            "changes": outcome
                .paths
                .iter()
                .map(|path| format!("applied typed CI repair to {path}"))
                .collect::<Vec<_>>(),
            "tests": ["configured validation commands passed before push"],
            "commit": outcome.commit,
            "diagnosis": pi_result.diagnosis,
            "confidence": pi_result.confidence,
        }))
    }

    async fn enqueue_follow_up_jobs(
        &self,
        repository: &RepositoryContext,
        pull_request: &PullRequestContext,
        policy: &Policy,
        reconciled: &crate::reconcile::ReconciledPullRequest,
    ) -> Result<(), WorkerError> {
        let automation_feedback: Vec<_> = reconciled
            .feedback
            .iter()
            .filter(|feedback| {
                !feedback.resolved && policy.is_configured_automation_actor(&feedback.actor_login)
            })
            .cloned()
            .collect();
        if !automation_feedback.is_empty() {
            self.queue
                .enqueue(JobSpec {
                    repository_id: repository.id,
                    pull_request_number: pull_request.number,
                    kind: JobKind::FeedbackRepair,
                    plan: json!({
                        "source": "reconcile",
                        "feedback": automation_feedback,
                        "policy_revision": policy.revision(),
                    }),
                })
                .await?;
        }

        let failed_checks: Vec<_> = reconciled
            .checks
            .iter()
            .filter(|check| {
                check
                    .conclusion
                    .as_deref()
                    .is_some_and(is_repairable_check_conclusion)
            })
            .cloned()
            .collect();
        if !failed_checks.is_empty() {
            self.queue
                .enqueue(JobSpec {
                    repository_id: repository.id,
                    pull_request_number: pull_request.number,
                    kind: JobKind::CiRepair,
                    plan: json!({
                        "source": "reconcile",
                        "checks": failed_checks,
                        "head_sha": pull_request.head_sha.clone(),
                        "policy_revision": policy.revision(),
                    }),
                })
                .await?;
        }

        let contexts = self
            .repositories
            .open_pull_request_contexts(repository.id)
            .await?;
        let stack_pull_requests: Vec<_> = contexts
            .iter()
            .map(|context| StackPullRequest {
                number: context.number,
                head_branch: context.head_branch.clone(),
                base_branch: context.base_branch.clone(),
                explicit_parent: policy.stack_parents.get(&context.number).copied(),
            })
            .collect();
        if let Ok(stack_plan) = StackPlanner::plan(&stack_pull_requests)
            && stack_plan.merge_order.len() > 1
            && stack_plan.merge_order.last().copied() == Some(pull_request.number)
        {
            self.queue
                .enqueue(JobSpec {
                    repository_id: repository.id,
                    pull_request_number: pull_request.number,
                    kind: JobKind::StackRebase,
                    plan: json!({
                        "source": "reconcile",
                        "pull_requests": stack_pull_requests,
                        "stack_plan": stack_plan,
                        "head_sha": pull_request.head_sha.clone(),
                        "policy_revision": policy.revision(),
                    }),
                })
                .await?;
        }
        Ok(())
    }

    async fn stack_rebase(
        &self,
        github: &GitHubClient,
        repository: &RepositoryContext,
        policy: &Policy,
        lease: &LeaseGuard,
        plan: &serde_json::Value,
        token: secrecy::SecretString,
    ) -> Result<serde_json::Value, WorkerError> {
        let pull_requests = plan
            .get("pull_requests")
            .cloned()
            .ok_or_else(|| WorkerError::MutationAdmissionRequired("stack plan is empty".into()))?;
        let pull_requests: Vec<StackPullRequest> = serde_json::from_value(pull_requests)
            .map_err(|error| WorkerError::PlanSerialization(error.to_string()))?;
        let stack_plan = StackPlanner::plan(&pull_requests)
            .map_err(|error| WorkerError::MutationAdmissionRequired(error.to_string()))?;
        let mut pending_updates = Vec::new();
        for update in &stack_plan.required_base_updates {
            lease.ensure().await?;
            let current = github
                .get_pull_request(&repository.owner, &repository.name, update.pull_request)
                .await?;
            if current.state.as_deref() != Some("open") || current.merged == Some(true) {
                return Err(WorkerError::MutationAdmissionRequired(format!(
                    "pull request {} is no longer open",
                    update.pull_request
                )));
            }
            match current.base.reference.as_deref() {
                Some(base) if base == update.from_branch => pending_updates.push(update),
                Some(base) if base == update.to_branch => {}
                Some(base) => {
                    return Err(WorkerError::MutationAdmissionRequired(format!(
                        "pull request {} base changed from {} to {}",
                        update.pull_request, update.from_branch, base
                    )));
                }
                None => {
                    return Err(WorkerError::MutationAdmissionRequired(format!(
                        "pull request {} base branch is unavailable",
                        update.pull_request
                    )));
                }
            }
        }
        let mut changes =
            vec!["computed a deterministic base-most to tip-most stack order".to_owned()];
        for update in pending_updates {
            lease.ensure().await?;
            let updated = github
                .update_pull_request_base(
                    &repository.owner,
                    &repository.name,
                    update.pull_request,
                    &update.to_branch,
                )
                .await?;
            if updated.base.reference.as_deref() != Some(update.to_branch.as_str()) {
                return Err(WorkerError::MutationAdmissionRequired(format!(
                    "GitHub did not confirm base update for pull request {}",
                    update.pull_request
                )));
            }
            changes.push(format!(
                "updated pull request {} base from {} to {}",
                update.pull_request, update.from_branch, update.to_branch
            ));
        }
        let by_number: BTreeMap<i32, &StackPullRequest> = pull_requests
            .iter()
            .map(|pull_request| (pull_request.number, pull_request))
            .collect();
        let mut blockers = Vec::new();
        for (index, number) in stack_plan.merge_order.iter().enumerate() {
            let pull_request = by_number.get(number).ok_or_else(|| {
                WorkerError::MutationAdmissionRequired(format!(
                    "stack plan references missing pull request {number}"
                ))
            })?;
            let target_branch = if let Some(parent_number) = index
                .checked_sub(1)
                .and_then(|parent_index| stack_plan.merge_order.get(parent_index))
            {
                by_number
                    .get(parent_number)
                    .ok_or_else(|| {
                        WorkerError::MutationAdmissionRequired(format!(
                            "stack plan references missing parent pull request {parent_number}"
                        ))
                    })?
                    .head_branch
                    .clone()
            } else {
                pull_request.base_branch.clone()
            };
            lease.ensure().await?;
            let current = github
                .get_pull_request(&repository.owner, &repository.name, *number)
                .await?;
            if current.state.as_deref() != Some("open") || current.merged == Some(true) {
                blockers.push(format!("pull request {number} is no longer open"));
                break;
            }
            let expected_repository = format!("{}/{}", repository.owner, repository.name);
            let head_repository = current
                .head
                .repository
                .as_ref()
                .and_then(|repository| repository.full_name.as_deref());
            if head_repository != Some(expected_repository.as_str()) {
                blockers.push(format!(
                    "pull request {number} is a fork head; branch-history writes are disabled"
                ));
                break;
            }
            let branch = current.head.reference.as_deref().ok_or_else(|| {
                WorkerError::MutationAdmissionRequired(format!(
                    "pull request {number} head branch is unavailable"
                ))
            })?;
            if branch != pull_request.head_branch {
                blockers.push(format!(
                    "pull request {number} head branch changed from {} to {}",
                    pull_request.head_branch, branch
                ));
                break;
            }
            let current_base = current.base.reference.as_deref().ok_or_else(|| {
                WorkerError::MutationAdmissionRequired(format!(
                    "pull request {number} base branch is unavailable"
                ))
            })?;
            if current_base != target_branch {
                blockers.push(format!(
                    "pull request {number} base is {current_base}, expected {target_branch}"
                ));
                break;
            }
            let allow_force_push = policy.allow_bot_force_push
                && policy
                    .bot_owned_branches
                    .iter()
                    .any(|allowed| allowed == branch);
            let workspace = RepositoryWorkspace::clone_branch(
                &repository.owner,
                &repository.name,
                branch,
                &current.head.sha,
                token.clone(),
            )
            .await?;
            match workspace
                .rebase_onto(
                    lease,
                    branch,
                    &target_branch,
                    &current.head.sha,
                    allow_force_push,
                    &policy.validation_commands,
                )
                .await
            {
                Ok(outcome) if outcome.before == outcome.after => {
                    changes.push(format!(
                        "verified {branch} is already based on {target_branch}"
                    ));
                }
                Ok(outcome) => {
                    changes.push(format!(
                        "rebased {branch} onto {} and pushed {}",
                        outcome.target_branch,
                        if outcome.force_pushed {
                            "with force-with-lease"
                        } else {
                            "without history rewrite"
                        }
                    ));
                }
                Err(crate::workspace::WorkspaceError::RebaseRequiresForcePush {
                    branch,
                    target_branch,
                }) => {
                    blockers.push(format!(
                        "rebase of {branch} onto {target_branch} requires an explicitly authorized bot-owned branch rewrite"
                    ));
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }

        Ok(json!({
            "completed": blockers.is_empty(),
            "changes": changes,
            "tests": ["GitHub confirmed stack bases; branch rebases used exact remote-head leases"],
            "remaining_blockers": blockers,
            "merge_order": stack_plan.merge_order,
            "required_base_updates": stack_plan.required_base_updates,
        }))
    }

    async fn pi_diagnosis(
        &self,
        github: &GitHubClient,
        repository: &RepositoryContext,
        policy: &Policy,
        job: &LeasedJob,
    ) -> Result<PiJobResult, WorkerError> {
        let Some(pi_worker) = &self.pi_worker else {
            return Err(WorkerError::PiUnavailable);
        };
        let failure_evidence = self.ci_failure_evidence(github, repository, job).await?;
        let request = PiJobRequest {
            protocol_version: "pitools.pi/v1".into(),
            job_id: job.id,
            repository: format!("{}/{}", repository.owner, repository.name),
            snapshot_path: "snapshot.json".into(),
            allowed_paths: policy.repair_allowed_paths.clone(),
            failure_evidence: Some(failure_evidence),
            policy_revision: policy.revision(),
            nonce: Uuid::now_v7().to_string(),
        };
        let result = pi_worker
            .execute(&request)
            .await
            .map_err(|error| WorkerError::Pi(error.to_string()))?;
        Ok(result)
    }

    async fn ci_failure_evidence(
        &self,
        github: &GitHubClient,
        repository: &RepositoryContext,
        job: &LeasedJob,
    ) -> Result<String, WorkerError> {
        let mut check_outputs = Vec::new();
        if let Some(checks) = job.plan.get("checks").and_then(serde_json::Value::as_array) {
            for check in checks.iter().take(16) {
                let Some(external_id) =
                    check.get("external_id").and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                let Some(check_run_id) = external_id.parse::<i64>().ok() else {
                    continue;
                };
                match github
                    .get_check_run(&repository.owner, &repository.name, check_run_id)
                    .await
                {
                    Ok(check_run) => {
                        let output = check_run.output.map(|output| {
                            json!({
                                "title": output.title,
                                "summary": output.summary,
                                "text": output.text,
                            })
                        });
                        let annotations = match github
                            .list_check_run_annotations(
                                &repository.owner,
                                &repository.name,
                                check_run.id,
                            )
                            .await
                        {
                            Ok(annotations) => serde_json::to_value(annotations)
                                .unwrap_or_else(|_| serde_json::Value::Array(Vec::new())),
                            Err(error) => {
                                tracing::warn!(
                                    job_id = %job.id,
                                    check_run_id = check_run.id,
                                    error = %error,
                                    "failed to fetch check annotations; continuing with stored failure evidence"
                                );
                                serde_json::Value::Array(Vec::new())
                            }
                        };
                        let actions_log = if check_run
                            .conclusion
                            .as_deref()
                            .is_some_and(is_repairable_check_conclusion)
                        {
                            if let Some(actions_job_id) =
                                actions_job_id_from_details_url(check_run.details_url.as_deref())
                            {
                                match github
                                    .download_workflow_job_logs(
                                        &repository.owner,
                                        &repository.name,
                                        actions_job_id,
                                    )
                                    .await
                                {
                                    Ok(log) => Some(json!(log)),
                                    Err(error) => {
                                        tracing::warn!(
                                            job_id = %job.id,
                                            check_run_id = check_run.id,
                                            actions_job_id,
                                            error = %error,
                                            "failed to fetch Actions job logs; continuing with check-run evidence"
                                        );
                                        None
                                    }
                                }
                            } else {
                                tracing::debug!(
                                    job_id = %job.id,
                                    check_run_id = check_run.id,
                                    "check run has no Actions job details URL; skipping job logs"
                                );
                                None
                            }
                        } else {
                            None
                        };
                        check_outputs.push(json!({
                            "id": check_run.id,
                            "name": check_run.name,
                            "status": check_run.status,
                            "conclusion": check_run.conclusion,
                            "details_url": check_run.details_url,
                            "output": output,
                            "annotations": annotations,
                            "actions_log": actions_log,
                        }));
                    }
                    Err(error) => {
                        tracing::warn!(
                            job_id = %job.id,
                            check_run_id,
                            error = %error,
                            "failed to fetch check run output; continuing with stored failure evidence"
                        );
                    }
                }
            }
        }
        prepare_ci_evidence(&job.plan, &serde_json::Value::Array(check_outputs))
            .map_err(|error| WorkerError::PiSerialization(error.to_string()))
    }

    async fn ensure_lease(&self, worker_id: &str, job: &LeasedJob) -> Result<(), WorkerError> {
        if self
            .queue
            .assert_lease(job.id, worker_id, job.lease_token)
            .await?
        {
            Ok(())
        } else {
            Err(WorkerError::LeaseLost)
        }
    }

    async fn load_policy(
        &self,
        github: &GitHubClient,
        repository: &RepositoryContext,
    ) -> Result<Policy, WorkerError> {
        let source = github
            .get_repository_policy(
                &repository.owner,
                &repository.name,
                &repository.default_branch,
            )
            .await?;
        let policy = match source {
            Some(source) => parse_policy(&source)?,
            None => Policy::default(),
        };
        self.repositories
            .save_policy(repository.id, &policy)
            .await?;
        Ok(policy)
    }
}

fn parse_policy(source: &str) -> Result<Policy, WorkerError> {
    if source.trim().is_empty() {
        Ok(Policy::default())
    } else {
        Policy::from_yaml(source).map_err(|error| WorkerError::Policy(error.to_string()))
    }
}

fn is_deterministic_feedback_rejection(error: &crate::workspace::WorkspaceError) -> bool {
    match error {
        crate::workspace::WorkspaceError::Feedback(error) => {
            !matches!(error, crate::feedback::FeedbackError::Io(_))
        }
        crate::workspace::WorkspaceError::RepairNotApplied
        | crate::workspace::WorkspaceError::ValidationFailed { .. } => true,
        _ => false,
    }
}

fn plan_is_approved(plan: &serde_json::Value) -> bool {
    plan.get("approved")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn plan_item_is_skipped(plan: &serde_json::Value, item_id: &str) -> bool {
    plan.get("skipped_items")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .any(|item| item.as_str() == Some(item_id))
}

fn plan_has_remaining_feedback(plan: &serde_json::Value) -> bool {
    plan.get("feedback")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .enumerate()
                .any(|(index, _)| !plan_item_is_skipped(plan, &format!("feedback-{index}")))
        })
}

fn plan_items_for_job(
    job: &LeasedJob,
    first_status: ItemStatus,
) -> Result<Vec<PlanItem>, WorkerError> {
    if job.kind == "feedback_repair" {
        let feedback = job
            .plan
            .get("feedback")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut started = false;
        let mut items = Vec::new();
        for index in 0..feedback.len().min(100) {
            let item_id = format!("feedback-{index}");
            let status = if plan_item_is_skipped(&job.plan, &item_id) {
                ItemStatus::Skipped
            } else if !started {
                started = true;
                first_status
            } else {
                ItemStatus::Pending
            };
            items.push(PlanItem::new(
                item_id,
                "Process automated feedback",
                status,
            )?);
        }
        if !items.is_empty() {
            return Ok(items);
        }
    }
    Ok(vec![PlanItem::new(
        job.kind.as_str(),
        job_kind_summary(&job.kind),
        first_status,
    )?])
}

fn skipped_item_result(item: &str) -> serde_json::Value {
    json!({
        "completed": true,
        "changes": [format!("skipped {item} as requested")],
        "tests": [],
        "remaining_blockers": [],
    })
}

fn waiting_for_approval(summary: &str, blocker: &str) -> serde_json::Value {
    json!({
        "completed": false,
        "waiting_approval": true,
        "changes": [summary],
        "tests": [],
        "remaining_blockers": [blocker],
        "requires_approval": true,
    })
}

fn job_kind_summary(kind: &str) -> &'static str {
    match kind {
        "reconcile" => "Refresh GitHub state and evaluate merge readiness",
        "feedback_repair" => "Validate and apply an approved automation suggestion",
        "ci_repair" => "Diagnose a GitHub Actions failure and prepare a repair plan",
        "stack_rebase" => "Plan a safe pull request stack rebase",
        _ => "Process a PiTools work item",
    }
}

fn result_lines(value: &serde_json::Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, thiserror::Error)]
enum WorkerError {
    #[error("repository {0} was not found or is inactive")]
    RepositoryNotFound(i64),
    #[error("pull request {number} was not found in repository {repository_id}")]
    PullRequestNotFound { repository_id: i64, number: i32 },
    #[error("policy error: {0}")]
    Policy(String),
    #[error("mutation job {0:?} requires an admitted typed plan")]
    MutationAdmissionRequired(String),
    #[error("unknown job kind: {0}")]
    UnknownJobKind(String),
    #[error("repository error: {0}")]
    Repository(#[from] crate::repository::RepositoryError),
    #[error("GitHub authentication error: {0}")]
    GitHubAuth(#[from] crate::github::auth::GitHubAuthError),
    #[error("GitHub client error: {0}")]
    GitHub(#[from] crate::github::client::GitHubClientError),
    #[error("workflow error: {0}")]
    Workflow(#[from] crate::workflow::WorkflowError),
    #[error("work-plan contract error: {0}")]
    Contract(#[from] crate::pr_controls::ContractError),
    #[error("queue error: {0}")]
    Queue(#[from] crate::queue::QueueError),
    #[error("Pi worker transport is unavailable")]
    PiUnavailable,
    #[error("Pi worker serialization failed: {0}")]
    PiSerialization(String),
    #[error("Pi worker failed: {0}")]
    Pi(String),
    #[error("job lease was cancelled or lost before the next mutation")]
    LeaseLost,
    #[error("job plan serialization failed: {0}")]
    PlanSerialization(String),
    #[error("workspace error: {0}")]
    Workspace(#[from] crate::workspace::WorkspaceError),
    #[error("review comment {0} has no resolvable review thread")]
    ReviewThreadNotFound(i64),
}
