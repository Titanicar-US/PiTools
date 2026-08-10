//! Fail-closed GitHub Actions repair planning and bounded validation execution.

use std::{
    collections::BTreeSet,
    path::{Component, Path},
    process::Stdio,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};

use crate::policy::ValidationCommand;

pub const MAX_CI_LOG_BYTES: usize = 128 * 1024;
pub const MAX_COMMAND_OUTPUT_BYTES: usize = 256 * 1024;
pub const MAX_CI_PATCH_BYTES: usize = 128 * 1024;
pub const MAX_CI_PATCHES: usize = 32;

/// Return whether a GitHub check conclusion should trigger CI repair diagnosis.
pub fn is_repairable_check_conclusion(conclusion: &str) -> bool {
    matches!(
        conclusion,
        "action_required" | "cancelled" | "failure" | "startup_failure" | "stale" | "timed_out"
    )
}

/// Redact common credential-shaped values before CI evidence crosses into Pi.
pub fn redact_ci_text(value: &str) -> String {
    let mut lines = Vec::new();
    let mut in_pem = false;
    for original_line in value.lines() {
        let normalized_original = original_line.to_ascii_lowercase();
        if in_pem || normalized_original.contains("-----begin ") {
            let ends_pem = normalized_original.contains("-----end ");
            lines.push("[REDACTED]".to_owned());
            in_pem = !ends_pem;
            continue;
        }
        let mut line = original_line.to_owned();
        for marker in [
            "ghp_",
            "github_pat_",
            "ghs_",
            "gho_",
            "ghu_",
            "ghr_",
            "sk-proj-",
            "AKIA",
            "npm_",
            "eyJ",
        ] {
            redact_token_marker(&mut line, marker);
        }
        let normalized = line.to_ascii_lowercase();
        if normalized.contains("authorization: bearer")
            || normalized.contains("password=")
            || normalized.contains("token=")
            || normalized.contains("secret=")
            || normalized.contains("private_key=")
        {
            let separator = line.find(['=', ':']).unwrap_or(line.len());
            lines.push(format!(
                "{}[REDACTED]",
                &line[..=separator.min(line.len().saturating_sub(1))]
            ));
        } else {
            lines.push(line.to_owned());
        }
    }
    lines.join("\n")
}

/// Serialize the bounded CI evidence envelope that is safe to send to Pi.
pub fn prepare_ci_evidence(
    job_plan: &serde_json::Value,
    check_outputs: &serde_json::Value,
) -> Result<String, serde_json::Error> {
    let evidence = serde_json::json!({
        "job_plan": job_plan,
        "check_outputs": check_outputs,
    });
    let serialized = serde_json::to_string_pretty(&evidence)?;
    Ok(truncate_utf8(
        &redact_ci_text(&serialized),
        MAX_CI_LOG_BYTES.min(48 * 1024),
    ))
}

