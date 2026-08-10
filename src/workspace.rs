//! Ephemeral repository workspaces for approved deterministic repairs.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use secrecy::{ExposeSecret, SecretString};
use uuid::Uuid;

use crate::{
    ci::{
        CiPatch, run_bounded_argv, run_bounded_argv_with_input, run_validation_commands,
        validate_patch_set,
    },
    feedback::{Feedback, RepairDecision, repair_feedback},
    queue::JobQueue,
};

const WORKSPACE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug)]
pub struct RepositoryWorkspace {
    root: PathBuf,
    repository_root: PathBuf,
    askpass: PathBuf,
    username: String,
    password: SecretString,
}

impl RepositoryWorkspace {
    pub async fn clone_branch(
        owner: &str,
        repository: &str,
        branch: &str,
        head_sha: &str,
        token: SecretString,
    ) -> Result<Self, WorkspaceError> {
        validate_segment(owner, "owner")?;
        validate_segment(repository, "repository")?;
        validate_branch(branch)?;
        validate_sha(head_sha)?;

        let root = std::env::temp_dir().join(format!("pitools-work-{}", Uuid::now_v7()));
        fs::create_dir(&root)?;
        let repository_root = root.join("repo");
        // Use the already-installed pitools executable as the askpass helper.
        // This keeps authentication working when /tmp is deliberately mounted
        // noexec and avoids writing an executable secret helper into a
        // workspace.
        let askpass = std::env::current_exe()?;
        let workspace = Self {
            root,
            repository_root,
            askpass,
            username: "x-access-token".into(),
            password: token,
        };
        let clone_url = format!("https://github.com/{owner}/{repository}.git");
        let destination = workspace.repository_root.to_string_lossy().into_owned();
        workspace
            .run(
                &workspace.root,
                &[
                    "git".into(),
                    "clone".into(),
                    "--no-checkout".into(),
                    "--filter=blob:none".into(),
                    "--single-branch".into(),
                    "--branch".into(),
                    branch.into(),
                    clone_url,
                    destination,
                ],
            )
            .await?;
        workspace
            .run(
                &workspace.repository_root,
                &[
                    "git".into(),
                    "fetch".into(),
                    "--no-tags".into(),
                    "origin".into(),
                    head_sha.into(),
                ],
            )
            .await?;
        workspace
            .run(
                &workspace.repository_root,
                &[
                    "git".into(),
                    "checkout".into(),
                    "--detach".into(),
                    head_sha.into(),
                ],
            )
            .await?;
        workspace.ensure_clean().await?;
        Ok(workspace)
    }

