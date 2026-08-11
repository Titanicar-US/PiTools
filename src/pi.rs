//! Typed, bounded JSONL boundary for the isolated TypeScript Pi worker.

use std::{
    collections::BTreeSet,
    path::{Component, Path},
    process::Stdio,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};
use uuid::Uuid;

use crate::ci::{CiPatch, validate_patch_set};

const MAX_PI_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_PI_INPUT_BYTES: usize = 256 * 1024;
pub const MAX_PI_SNAPSHOT_FILES: usize = 16;
pub const MAX_PI_SNAPSHOT_FILE_BYTES: usize = 16 * 1024;
pub const MAX_PI_SNAPSHOT_BYTES: usize = 48 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PiJobRequest {
    pub protocol_version: String,
    pub job_id: Uuid,
    pub repository: String,
    pub snapshot_path: String,
    pub snapshot_files: Vec<PiSnapshotFile>,
    pub allowed_paths: Vec<String>,
    pub failure_evidence: Option<String>,
    pub policy_revision: String,
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PiSnapshotFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PiJobResult {
    pub protocol_version: String,
    pub job_id: Uuid,
    pub nonce: String,
    pub diagnosis: String,
    pub confidence: f64,
    pub proposed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed_patches: Vec<CiPatch>,
    pub validation_commands: Vec<String>,
    pub risks: Vec<String>,
    pub requires_approval: bool,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct PiWorker {
    pub command: Vec<String>,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct NatsPiWorker {
    client: async_nats::Client,
    pub subject: String,
    pub timeout: Duration,
}

impl PiJobRequest {
    pub fn validate(&self) -> Result<(), PiError> {
        if self.protocol_version != "pitools.pi/v1"
            || self.repository.trim().is_empty()
            || self.snapshot_path.trim().is_empty()
            || self.policy_revision.trim().is_empty()
            || self.nonce.trim().is_empty()
        {
            return Err(PiError::InvalidRequest);
        }
        validate_relative_path(&self.snapshot_path)?;
        for path in &self.allowed_paths {
            validate_relative_path(path)?;
        }
        if self.snapshot_files.len() > MAX_PI_SNAPSHOT_FILES {
            return Err(PiError::InputTooLarge);
        }
        let mut snapshot_paths = BTreeSet::new();
        let mut snapshot_bytes: usize = 0;
        for file in &self.snapshot_files {
            validate_relative_path(&file.path)?;
            if !self.allowed_paths.iter().any(|path| path == &file.path)
                || !snapshot_paths.insert(file.path.clone())
            {
                return Err(PiError::PathNotAllowed);
            }
            if file.content.len() > MAX_PI_SNAPSHOT_FILE_BYTES {
                return Err(PiError::InputTooLarge);
            }
            snapshot_bytes = snapshot_bytes.saturating_add(file.content.len());
            if snapshot_bytes > MAX_PI_SNAPSHOT_BYTES {
                return Err(PiError::InputTooLarge);
            }
            if contains_secret_like(&file.content) {
                return Err(PiError::SecretLikeInput);
            }
        }
        if self
            .failure_evidence
            .as_deref()
            .is_some_and(contains_secret_like)
        {
            return Err(PiError::SecretLikeInput);
        }
        Ok(())
    }
}

impl PiWorker {
    pub async fn execute(&self, request: &PiJobRequest) -> Result<PiJobResult, PiError> {
        let (program, args) = self.command.split_first().ok_or(PiError::EmptyCommand)?;
        request.validate()?;
        if self
            .command
            .iter()
            .any(|part| part.contains(['\0', '\r', '\n']))
            || is_shell_program(program)
        {
            return Err(PiError::UnsafeCommand);
        }
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .env(
                "PATH",
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| PiError::Spawn(error.to_string()))?;
        let input = serde_json::to_vec(request)
            .map_err(|error| PiError::Serialization(error.to_string()))?;
        if input.len() > MAX_PI_INPUT_BYTES {
            return Err(PiError::InputTooLarge);
        }
        let mut stdin = child.stdin.take().ok_or(PiError::MissingPipe)?;
        tokio::io::AsyncWriteExt::write_all(&mut stdin, &input)
            .await
            .map_err(|error| PiError::Write(error.to_string()))?;
        tokio::io::AsyncWriteExt::write_all(&mut stdin, b"\n")
            .await
            .map_err(|error| PiError::Write(error.to_string()))?;
        drop(stdin);
        let stdout = child.stdout.take().ok_or(PiError::MissingPipe)?;
        let stderr = child.stderr.take().ok_or(PiError::MissingPipe)?;
        let result = timeout(self.timeout, async {
            let (output, errors) = tokio::join!(read_output(stdout), read_output(stderr));
            let status = child
                .wait()
                .await
                .map_err(|error| PiError::Wait(error.to_string()))?;
            Ok::<_, PiError>((status.success(), output?, errors?))
        })
        .await
        .map_err(|_| PiError::TimedOut)??;
        if result.1.len() > MAX_PI_OUTPUT_BYTES || result.2.len() > MAX_PI_OUTPUT_BYTES {
            return Err(PiError::OutputTooLarge);
        }
        if !result.0 {
            return Err(PiError::WorkerFailed(
                String::from_utf8_lossy(&result.2).into_owned(),
            ));
        }
        let value = result
            .1
            .split(|byte| *byte == b'\n')
            .find(|line| !line.is_empty())
            .ok_or(PiError::EmptyOutput)?;
        let parsed: PiJobResult = serde_json::from_slice(value)
            .map_err(|error| PiError::InvalidResult(error.to_string()))?;
        validate_result(request, &parsed)
    }
}

impl NatsPiWorker {
    pub fn new(client: async_nats::Client) -> Self {
        Self {
            client,
            subject: "pitools.pi.requests".into(),
            timeout: Duration::from_secs(10 * 60),
        }
    }

    pub async fn execute(&self, request: &PiJobRequest) -> Result<PiJobResult, PiError> {
        request.validate()?;
        let payload = serde_json::to_vec(request)
            .map_err(|error| PiError::Serialization(error.to_string()))?;
        if payload.len() > MAX_PI_INPUT_BYTES {
            return Err(PiError::InputTooLarge);
        }
        let message = tokio::time::timeout(
            self.timeout,
            self.client.request(self.subject.clone(), payload.into()),
        )
        .await
        .map_err(|_| PiError::TimedOut)?
        .map_err(|error| PiError::WorkerFailed(error.to_string()))?;
        if message.payload.len() > MAX_PI_OUTPUT_BYTES {
            return Err(PiError::OutputTooLarge);
        }
        let parsed: PiJobResult = serde_json::from_slice(&message.payload)
            .map_err(|error| PiError::InvalidResult(error.to_string()))?;
        validate_result(request, &parsed)
    }
}

fn validate_result(request: &PiJobRequest, parsed: &PiJobResult) -> Result<PiJobResult, PiError> {
    if parsed.protocol_version != request.protocol_version
        || parsed.job_id != request.job_id
        || parsed.nonce != request.nonce
        || !(0.0..=1.0).contains(&parsed.confidence)
    {
        return Err(PiError::BindingMismatch);
    }
    if parsed
        .proposed_files
        .iter()
        .any(|path| !request.allowed_paths.iter().any(|allowed| allowed == path))
    {
        return Err(PiError::PathNotAllowed);
    }
    if !parsed.proposed_patches.is_empty() && !parsed.requires_approval {
        return Err(PiError::ApprovalRequired);
    }
    if !parsed.proposed_patches.is_empty() {
        validate_patch_set(&parsed.proposed_patches, &request.allowed_paths)
            .map_err(|error| PiError::InvalidResult(error.to_string()))?;
    }
    let serialized =
        serde_json::to_string(parsed).map_err(|error| PiError::Serialization(error.to_string()))?;
    if contains_secret_like(&serialized) {
        return Err(PiError::SecretLikeOutput);
    }
    Ok(parsed.clone())
}

async fn read_output<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> Result<Vec<u8>, PiError> {
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| PiError::Read(error.to_string()))?;
    Ok(bytes)
}

fn validate_relative_path(value: &str) -> Result<(), PiError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || value.contains('\\')
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(PiError::InvalidPath(value.into()));
    }
    Ok(())
}