fn truncate_utf8(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut end = maximum;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn redact_token_marker(value: &mut String, marker: &str) {
    let mut search_from = 0;
    while let Some(relative) = value[search_from..].find(marker) {
        let start = search_from + relative;
        let end = value[start..]
            .find(char::is_whitespace)
            .map(|offset| start + offset)
            .unwrap_or(value.len());
        value.replace_range(start..end, "[REDACTED]");
        search_from = start + "[REDACTED]".len();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CiFailureKind {
    Test,
    Lint,
    Build,
    Dependency,
    Infrastructure,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CiFailure {
    pub repository_id: i64,
    pub pull_request_number: i32,
    pub head_sha: String,
    pub workflow_name: String,
    pub job_name: String,
    pub check_name: String,
    pub details_url: Option<String>,
    pub log_excerpt: String,
    pub kind: CiFailureKind,
}

impl CiFailure {
    pub fn new(input: CiFailureInput) -> Result<Self, CiError> {
        let CiFailureInput {
            repository_id,
            pull_request_number,
            head_sha,
            workflow_name,
            job_name,
            check_name,
            details_url,
            log_excerpt,
        } = input;
        let log_excerpt = bounded_text(log_excerpt, MAX_CI_LOG_BYTES)?;
        let check_name = bounded_text(check_name, 256)?;
        let job_name = bounded_text(job_name, 256)?;
        let workflow_name = bounded_text(workflow_name, 256)?;
        let head_sha = bounded_text(head_sha, 128)?;
        let kind = classify_failure(&workflow_name, &job_name, &check_name, &log_excerpt);
        Ok(Self {
            repository_id,
            pull_request_number,
            head_sha,
            workflow_name,
            job_name,
            check_name,
            details_url,
            log_excerpt,
            kind,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CiFailureInput {
    pub repository_id: i64,
    pub pull_request_number: i32,
    pub head_sha: String,
    pub workflow_name: String,
    pub job_name: String,
    pub check_name: String,
    pub details_url: Option<String>,
    pub log_excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CiRepairPlan {
    pub failure: CiFailure,
    pub worktree: WorktreePlan,
    pub validation_commands: Vec<ValidationCommand>,
    pub requires_approval: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiMutationAdmission {
    NoMutation,
    WaitingApproval,
    Admitted,
}

/// Enforce the service-level approval boundary for typed CI mutations.
///
/// Repository policy may add stricter gates, but it cannot disable this
/// approval requirement for a provider-produced patch.
pub fn admit_ci_mutation(
    patch_count: usize,
    requires_approval: bool,
    plan_approved: bool,
) -> Result<CiMutationAdmission, CiError> {
    if patch_count == 0 {
        return Ok(CiMutationAdmission::NoMutation);
    }
    if !requires_approval {
        return Err(CiError::ApprovalRequired);
    }
    if !plan_approved {
        return Ok(CiMutationAdmission::WaitingApproval);
    }
    Ok(CiMutationAdmission::Admitted)
}

/// A single-file unified diff proposed by the isolated Pi worker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CiPatch {
    pub path: String,
    pub unified_diff: String,
}

/// Validate patch metadata before any repository workspace is touched.
///
/// Patch application is performed by `git apply` only after this structural
/// check. Exact paths keep a provider response from widening its write scope.
pub fn validate_patch_set(patches: &[CiPatch], allowed_paths: &[String]) -> Result<(), CiError> {
    if patches.is_empty() {
        return Err(CiError::NoPatches);
    }
    if patches.len() > MAX_CI_PATCHES {
        return Err(CiError::TooManyPatches);
    }
    let allowed_paths: BTreeSet<&str> = allowed_paths.iter().map(String::as_str).collect();
    for patch in patches {
        validate_repository_path(&patch.path)?;
        if !allowed_paths.contains(patch.path.as_str()) {
            return Err(CiError::PathNotAllowed(patch.path.clone()));
        }
        if patch.unified_diff.is_empty()
            || patch.unified_diff.len() > MAX_CI_PATCH_BYTES
            || patch.unified_diff.contains(['\0', '\r'])
        {
            return Err(CiError::InvalidPatch(patch.path.clone()));
        }
        let old_header = format!("--- a/{}", patch.path);
        let new_header = format!("+++ b/{}", patch.path);
        let old_count = patch
            .unified_diff
            .lines()
            .filter(|line| *line == old_header)
            .count();
        let new_count = patch
            .unified_diff
            .lines()
            .filter(|line| *line == new_header)
            .count();
        if old_count != 1
            || new_count != 1
            || patch
                .unified_diff
                .lines()
                .filter(|line| line.starts_with("--- ") || line.starts_with("+++ "))
                .count()
                != 2
            || !patch
                .unified_diff
                .lines()
                .any(|line| line.starts_with("@@"))
        {
            return Err(CiError::InvalidPatch(patch.path.clone()));
        }
    }
    Ok(())
}

impl CiRepairPlan {
    pub fn new(
        failure: CiFailure,
        worktree: WorktreePlan,
        validation_commands: Vec<ValidationCommand>,
    ) -> Result<Self, CiError> {
        if validation_commands.is_empty() {
            return Err(CiError::NoValidationCommands);
        }
        Ok(Self {
            failure,
            worktree,
            validation_commands,
            requires_approval: true,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreePlan {
    pub root: String,
    pub branch: String,
    pub head_sha: String,
}

impl WorktreePlan {
    pub fn new(
        root: impl Into<String>,
        branch: impl Into<String>,
        head_sha: impl Into<String>,
    ) -> Result<Self, CiError> {
        let root = root.into();
        let branch = branch.into();
        let head_sha = head_sha.into();
        if root.trim().is_empty() || head_sha.trim().is_empty() {
            return Err(CiError::InvalidWorktree(
                "root and head_sha are required".into(),
            ));
        }
        validate_branch(&branch)?;
        Ok(Self {
            root,
            branch,
            head_sha,
        })
    }

    pub fn git_worktree_add_argv(&self, destination: &Path) -> Result<Vec<String>, CiError> {
        validate_relative_destination(destination)?;
        Ok(vec![
            "git".into(),
            "worktree".into(),
            "add".into(),
            "--detach".into(),
            destination.to_string_lossy().into_owned(),
            self.head_sha.clone(),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub argv: Vec<String>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub async fn run_validation_commands(
    worktree: &Path,
    commands: &[ValidationCommand],
    timeout_duration: Duration,
) -> Result<Vec<CommandResult>, CiError> {
    if commands.is_empty() {
        return Err(CiError::NoValidationCommands);
    }
    let mut results = Vec::with_capacity(commands.len());
    for command in commands {
        let argv = command
            .argv()
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let result = run_bounded_argv(worktree, &argv, timeout_duration, &[]).await?;
        let success = result.success;
        results.push(result);
        if !success {
            break;
        }
    }
    Ok(results)
}

/// Build the command used to run repository validation without the control
/// plane filesystem or network namespace.
///
/// The runtime image must provide bubblewrap at `/usr/bin/bwrap`. A missing
/// sandbox executable is intentionally a deployment failure, not a reason to
/// fall back to running repository code beside GitHub credentials.
pub fn validation_sandbox_argv(
    worktree: &Path,
    command: &[String],
) -> Result<Vec<String>, CiError> {
    if !worktree.is_absolute()
        || worktree.as_os_str().is_empty()
        || worktree.to_string_lossy().contains(['\0', '\r', '\n'])
    {
        return Err(CiError::InvalidWorktree(worktree.display().to_string()));
    }
    let (program, _) = command.split_first().ok_or(CiError::EmptyCommand)?;
    if command
        .iter()
        .any(|value| value.contains(['\0', '\r', '\n']))
    {
        return Err(CiError::UnsafeCommand);
    }

    let mut argv = vec![
        "/usr/bin/bwrap".to_owned(),
        "--die-with-parent".to_owned(),
        "--unshare-all".to_owned(),
        "--new-session".to_owned(),
        "--tmpfs".to_owned(),
        "/".to_owned(),
        "--dir".to_owned(),
        "/usr".to_owned(),
        "--ro-bind".to_owned(),
        "/usr".to_owned(),
        "/usr".to_owned(),
        "--symlink".to_owned(),
        "usr/bin".to_owned(),
        "/bin".to_owned(),
        "--symlink".to_owned(),
        "usr/sbin".to_owned(),
        "/sbin".to_owned(),
        "--dir".to_owned(),
        "/lib".to_owned(),
        "--ro-bind".to_owned(),
        "/lib".to_owned(),
        "/lib".to_owned(),
        "--dir".to_owned(),
        "/lib64".to_owned(),
        "--ro-bind".to_owned(),
        "/lib64".to_owned(),
        "/lib64".to_owned(),
        "--dir".to_owned(),
        "/etc".to_owned(),
        "--ro-bind".to_owned(),
        "/etc".to_owned(),
        "/etc".to_owned(),
        "--dev".to_owned(),
        "/dev".to_owned(),
        "--proc".to_owned(),
        "/proc".to_owned(),
        "--tmpfs".to_owned(),
        "/tmp".to_owned(),
        "--dir".to_owned(),
        "/tmp/home".to_owned(),
        "--dir".to_owned(),
        "/workspace".to_owned(),
        "--bind".to_owned(),
        worktree.to_string_lossy().into_owned(),
        "/workspace".to_owned(),
        "--chdir".to_owned(),
        "/workspace".to_owned(),
        "--setenv".to_owned(),
        "CI".to_owned(),
        "1".to_owned(),
        "--setenv".to_owned(),
        "HOME".to_owned(),
        "/tmp/home".to_owned(),
        "--setenv".to_owned(),
        "TMPDIR".to_owned(),
        "/tmp".to_owned(),
        "--setenv".to_owned(),
        "PATH".to_owned(),
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_owned(),
        "--setenv".to_owned(),
        "GIT_TERMINAL_PROMPT".to_owned(),
        "0".to_owned(),
        "--setenv".to_owned(),
        "GIT_CONFIG_NOSYSTEM".to_owned(),
        "1".to_owned(),
        "--unshare-net".to_owned(),
        "--".to_owned(),
        program.clone(),
    ];
    argv.extend(command.iter().skip(1).cloned());
    Ok(argv)
}

/// Run configured validation in a credential-free, network-isolated sandbox.
pub async fn run_validation_commands_sandboxed(
    worktree: &Path,
    commands: &[ValidationCommand],
    timeout_duration: Duration,
) -> Result<Vec<CommandResult>, CiError> {
    if commands.is_empty() {
        return Err(CiError::NoValidationCommands);
    }
    let mut results = Vec::with_capacity(commands.len());
    for command in commands {
        let argv = command
            .argv()
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let sandbox_argv = validation_sandbox_argv(worktree, &argv)?;
        let mut result =
            run_bounded_argv(Path::new("/"), &sandbox_argv, timeout_duration, &[]).await?;
        result.argv = argv;
        let success = result.success;
        results.push(result);
        if !success {
            break;
        }
    }
    Ok(results)
}

/// Run a fixed, internally generated argv with a scrubbed environment.
///
/// Callers must pass an allowlisted executable and arguments. This function
/// deliberately does not invoke a shell or inherit the worker environment.
pub async fn run_bounded_argv(
    worktree: &Path,
    argv: &[String],
    timeout_duration: Duration,
    environment: &[(&str, &str)],
) -> Result<CommandResult, CiError> {
    run_bounded_argv_with_optional_input(worktree, argv, timeout_duration, environment, None).await
}

/// Run a fixed argv with bounded stdin and a scrubbed environment.
pub async fn run_bounded_argv_with_input(
    worktree: &Path,
    argv: &[String],
    timeout_duration: Duration,
    environment: &[(&str, &str)],
    input: &[u8],
) -> Result<CommandResult, CiError> {
    run_bounded_argv_with_optional_input(worktree, argv, timeout_duration, environment, Some(input))
        .await
}

async fn run_bounded_argv_with_optional_input(
    worktree: &Path,
    argv: &[String],
    timeout_duration: Duration,
    environment: &[(&str, &str)],
    input: Option<&[u8]>,
) -> Result<CommandResult, CiError> {
    let (program, args) = argv.split_first().ok_or(CiError::EmptyCommand)?;
    if argv.iter().any(|value| value.contains(['\0', '\r', '\n'])) {
        return Err(CiError::UnsafeCommand);
    }
    let mut child = Command::new(program);
    child
        .args(args)
        .current_dir(worktree)
        .env_clear()
        .env("CI", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        )
        .envs(environment.iter().copied())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = child
        .spawn()
        .map_err(|error| CiError::Spawn(error.to_string()))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().ok_or(CiError::MissingPipe)?;
    let stderr = child.stderr.take().ok_or(CiError::MissingPipe)?;
    let result = timeout(timeout_duration, async move {
        let write = async {
            if let Some(input) = input {
                let mut stdin = stdin.ok_or(CiError::MissingPipe)?;
                tokio::io::AsyncWriteExt::write_all(&mut stdin, input)
                    .await
                    .map_err(|error| CiError::WriteInput(error.to_string()))?;
            }
            Ok::<_, CiError>(())
        };
        let read = async { tokio::join!(read_limited(stdout), read_limited(stderr)) };
        let (write, (stdout, stderr)) = tokio::join!(write, read);
        write?;
        let status = child
            .wait()
            .await
            .map_err(|error| CiError::Wait(error.to_string()))?;
        Ok::<_, CiError>((status.success(), stdout?, stderr?))
    })
    .await
    .map_err(|_| CiError::TimedOut)??;
    Ok(CommandResult {
        argv: argv.to_vec(),
        success: result.0,
        stdout: String::from_utf8_lossy(&result.1).into_owned(),
        stderr: String::from_utf8_lossy(&result.2).into_owned(),
    })
}

async fn read_limited<R: AsyncRead + Unpin>(reader: R) -> Result<Vec<u8>, CiError> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_COMMAND_OUTPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| CiError::ReadOutput(error.to_string()))?;
    if bytes.len() > MAX_COMMAND_OUTPUT_BYTES {
        return Err(CiError::OutputTooLarge);
    }
    Ok(bytes)
}

fn classify_failure(workflow: &str, job: &str, check: &str, log: &str) -> CiFailureKind {
    let haystack = format!("{workflow} {job} {check} {log}").to_ascii_lowercase();
    if haystack.contains("dependabot") || haystack.contains("dependency") {
        CiFailureKind::Dependency
    } else if haystack.contains("clippy")
        || haystack.contains("lint")
        || haystack.contains("format")
    {
        CiFailureKind::Lint
    } else if haystack.contains("test") || haystack.contains("spec") {
        CiFailureKind::Test
    } else if haystack.contains("build") || haystack.contains("compile") {
        CiFailureKind::Build
    } else if haystack.contains("runner")
        || haystack.contains("timeout")
        || haystack.contains("network")
    {
        CiFailureKind::Infrastructure
    } else {
        CiFailureKind::Unknown
    }
}

fn bounded_text(value: String, maximum: usize) -> Result<String, CiError> {
    if value.len() > maximum || value.contains(['\0', '\r']) {
        return Err(CiError::InputTooLarge);
    }
    Ok(value)
}

fn validate_branch(branch: &str) -> Result<(), CiError> {
    if branch.is_empty()
        || branch.starts_with('-')
        || branch.contains(['\0', '\r', '\n', ' '])
        || branch.contains("..")
    {
        return Err(CiError::InvalidBranch(branch.into()));
    }
    Ok(())
}

fn validate_relative_destination(path: &Path) -> Result<(), CiError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(CiError::InvalidWorktree(path.display().to_string()));
    }
    Ok(())
}

fn validate_repository_path(path: &str) -> Result<(), CiError> {
    if path.is_empty()
        || path.contains(['\\', '\0', '\r', '\n'])
        || Path::new(path).is_absolute()
        || !Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(CiError::InvalidPatch(path.into()));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum CiError {
    #[error("typed CI mutations require explicit approval")]
    ApprovalRequired,
    #[error("no validation commands were configured")]
    NoValidationCommands,
    #[error("command has no executable")]
    EmptyCommand,
    #[error("command contains unsafe control characters")]
    UnsafeCommand,
    #[error("worktree path is invalid: {0}")]
    InvalidWorktree(String),
    #[error("branch name is invalid: {0}")]
    InvalidBranch(String),
    #[error("CI input is too large or contains unsupported line endings")]
    InputTooLarge,
    #[error("command output exceeded the configured limit")]
    OutputTooLarge,
    #[error("failed to spawn validation command: {0}")]
    Spawn(String),
    #[error("failed to wait for validation command: {0}")]
    Wait(String),
    #[error("failed to read validation output: {0}")]
    ReadOutput(String),
    #[error("validation command timed out")]
    TimedOut,
    #[error("validation command pipe unavailable")]
    MissingPipe,
    #[error("failed to write command input: {0}")]
    WriteInput(String),
    #[error("CI repair did not contain a patch")]
    NoPatches,
    #[error("CI repair contained too many patches")]
    TooManyPatches,
    #[error("CI repair proposed a path outside the allowlist: {0}")]
    PathNotAllowed(String),
    #[error("CI repair patch is invalid for path: {0}")]
    InvalidPatch(String),
}