    pub async fn apply_feedback(
        &self,
        lease: &LeaseGuard,
        actor_allowlist: &[String],
        feedback: &Feedback,
        branch: &str,
        validation_commands: &[crate::policy::ValidationCommand],
    ) -> Result<RepairCommit, WorkspaceError> {
        self.ensure_clean().await?;
        lease.ensure().await?;
        let outcome = repair_feedback(
            &self.repository_root,
            actor_allowlist,
            feedback,
            RepairDecision::Apply,
        )?;
        if !outcome.resolution_eligible {
            return Err(WorkspaceError::RepairNotApplied);
        }
        let path = outcome
            .path
            .clone()
            .ok_or(WorkspaceError::RepairNotApplied)?;
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &["git".into(), "add".into(), "--".into(), path],
        )
        .await?;
        let validation = run_validation_commands(
            &self.repository_root,
            validation_commands,
            WORKSPACE_TIMEOUT,
        )
        .await?;
        if let Some(failed) = validation.iter().find(|result| !result.success) {
            return Err(WorkspaceError::ValidationFailed {
                argv: failed.argv.clone(),
                stderr: failed.stderr.clone(),
            });
        }
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "config".into(),
                "user.name".into(),
                "PiTools[bot]".into(),
            ],
        )
        .await?;
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "config".into(),
                "user.email".into(),
                "pitools[bot]@users.noreply.github.com".into(),
            ],
        )
        .await?;
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "commit".into(),
                "-m".into(),
                "fix: apply automated review suggestion".into(),
            ],
        )
        .await?;
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "push".into(),
                "origin".into(),
                format!("HEAD:refs/heads/{branch}"),
            ],
        )
        .await?;
        let commit = self
            .run(
                &self.repository_root,
                &["git".into(), "rev-parse".into(), "HEAD".into()],
            )
            .await?;
        Ok(RepairCommit {
            commit: commit.stdout.trim().to_owned(),
            path: outcome.path.unwrap_or_default(),
        })
    }

    pub async fn apply_ci_repair(
        &self,
        lease: &LeaseGuard,
        patches: &[CiPatch],
        allowed_paths: &[String],
        branch: &str,
        validation_commands: &[crate::policy::ValidationCommand],
    ) -> Result<CiRepairCommit, WorkspaceError> {
        validate_patch_set(patches, allowed_paths)
            .map_err(|error| WorkspaceError::CiPatch(error.to_string()))?;
        self.ensure_clean().await?;
        for patch in patches {
            lease.ensure().await?;
            self.run_patch_command(
                lease,
                &[
                    "git",
                    "apply",
                    "--check",
                    "--whitespace=error",
                    "--recount",
                    "-",
                ],
                patch.unified_diff.as_bytes(),
            )
            .await?;
            lease.ensure().await?;
            self.run_patch_command(
                lease,
                &["git", "apply", "--whitespace=error", "--recount", "-"],
                patch.unified_diff.as_bytes(),
            )
            .await?;
        }
        let paths = patches
            .iter()
            .map(|patch| patch.path.clone())
            .collect::<Vec<_>>();
        lease.ensure().await?;
        let mut add_argv = vec!["git".to_owned(), "add".to_owned(), "--".to_owned()];
        add_argv.extend(paths.iter().cloned());
        self.run(&self.repository_root, &add_argv).await?;
        let validation = run_validation_commands(
            &self.repository_root,
            validation_commands,
            WORKSPACE_TIMEOUT,
        )
        .await?;
        if let Some(failed) = validation.iter().find(|result| !result.success) {
            return Err(WorkspaceError::ValidationFailed {
                argv: failed.argv.clone(),
                stderr: failed.stderr.clone(),
            });
        }
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "config".into(),
                "user.name".into(),
                "PiTools[bot]".into(),
            ],
        )
        .await?;
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "config".into(),
                "user.email".into(),
                "pitools[bot]@users.noreply.github.com".into(),
            ],
        )
        .await?;
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "commit".into(),
                "-m".into(),
                "fix: repair failed CI check".into(),
            ],
        )
        .await?;
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "push".into(),
                "origin".into(),
                format!("HEAD:refs/heads/{branch}"),
            ],
        )
        .await?;
        let commit = self
            .run(
                &self.repository_root,
                &["git".into(), "rev-parse".into(), "HEAD".into()],
            )
            .await?;
        Ok(CiRepairCommit {
            commit: commit.stdout.trim().to_owned(),
            paths,
        })
    }

    /// Rebase a checked-out branch onto a freshly fetched target and publish
    /// it only with an exact remote-head lease. A changed history is rejected
    /// unless the caller has already authorized force-with-lease for this
    /// explicitly bot-owned branch.
    pub async fn rebase_onto(
        &self,
        lease: &LeaseGuard,
        branch: &str,
        target_branch: &str,
        expected_head_sha: &str,
        allow_force_push: bool,
        validation_commands: &[crate::policy::ValidationCommand],
    ) -> Result<RebaseCommit, WorkspaceError> {
        validate_branch(branch)?;
        validate_branch(target_branch)?;
        validate_sha(expected_head_sha)?;
        if branch == target_branch {
            return Err(WorkspaceError::InvalidInput(
                "rebase branch and target branch must differ".into(),
            ));
        }
        self.ensure_clean().await?;
        lease.ensure().await?;

        let remote_head_argv = vec![
            "git".into(),
            "ls-remote".into(),
            "origin".into(),
            format!("refs/heads/{branch}"),
        ];
        let remote_head = self
            .run_result(&self.repository_root, &remote_head_argv)
            .await?;
        if !remote_head.success {
            return Err(WorkspaceError::CommandFailed {
                argv: remote_head_argv,
                stderr: remote_head.stderr,
            });
        }
        let observed_head = remote_head
            .stdout
            .split_whitespace()
            .next()
            .unwrap_or_default();
        if observed_head != expected_head_sha {
            return Err(WorkspaceError::RemoteHeadChanged {
                branch: branch.into(),
                expected: expected_head_sha.into(),
                observed: if observed_head.is_empty() {
                    "missing".into()
                } else {
                    observed_head.into()
                },
            });
        }

        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "fetch".into(),
                "--no-tags".into(),
                "origin".into(),
                format!("refs/heads/{target_branch}:refs/remotes/origin/{target_branch}"),
            ],
        )
        .await?;
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "config".into(),
                "user.name".into(),
                "PiTools[bot]".into(),
            ],
        )
        .await?;
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "config".into(),
                "user.email".into(),
                "pitools[bot]@users.noreply.github.com".into(),
            ],
        )
        .await?;

        let rebase_argv = vec![
            "git".into(),
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "rebase".into(),
            format!("refs/remotes/origin/{target_branch}"),
        ];
        let rebase = self.run_result(&self.repository_root, &rebase_argv).await?;
        if !rebase.success {
            let _ = self
                .run_result(
                    &self.repository_root,
                    &[
                        "git".into(),
                        "-c".into(),
                        "core.hooksPath=/dev/null".into(),
                        "rebase".into(),
                        "--abort".into(),
                    ],
                )
                .await;
            return Err(WorkspaceError::RebaseConflict {
                branch: branch.into(),
                target_branch: target_branch.into(),
                stderr: rebase.stderr,
            });
        }

        lease.ensure().await?;
        let after = self
            .run(
                &self.repository_root,
                &["git".into(), "rev-parse".into(), "HEAD".into()],
            )
            .await?
            .stdout
            .trim()
            .to_owned();
        if after == expected_head_sha {
            return Ok(RebaseCommit {
                before: expected_head_sha.into(),
                after,
                target_branch: target_branch.into(),
                force_pushed: false,
            });
        }

        let validation = run_validation_commands(
            &self.repository_root,
            validation_commands,
            WORKSPACE_TIMEOUT,
        )
        .await?;
        if let Some(failed) = validation.iter().find(|result| !result.success) {
            return Err(WorkspaceError::ValidationFailed {
                argv: failed.argv.clone(),
                stderr: failed.stderr.clone(),
            });
        }

        let ancestor_argv = vec![
            "git".into(),
            "merge-base".into(),
            "--is-ancestor".into(),
            expected_head_sha.into(),
            after.clone(),
        ];
        let ancestor = self
            .run_result(&self.repository_root, &ancestor_argv)
            .await?;
        let force_pushed = if ancestor.success {
            false
        } else if allow_force_push {
            true
        } else {
            return Err(WorkspaceError::RebaseRequiresForcePush {
                branch: branch.into(),
                target_branch: target_branch.into(),
            });
        };

        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &rebase_push_argv(branch, expected_head_sha, force_pushed)?,
        )
        .await?;
        Ok(RebaseCommit {
            before: expected_head_sha.into(),
            after,
            target_branch: target_branch.into(),
            force_pushed,
        })
    }

    async fn run_patch_command(
        &self,
        lease: &LeaseGuard,
        argv: &[&str],
        input: &[u8],
    ) -> Result<(), WorkspaceError> {
        let argv = argv
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let result = run_bounded_argv_with_input(
            &self.repository_root,
            &argv,
            WORKSPACE_TIMEOUT,
            &[],
            input,
        )
        .await?;
        lease.ensure().await?;
        if result.success {
            Ok(())
        } else {
            Err(WorkspaceError::CommandFailed {
                argv,
                stderr: result.stderr,
            })
        }
    }

    async fn ensure_clean(&self) -> Result<(), WorkspaceError> {
        let result = self
            .run(
                &self.repository_root,
                &[
                    "git".into(),
                    "status".into(),
                    "--porcelain=v1".into(),
                    "--untracked-files=all".into(),
                ],
            )
            .await?;
        if !result.stdout.trim().is_empty() {
            return Err(WorkspaceError::DirtyWorkspace);
        }
        Ok(())
    }

    async fn run_result(
        &self,
        worktree: &Path,
        argv: &[String],
    ) -> Result<crate::ci::CommandResult, WorkspaceError> {
        let askpass = self.askpass.to_string_lossy().into_owned();
        let environment = [
            ("GIT_ASKPASS", askpass.as_str()),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("PITOOLS_GIT_ASKPASS", "1"),
            ("PITOOLS_GIT_USERNAME", self.username.as_str()),
            ("PITOOLS_GIT_PASSWORD", self.password.expose_secret()),
        ];
        let result = run_bounded_argv(worktree, argv, WORKSPACE_TIMEOUT, &environment).await?;
        Ok(crate::ci::CommandResult {
            stderr: result
                .stderr
                .replace(self.password.expose_secret(), "[REDACTED]"),
            ..result
        })
    }

    async fn run(
        &self,
        worktree: &Path,
        argv: &[String],
    ) -> Result<crate::ci::CommandResult, WorkspaceError> {
        let result = self.run_result(worktree, argv).await?;
        if !result.success {
            return Err(WorkspaceError::CommandFailed {
                argv: argv.to_vec(),
                stderr: result.stderr,
            });
        }
        Ok(result)
    }
}