fn is_shell_program(program: &str) -> bool {
    matches!(
        Path::new(program)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("sh" | "bash" | "dash" | "zsh" | "fish" | "cmd" | "powershell" | "pwsh")
    )
}

fn contains_secret_like(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    if [
        "github_pat_",
        "ghp_",
        "ghs_",
        "gho_",
        "ghu_",
        "ghr_",
        "sk-proj-",
        "-----begin private key",
        "bearer ",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
        || normalized.contains("npm_")
        || normalized.contains("akia")
        || normalized.split_whitespace().any(looks_like_jwt)
        || [
            "token=",
            "token:",
            "password=",
            "password:",
            "secret=",
            "secret:",
            "private_key=",
            "private-key=",
            "api_key=",
            "api-key=",
        ]
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        return true;
    }
    false
}

fn looks_like_jwt(value: &str) -> bool {
    let mut segments = value.split('.');
    let Some(header) = segments.next() else {
        return false;
    };
    let Some(payload) = segments.next() else {
        return false;
    };
    let Some(signature) = segments.next() else {
        return false;
    };
    header.starts_with("eyj")
        && header.len() >= 8
        && payload.len() >= 8
        && signature.len() >= 8
        && segments.next().is_none()
}

#[derive(Debug, thiserror::Error)]
pub enum PiError {
    #[error("Pi worker command is empty")]
    EmptyCommand,
    #[error("Pi worker command contains unsafe control characters")]
    UnsafeCommand,
    #[error("Pi worker input is too large")]
    InputTooLarge,
    #[error("Pi worker output is too large")]
    OutputTooLarge,
    #[error("Pi worker failed to start: {0}")]
    Spawn(String),
    #[error("Pi worker failed to write input: {0}")]
    Write(String),
    #[error("Pi worker failed to read output: {0}")]
    Read(String),
    #[error("Pi worker failed to wait: {0}")]
    Wait(String),
    #[error("Pi worker timed out")]
    TimedOut,
    #[error("Pi worker pipe unavailable")]
    MissingPipe,
    #[error("Pi worker returned no output")]
    EmptyOutput,
    #[error("Pi worker returned invalid JSON: {0}")]
    InvalidResult(String),
    #[error("Pi worker result was not bound to its request")]
    BindingMismatch,
    #[error("Pi worker typed patches require explicit approval")]
    ApprovalRequired,
    #[error("Pi worker proposed a path outside the request allowlist")]
    PathNotAllowed,
    #[error("Pi worker output contained secret-like material")]
    SecretLikeOutput,
    #[error("Pi worker input contained secret-like material")]
    SecretLikeInput,
    #[error("Pi worker request is invalid")]
    InvalidRequest,
    #[error("Pi worker path is invalid: {0}")]
    InvalidPath(String),
    #[error("Pi worker failed: {0}")]
    WorkerFailed(String),
    #[error("Pi worker serialization failed: {0}")]
    Serialization(String),
}
