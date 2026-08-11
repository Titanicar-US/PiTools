//! Ephemeral repository workspaces for approved deterministic repairs.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use secrecy::{ExposeSecret, SecretString};
use uuid::Uuid;

use crate::{
    ci::{
        CiPatch, run_bounded_argv, run_bounded_argv_with_input, run_validation_commands_sandboxed,
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
    remote_url: String,
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
        let clone_url = format!("https://github.com/{owner}/{repository}.git");
        // Use the already-installed pitools executable as the askpass helper.
        // This keeps authentication working when /tmp is deliberately mounted
        // noexec and avoids writing an executable secret helper into a
        // workspace.
        let askpass = std::env::current_exe()?;
        let workspace = Self {
            root,
            repository_root,
            askpass,
            remote_url: clone_url.clone(),
            username: "x-access-token".into(),
            password: token,
        };
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
        expected_head_sha: &str,
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
            &["git".into(), "add".into(), "--".into(), path.clone()],
        )
        .await?;
        let validation_root = self
            .prepare_validation_snapshot(std::slice::from_ref(&path))
            .await?;
        let validation = run_validation_commands_sandboxed(
            &validation_root,
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
        self.ensure_staged_paths(std::slice::from_ref(&path))
            .await?;
        lease.ensure().await?;
        lease.ensure().await?;
        self.ensure_remote_head(branch, expected_head_sha).await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "-c".into(),
                "core.hooksPath=/dev/null".into(),
                "-c".into(),
                "user.name=PiTools[bot]".into(),
                "-c".into(),
                "user.email=pitools[bot]@users.noreply.github.com".into(),
                "commit".into(),
                "--no-verify".into(),
                "-m".into(),
                audit_commit_message(
                    "fix: apply automated review suggestion",
                    lease.job_id,
                    "feedback",
                    expected_head_sha,
                ),
            ],
        )
        .await?;
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "-c".into(),
                "core.hooksPath=/dev/null".into(),
                "-c".into(),
                format!("remote.origin.url={}", self.remote_url),
                "push".into(),
                "--no-verify".into(),
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
        expected_head_sha: &str,
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
        let validation_root = self.prepare_validation_snapshot(&paths).await?;
        let validation = run_validation_commands_sandboxed(
            &validation_root,
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
        self.ensure_staged_paths(&paths).await?;
        lease.ensure().await?;
        lease.ensure().await?;
        self.ensure_remote_head(branch, expected_head_sha).await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "-c".into(),
                "core.hooksPath=/dev/null".into(),
                "-c".into(),
                "user.name=PiTools[bot]".into(),
                "-c".into(),
                "user.email=pitools[bot]@users.noreply.github.com".into(),
                "commit".into(),
                "--no-verify".into(),
                "-m".into(),
                audit_commit_message(
                    "fix: repair failed CI check",
                    lease.job_id,
                    "ci",
                    expected_head_sha,
                ),
            ],
        )
        .await?;
        lease.ensure().await?;
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "-c".into(),
                "core.hooksPath=/dev/null".into(),
                "-c".into(),
                format!("remote.origin.url={}", self.remote_url),
                "push".into(),
                "--no-verify".into(),
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

    async fn prepare_validation_snapshot(
        &self,
        changed_paths: &[String],
    ) -> Result<PathBuf, WorkspaceError> {
        let validation_root = self.root.join("validation");
        fs::create_dir(&validation_root)?;
        let archive = self.root.join("source.tar");
        self.run(
            &self.repository_root,
            &[
                "git".into(),
                "-c".into(),
                "core.hooksPath=/dev/null".into(),
                "archive".into(),
                "--format=tar".into(),
                "--output".into(),
                archive.to_string_lossy().into_owned(),
                "HEAD".into(),
            ],
        )
        .await?;
        self.run(
            &self.repository_root,
            &[
                "tar".into(),
                "-xf".into(),
                archive.to_string_lossy().into_owned(),
                "-C".into(),
                validation_root.to_string_lossy().into_owned(),
            ],
        )
        .await?;
        fs::remove_file(archive)?;

        for path in changed_paths {
            copy_changed_path(&self.repository_root, &validation_root, path)?;
        }
        Ok(validation_root)
    }

    async fn ensure_staged_paths(&self, expected: &[String]) -> Result<(), WorkspaceError> {
        let staged = self
            .run(
                &self.repository_root,
                &[
                    "git".into(),
                    "diff".into(),
                    "--cached".into(),
                    "--name-only".into(),
                    "--".into(),
                ],
            )
            .await?;
        validate_staged_paths(expected, &staged.stdout)?;

        let summary = self
            .run(
                &self.repository_root,
                &[
                    "git".into(),
                    "diff".into(),
                    "--cached".into(),
                    "--summary".into(),
                    "--".into(),
                ],
            )
            .await?;
        validate_staged_diff_summary(&summary.stdout)?;

        let unstaged = self
            .run(
                &self.repository_root,
                &[
                    "git".into(),
                    "diff".into(),
                    "--name-only".into(),
                    "--".into(),
                ],
            )
            .await?;
        if !unstaged.stdout.trim().is_empty() {
            return Err(WorkspaceError::UnexpectedWorkspaceChanges(
                unstaged.stdout.trim().to_owned(),
            ));
        }

        let untracked = self
            .run(
                &self.repository_root,
                &[
                    "git".into(),
                    "ls-files".into(),
                    "--others".into(),
                    "--exclude-standard".into(),
                ],
            )
            .await?;
        if !untracked.stdout.trim().is_empty() {
            return Err(WorkspaceError::UnexpectedWorkspaceChanges(
                untracked.stdout.trim().to_owned(),
            ));
        }
        Ok(())
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

        let validation_root = self.prepare_validation_snapshot(&[]).await?;
        let validation = run_validation_commands_sandboxed(
            &validation_root,
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

    async fn ensure_remote_head(
        &self,
        branch: &str,
        expected_head_sha: &str,
    ) -> Result<(), WorkspaceError> {
        validate_sha(expected_head_sha)?;
        let argv = remote_head_argv(branch)?;
        let result = self.run_result(&self.repository_root, &argv).await?;
        if !result.success {
            return Err(WorkspaceError::CommandFailed {
                argv,
                stderr: result.stderr,
            });
        }
        let observed = result.stdout.split_whitespace().next().unwrap_or_default();
        if observed != expected_head_sha {
            return Err(WorkspaceError::RemoteHeadChanged {
                branch: branch.into(),
                expected: expected_head_sha.into(),
                observed: if observed.is_empty() {
                    "missing".into()
                } else {
                    observed.into()
                },
            });
        }
        Ok(())
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
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_SYSTEM", "/dev/null"),
            ("GIT_ATTR_NOSYSTEM", "1"),
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

/// Build the exact remote-head probe used immediately before a normal push.
pub fn remote_head_argv(branch: &str) -> Result<Vec<String>, WorkspaceError> {
    validate_branch(branch)?;
    Ok(vec![
        "git".to_owned(),
        "ls-remote".to_owned(),
        "origin".to_owned(),
        format!("refs/heads/{branch}"),
    ])
}

/// Build the deterministic audit trailers used on bot-authored repair commits.
pub fn audit_commit_message(
    subject: &str,
    job_id: Uuid,
    repair_kind: &str,
    expected_head_sha: &str,
) -> String {
    format!(
        "{subject}\n\nPiTools-Job: {job_id}\nPiTools-Repair: {repair_kind}\nPiTools-Head: {expected_head_sha}"
    )
}

/// Verify that Git will commit exactly the files admitted by the repair plan.
pub fn validate_staged_paths(expected: &[String], observed: &str) -> Result<(), WorkspaceError> {
    let expected = expected
        .iter()
        .map(|path| {
            validate_workspace_relative_path(path)?;
            Ok(path.clone())
        })
        .collect::<Result<BTreeSet<_>, WorkspaceError>>()?;
    let observed = observed
        .lines()
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if expected == observed {
        Ok(())
    } else {
        Err(WorkspaceError::UnexpectedStagedPaths {
            expected: expected.into_iter().collect(),
            observed: observed.into_iter().collect(),
        })
    }
}

/// Reject staged metadata that widens a typed content-only repair into a
/// rename, copy, or executable-mode change.
pub fn validate_staged_diff_summary(summary: &str) -> Result<(), WorkspaceError> {
    let normalized = summary.to_ascii_lowercase();
    if normalized.lines().any(|line| {
        line.contains("mode change") || line.starts_with("rename ") || line.starts_with("copy ")
    }) {
        return Err(WorkspaceError::UnexpectedWorkspaceChanges(
            summary.trim().to_owned(),
        ));
    }
    Ok(())
}

fn copy_changed_path(
    source_root: &Path,
    destination_root: &Path,
    path: &str,
) -> Result<(), WorkspaceError> {
    validate_workspace_relative_path(path)?;
    let source = source_root.join(path);
    let destination = destination_root.join(path);
    match fs::symlink_metadata(&source) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(WorkspaceError::InvalidInput(
            format!("symlink repair path is not supported: {path}"),
        )),
        Ok(metadata) if metadata.is_file() => {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(source, destination)?;
            Ok(())
        }
        Ok(_) => Err(WorkspaceError::InvalidInput(format!(
            "repair path is not a regular file: {path}"
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::remove_file(destination) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn validate_workspace_relative_path(value: &str) -> Result<(), WorkspaceError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains(['\\', '\0', '\r', '\n'])
        || path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return Err(WorkspaceError::InvalidInput(format!(
            "invalid repository-relative path: {value}"
        )));
    }
    Ok(())
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
    #[error(
        "staged paths do not match the approved mutation set: expected {expected:?}, observed {observed:?}"
    )]
    UnexpectedStagedPaths {
        expected: Vec<String>,
        observed: Vec<String>,
    },
    #[error("validation changed the mutation workspace unexpectedly: {0}")]
    UnexpectedWorkspaceChanges(String),
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