#[derive(Clone)]
pub struct LeaseGuard {
    queue: JobQueue,
    job_id: Uuid,
    worker_id: String,
    lease_token: Uuid,
}

impl LeaseGuard {
    pub fn new(
        queue: JobQueue,
        job_id: Uuid,
        worker_id: impl Into<String>,
        lease_token: Uuid,
    ) -> Self {
        Self {
            queue,
            job_id,
            worker_id: worker_id.into(),
            lease_token,
        }
    }

    pub async fn ensure(&self) -> Result<(), WorkspaceError> {
        if self
            .queue
            .assert_lease(self.job_id, &self.worker_id, self.lease_token)
            .await
            .map_err(|error| WorkspaceError::LeaseCheck(error.to_string()))?
        {
            Ok(())
        } else {
            Err(WorkspaceError::LeaseLost)
        }
    }
}

impl Drop for RepositoryWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairCommit {
    pub commit: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiRepairCommit {
    pub commit: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebaseCommit {
    pub before: String,
    pub after: String,
    pub target_branch: String,
    pub force_pushed: bool,
}

/// Build the only push command permitted by the rebase executor.
///
/// A rewritten branch must carry an exact expected remote head in its lease;
/// callers must separately authorize that mode for an explicitly bot-owned
/// branch.
pub fn rebase_push_argv(
    branch: &str,
    expected_head_sha: &str,
    force_with_lease: bool,
) -> Result<Vec<String>, WorkspaceError> {
    validate_branch(branch)?;
    validate_sha(expected_head_sha)?;
    let mut argv = vec!["git".to_owned(), "push".to_owned()];
    if force_with_lease {
        argv.push(format!(
            "--force-with-lease=refs/heads/{branch}:{expected_head_sha}"
        ));
    }
    argv.push("origin".to_owned());
    argv.push(format!("HEAD:refs/heads/{branch}"));
    Ok(argv)
}

fn validate_segment(value: &str, field: &'static str) -> Result<(), WorkspaceError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\', '\0', '\r', '\n'])
    {
        return Err(WorkspaceError::InvalidInput(format!("invalid {field}")));
    }
    Ok(())
}

fn validate_branch(value: &str) -> Result<(), WorkspaceError> {
    if value.is_empty()
        || value.len() > 255
        || value == "."
        || value == ".."
        || value.starts_with('-')
        || value.starts_with('/')
        || value.ends_with('/')
        || value.ends_with('.')
        || value == "@"
        || value.contains("//")
        || value.contains("@{")
        || value.contains(['\0', '\r', '\n', ' ', '~', '^', ':', '?', '*', '[', '\\'])
        || value.contains("..")
        || value.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
        || value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(WorkspaceError::InvalidInput("invalid branch".into()));
    }
    Ok(())
}

fn validate_sha(value: &str) -> Result<(), WorkspaceError> {
    if value.len() < 7 || value.len() > 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(WorkspaceError::InvalidInput("invalid commit SHA".into()));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("workspace I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("workspace input is invalid: {0}")]
    InvalidInput(String),
    #[error("workspace command failed: {argv:?}: {stderr}")]
    CommandFailed { argv: Vec<String>, stderr: String },
    #[error("workspace command error: {0}")]
    Command(#[from] crate::ci::CiError),
    #[error("validation failed: {argv:?}: {stderr}")]
    ValidationFailed { argv: Vec<String>, stderr: String },
    #[error("workspace started dirty")]
    DirtyWorkspace,
    #[error("deterministic feedback repair did not apply a change")]
    RepairNotApplied,
    #[error("feedback repair failed: {0}")]
    Feedback(#[from] crate::feedback::FeedbackError),
    #[error("CI repair patch is invalid: {0}")]
    CiPatch(String),
    #[error("job lease check failed: {0}")]
    LeaseCheck(String),
    #[error("job lease was cancelled or lost before mutation")]
    LeaseLost,
    #[error("remote head for branch {branch} changed: expected {expected}, observed {observed}")]
    RemoteHeadChanged {
        branch: String,
        expected: String,
        observed: String,
    },
    #[error("rebase conflict for {branch} onto {target_branch}: {stderr}")]
    RebaseConflict {
        branch: String,
        target_branch: String,
        stderr: String,
    },
    #[error(
        "rebase of {branch} onto {target_branch} requires an explicitly authorized force-with-lease push"
    )]
    RebaseRequiresForcePush {
        branch: String,
        target_branch: String,
    },
}
