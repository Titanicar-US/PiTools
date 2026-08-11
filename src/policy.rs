use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Policy {
    pub automation_actors: Vec<String>,
    pub maintainers: Vec<String>,
    pub required_checks: Vec<String>,
    pub require_approval: bool,
    pub require_current_branch: bool,
    pub reconcile_interval_seconds: u64,
    pub allow_bot_force_push: bool,
    /// Exact branch names that PiTools may rewrite when force-push is enabled.
    pub bot_owned_branches: Vec<String>,
    /// Exact repository-relative files Pi may propose for an approved repair.
    pub repair_allowed_paths: Vec<String>,
    pub stack_parents: BTreeMap<i32, i32>,
    pub validation_commands: Vec<ValidationCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValidationCommand {
    GitDiffCheck,
    MakeCheck,
    CargoCheck,
    CargoTest,
    CargoClippy,
    NpmCheck,
}

impl ValidationCommand {
    pub fn argv(&self) -> &'static [&'static str] {
        match self {
            Self::GitDiffCheck => &["git", "diff", "--cached", "--check", "--"],
            Self::MakeCheck => &["make", "check"],
            Self::CargoCheck => &["cargo", "check", "--locked"],
            Self::CargoTest => &["cargo", "test", "--locked"],
            Self::CargoClippy => &[
                "cargo",
                "clippy",
                "--locked",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            Self::NpmCheck => &["npm", "--prefix", "workers/pi", "run", "check"],
        }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            automation_actors: Vec::new(),
            maintainers: Vec::new(),
            required_checks: Vec::new(),
            require_approval: true,
            require_current_branch: true,
            reconcile_interval_seconds: 300,
            allow_bot_force_push: false,
            bot_owned_branches: Vec::new(),
            repair_allowed_paths: Vec::new(),
            stack_parents: BTreeMap::new(),
            validation_commands: vec![ValidationCommand::GitDiffCheck],
        }
    }
}

impl Policy {
    pub fn from_yaml(source: &str) -> Result<Self, PolicyError> {
        let policy: Self = serde_yaml_ng::from_str(source)
            .map_err(|error| PolicyError::Invalid(error.to_string()))?;
        policy.validate()?;
        Ok(policy)
    }

    pub fn revision(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("Policy serialization is infallible");
        format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
    }

    fn validate(&self) -> Result<(), PolicyError> {
        if self.reconcile_interval_seconds < 30 {
            return Err(PolicyError::Invalid(
                "reconcile_interval_seconds must be at least 30".into(),
            ));
        }
        if self
            .automation_actors
            .iter()
            .any(|actor| actor.trim().is_empty())
        {
            return Err(PolicyError::Invalid(
                "automation_actors cannot contain empty values".into(),
            ));
        }
        if self
            .maintainers
            .iter()
            .any(|maintainer| maintainer.trim().is_empty())
        {
            return Err(PolicyError::Invalid(
                "maintainers cannot contain empty values".into(),
            ));
        }
        if self.repair_allowed_paths.iter().any(|path| {
            path.is_empty()
                || path.contains(['\\', '\0', '\r', '\n'])
                || Path::new(path).is_absolute()
                || !Path::new(path)
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
        }) {
            return Err(PolicyError::Invalid(
                "repair_allowed_paths must contain canonical repository-relative paths".into(),
            ));
        }
        if self.bot_owned_branches.iter().any(|branch| {
            branch.is_empty()
                || branch.len() > 255
                || branch == "."
                || branch == ".."
                || branch.starts_with('-')
                || branch.starts_with('/')
                || branch.ends_with('/')
                || branch.ends_with('.')
                || branch == "@"
                || branch.contains("//")
                || branch.contains("@{")
                || branch.contains(['\0', '\r', '\n', ' ', '~', '^', ':', '?', '*', '[', '\\'])
                || branch.contains("..")
                || branch.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
                || branch
                    .split('/')
                    .any(|component| component.is_empty() || component == "." || component == "..")
        }) {
            return Err(PolicyError::Invalid(
                "bot_owned_branches must contain valid branch names".into(),
            ));
        }
        if self.allow_bot_force_push && self.bot_owned_branches.is_empty() {
            return Err(PolicyError::Invalid(
                "allow_bot_force_push requires bot_owned_branches".into(),
            ));
        }
        if self
            .stack_parents
            .iter()
            .any(|(child, parent)| child == parent)
        {
            return Err(PolicyError::Invalid(
                "stack_parents cannot point a pull request to itself".into(),
            ));
        }
        Ok(())
    }

    pub fn is_configured_automation_actor(&self, login: &str) -> bool {
        self.automation_actors.iter().any(|actor| actor == login)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("invalid policy: {0}")]
    Invalid(String),
}
