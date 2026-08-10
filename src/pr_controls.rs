use std::collections::BTreeSet;

use url::Url;

pub const WORK_PLAN_MARKER: &str = "<!-- pitools:work-plan:v1 -->";
pub const FINAL_SUMMARY_MARKER_PREFIX: &str = "<!-- pitools:final-summary:v1:";
pub const MAX_GITHUB_MARKDOWN_BYTES: usize = 60_000;

const MAX_ITEMS: usize = 100;
const MAX_ITEM_ID_BYTES: usize = 64;
const MAX_TEXT_BYTES: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

impl ItemStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    id: String,
    summary: String,
    status: ItemStatus,
}

impl PlanItem {
    pub fn new(
        id: impl Into<String>,
        summary: impl Into<String>,
        status: ItemStatus,
    ) -> Result<Self, ContractError> {
        let id = id.into();
        let summary = summary.into();
        validate_identifier("item id", &id)?;
        validate_safe_text("item summary", &summary)?;
        Ok(Self {
            id,
            summary,
            status,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn status(&self) -> ItemStatus {
        self.status
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkPlanComment {
    check_run_url: Url,
    items: Vec<PlanItem>,
}

impl WorkPlanComment {
    pub fn new(
        check_run_url: impl AsRef<str>,
        items: Vec<PlanItem>,
    ) -> Result<Self, ContractError> {
        validate_items(&items)?;
        let check_run_url = validate_https_url(check_run_url.as_ref())?;
        Ok(Self {
            check_run_url,
            items,
        })
    }

    pub fn render(&self) -> Result<String, ContractError> {
        let mut markdown = format!(
            "{WORK_PLAN_MARKER}\n## PiTools work plan\n\n[Open Check Run]({})\n\n",
            self.check_run_url
        );
        if self.items.is_empty() {
            markdown.push_str("No work items.\n");
        } else {
            for item in &self.items {
                markdown.push_str(&format!(
                    "- `{}` — {} — {}\n",
                    item.id,
                    item.status.label(),
                    escape_markdown(&item.summary)
                ));
            }
        }
        markdown.push_str(
            "\nUse the Check Run controls to approve planned work when requested, skip the current item, or cancel the run.\n",
        );
        ensure_render_bound(markdown)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalSummary {
    run_id: String,
    changes: Vec<String>,
    tests: Vec<String>,
    remaining_blockers: Vec<String>,
}

impl FinalSummary {
    pub fn new<C, T, B, CS, TS, BS>(
        run_id: impl Into<String>,
        changes: C,
        tests: T,
        remaining_blockers: B,
    ) -> Result<Self, ContractError>
    where
        C: IntoIterator<Item = CS>,
        T: IntoIterator<Item = TS>,
        B: IntoIterator<Item = BS>,
        CS: Into<String>,
        TS: Into<String>,
        BS: Into<String>,
    {
        let run_id = run_id.into();
        validate_identifier("run id", &run_id)?;
        let changes = collect_safe_lines("change", changes)?;
        let tests = collect_safe_lines("test", tests)?;
        let remaining_blockers = collect_safe_lines("remaining blocker", remaining_blockers)?;
        Ok(Self {
            run_id,
            changes,
            tests,
            remaining_blockers,
        })
    }

    pub fn render(&self) -> Result<String, ContractError> {
        let mut markdown = format!(
            "{FINAL_SUMMARY_MARKER_PREFIX}{} -->\n## PiTools final summary\n",
            self.run_id
        );
        render_section(&mut markdown, "Changes", &self.changes);
        render_section(&mut markdown, "Tests", &self.tests);
        render_section(
            &mut markdown,
            "Remaining blockers",
            &self.remaining_blockers,
        );
        ensure_render_bound(markdown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlAction {
    ApprovePlan,
    SkipCurrentItem,
    CancelRun,
}

impl ControlAction {
    pub fn parse(value: &str) -> Result<Self, ControlError> {
        match value {
            "approve-plan" => Ok(Self::ApprovePlan),
            "skip-current-item" => Ok(Self::SkipCurrentItem),
            "cancel-run" => Ok(Self::CancelRun),
            _ => Err(ControlError::UnknownAction),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlRequest {
    request_id: String,
    actor_login: String,
    action: ControlAction,
}

impl ControlRequest {
    pub fn new(
        request_id: impl Into<String>,
        actor_login: impl Into<String>,
        action: ControlAction,
    ) -> Result<Self, ContractError> {
        let request_id = request_id.into();
        let actor_login = actor_login.into();
        validate_identifier("control request id", &request_id)?;
        validate_identifier("actor login", &actor_login)?;
        Ok(Self {
            request_id,
            actor_login,
            action,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    WaitingApproval,
    Running,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunState {
    status: RunStatus,
    items: Vec<PlanItem>,
    applied_request_ids: BTreeSet<String>,
}

impl RunState {
    pub fn new(mut items: Vec<PlanItem>) -> Result<Self, ContractError> {
        validate_items(&items)?;
        let status = if let Some(item) = items
            .iter_mut()
            .find(|item| item.status == ItemStatus::Pending)
        {
            item.status = ItemStatus::InProgress;
            RunStatus::Running
        } else if items
            .iter()
            .any(|item| item.status == ItemStatus::InProgress)
        {
            RunStatus::Running
        } else {
            RunStatus::Completed
        };
        Ok(Self {
            status,
            items,
            applied_request_ids: BTreeSet::new(),
        })
    }

    pub fn waiting_approval(items: Vec<PlanItem>) -> Result<Self, ContractError> {
        validate_items(&items)?;
        Ok(Self {
            status: RunStatus::WaitingApproval,
            items,
            applied_request_ids: BTreeSet::new(),
        })
    }

    pub fn status(&self) -> RunStatus {
        self.status
    }

    pub fn items(&self) -> &[PlanItem] {
        &self.items
    }
}

pub fn is_authorized(actor_login: &str, pr_author: &str, maintainers: &[String]) -> bool {
    !actor_login.is_empty()
        && ((!pr_author.is_empty() && actor_login.eq_ignore_ascii_case(pr_author))
            || maintainers.iter().any(|maintainer| {
                !maintainer.is_empty() && actor_login.eq_ignore_ascii_case(maintainer)
            }))
}

pub fn apply_control(
    state: &RunState,
    request: &ControlRequest,
    pr_author: &str,
    maintainers: &[String],
) -> Result<RunState, ControlError> {
    if !is_authorized(&request.actor_login, pr_author, maintainers) {
        return Err(ControlError::Unauthorized);
    }
    if state.applied_request_ids.contains(&request.request_id) {
        return Err(ControlError::DuplicateAction);
    }
    let mut updated = state.clone();
    match request.action {
        ControlAction::ApprovePlan => {
            if updated.status != RunStatus::WaitingApproval {
                return Err(ControlError::ApprovalNotApplicable);
            }
            let next = updated
                .items
                .iter_mut()
                .find(|item| item.status == ItemStatus::Pending)
                .ok_or(ControlError::RunNotActive)?;
            next.status = ItemStatus::InProgress;
            updated.status = RunStatus::Running;
        }
        ControlAction::SkipCurrentItem => {
            let waiting_for_approval = updated.status == RunStatus::WaitingApproval;
            if !waiting_for_approval && updated.status != RunStatus::Running {
                return Err(ControlError::RunNotActive);
            }
            let expected_status = if waiting_for_approval {
                ItemStatus::Pending
            } else {
                ItemStatus::InProgress
            };
            let current = updated
                .items
                .iter_mut()
                .find(|item| item.status == expected_status)
                .ok_or(ControlError::NoCurrentItem)?;
            current.status = ItemStatus::Skipped;
            if let Some(next) = updated
                .items
                .iter_mut()
                .find(|item| item.status == ItemStatus::Pending)
            {
                if !waiting_for_approval {
                    next.status = ItemStatus::InProgress;
                }
            } else {
                updated.status = RunStatus::Completed;
            }
        }
        ControlAction::CancelRun => {
            if !matches!(
                updated.status,
                RunStatus::WaitingApproval | RunStatus::Running
            ) {
                return Err(ControlError::RunNotActive);
            }
            for item in &mut updated.items {
                if matches!(item.status, ItemStatus::Pending | ItemStatus::InProgress) {
                    item.status = ItemStatus::Cancelled;
                }
            }
            updated.status = RunStatus::Cancelled;
        }
    }
    updated
        .applied_request_ids
        .insert(request.request_id.clone());
    Ok(updated)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ContractError {
    #[error("invalid {field}: {reason}")]
    Invalid { field: &'static str, reason: String },
    #[error("rendered GitHub Markdown exceeds {MAX_GITHUB_MARKDOWN_BYTES} bytes")]
    MarkdownTooLarge,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ControlError {
    #[error("unknown control action")]
    UnknownAction,
    #[error("actor is not authorized for this pull request")]
    Unauthorized,
    #[error("control request was already applied")]
    DuplicateAction,
    #[error("run is not active")]
    RunNotActive,
    #[error("run is not waiting for approval")]
    ApprovalNotApplicable,
    #[error("run has no current item")]
    NoCurrentItem,
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > MAX_ITEM_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(ContractError::Invalid {
            field,
            reason: "must be 1-64 ASCII letters, digits, '.', '_' or '-'".into(),
        });
    }
    Ok(())
}

fn validate_safe_text(field: &'static str, value: &str) -> Result<(), ContractError> {
    if value.trim().is_empty() || value.len() > MAX_TEXT_BYTES || value.contains(['\r', '\0']) {
        return Err(ContractError::Invalid {
            field,
            reason: "must be non-empty, single-line-safe text of at most 1000 bytes".into(),
        });
    }
    let normalized = value.to_ascii_lowercase();
    const SECRET_INDICATORS: [&str; 6] = [
        "github_pat_",
        "ghp_",
        "sk-proj-",
        "-----begin private key",
        "authorization: bearer",
        "x-access-token:",
    ];
    if SECRET_INDICATORS
        .iter()
        .any(|indicator| normalized.contains(indicator))
    {
        return Err(ContractError::Invalid {
            field,
            reason: "contains secret-like material".into(),
        });
    }
    Ok(())
}

fn validate_items(items: &[PlanItem]) -> Result<(), ContractError> {
    if items.len() > MAX_ITEMS {
        return Err(ContractError::Invalid {
            field: "plan items",
            reason: "cannot contain more than 100 entries".into(),
        });
    }
    let mut ids = BTreeSet::new();
    if items.iter().any(|item| !ids.insert(item.id.as_str())) {
        return Err(ContractError::Invalid {
            field: "plan items",
            reason: "item ids must be unique".into(),
        });
    }
    Ok(())
}

fn validate_https_url(value: &str) -> Result<Url, ContractError> {
    let url = Url::parse(value).map_err(|_| ContractError::Invalid {
        field: "Check Run URL",
        reason: "must be an absolute HTTPS URL".into(),
    })?;
    if url.scheme() != "https" || url.host_str().is_none() || url.username() != "" {
        return Err(ContractError::Invalid {
            field: "Check Run URL",
            reason: "must be an absolute HTTPS URL without user information".into(),
        });
    }
    Ok(url)
}

fn collect_safe_lines<I, S>(field: &'static str, values: I) -> Result<Vec<String>, ContractError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let values: Vec<String> = values.into_iter().map(Into::into).collect();
    if values.len() > MAX_ITEMS {
        return Err(ContractError::Invalid {
            field,
            reason: "cannot contain more than 100 entries".into(),
        });
    }
    for value in &values {
        validate_safe_text(field, value)?;
    }
    Ok(values)
}

fn render_section(markdown: &mut String, heading: &str, values: &[String]) {
    markdown.push_str(&format!("\n## {heading}\n\n"));
    if values.is_empty() {
        markdown.push_str("- None.\n");
    } else {
        for value in values {
            markdown.push_str(&format!("- {}\n", escape_markdown(value)));
        }
    }
}

fn escape_markdown(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\\' | '*' | '[' | ']' | '`' => {
                escaped.push('\\');
                escaped.push(character);
            }
            '\n' | '\t' => escaped.push(' '),
            character if character.is_control() => {}
            character => escaped.push(character),
        }
    }
    escaped
}

fn ensure_render_bound(markdown: String) -> Result<String, ContractError> {
    if markdown.len() > MAX_GITHUB_MARKDOWN_BYTES {
        Err(ContractError::MarkdownTooLarge)
    } else {
        Ok(markdown)
    }
}
