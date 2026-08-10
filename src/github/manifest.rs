use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const DEFAULT_EVENTS: &[&str] = &[
    "check_run",
    "check_suite",
    "issue_comment",
    "installation",
    "installation_repositories",
    "pull_request",
    "pull_request_review",
    "pull_request_review_comment",
    "status",
    "workflow_run",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AppManifest {
    pub name: String,
    pub url: String,
    pub hook_attributes: HookAttributes,
    pub redirect_url: String,
    pub public: bool,
    pub default_permissions: BTreeMap<String, String>,
    pub default_events: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HookAttributes {
    pub url: String,
    pub active: bool,
}

impl AppManifest {
    pub fn for_public_project(base_url: &str) -> Self {
        let mut permissions = BTreeMap::new();
        permissions.insert("actions".into(), "read".into());
        permissions.insert("checks".into(), "write".into());
        permissions.insert("contents".into(), "write".into());
        permissions.insert("issues".into(), "write".into());
        permissions.insert("metadata".into(), "read".into());
        permissions.insert("pull_requests".into(), "write".into());
        permissions.insert("statuses".into(), "read".into());
        let webhook_url = format!("{base_url}/github/webhook");
        Self {
            name: "PiTools".into(),
            url: base_url.into(),
            hook_attributes: HookAttributes {
                url: webhook_url,
                active: true,
            },
            redirect_url: format!("{base_url}/github/manifest/callback"),
            public: false,
            default_permissions: permissions,
            default_events: DEFAULT_EVENTS.iter().map(|event| (*event).into()).collect(),
        }
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.name.trim().is_empty()
            || self.url.trim().is_empty()
            || self.hook_attributes.url.trim().is_empty()
            || self.redirect_url.trim().is_empty()
        {
            return Err(ManifestError::MissingEndpoint);
        }
        for (field, value) in [
            ("url", self.url.as_str()),
            ("hook_attributes.url", self.hook_attributes.url.as_str()),
            ("redirect_url", self.redirect_url.as_str()),
        ] {
            let parsed = url::Url::parse(value).map_err(|_| ManifestError::InvalidEndpoint {
                field: field.into(),
            })?;
            if parsed.scheme() != "https"
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
            {
                return Err(ManifestError::InvalidEndpoint {
                    field: field.into(),
                });
            }
        }
        if !self.hook_attributes.url.ends_with("/github/webhook") {
            return Err(ManifestError::InvalidWebhookPath);
        }
        for event in &self.default_events {
            if !DEFAULT_EVENTS.contains(&event.as_str()) {
                return Err(ManifestError::UnsupportedEvent(event.clone()));
            }
        }
        for (permission, level) in &self.default_permissions {
            if !matches!(level.as_str(), "read" | "write") {
                return Err(ManifestError::UnsupportedPermission {
                    permission: permission.clone(),
                    level: level.clone(),
                });
            }
        }
        Ok(())
    }

    pub fn as_json(&self) -> Result<serde_json::Value, ManifestError> {
        self.validate()?;
        serde_json::to_value(self).map_err(|error| ManifestError::Serialization(error.to_string()))
    }
}

/// The one-time response returned by GitHub's App Manifest conversion flow.
/// PiTools deliberately does not persist these credentials.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ManifestConversion {
    pub id: u64,
    pub name: String,
    pub slug: String,
    pub client_id: String,
    pub client_secret: String,
    pub webhook_secret: String,
    pub pem: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_url: Option<String>,
}

pub fn validate_manifest_code(code: &str) -> Result<(), ManifestError> {
    if code.is_empty()
        || code.len() > 128
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ManifestError::InvalidCode);
    }
    Ok(())
}

pub async fn exchange_manifest_code(code: &str) -> Result<ManifestConversion, ManifestError> {
    validate_manifest_code(code)?;
    let response = reqwest::Client::builder()
        .user_agent("PiTools/0.1")
        .build()
        .map_err(|error| ManifestError::Conversion(error.to_string()))?
        .post(format!(
            "https://api.github.com/app-manifests/{code}/conversions"
        ))
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .send()
        .await
        .map_err(|error| ManifestError::Conversion(error.to_string()))?;
    if !response.status().is_success() {
        return Err(ManifestError::Conversion(format!(
            "GitHub returned HTTP {}",
            response.status().as_u16()
        )));
    }
    response
        .json()
        .await
        .map_err(|error| ManifestError::Conversion(error.to_string()))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("GitHub App manifest endpoint fields cannot be empty")]
    MissingEndpoint,
    #[error("GitHub App manifest endpoint is not an HTTPS URL: {field}")]
    InvalidEndpoint { field: String },
    #[error("GitHub App webhook URL must end with /github/webhook")]
    InvalidWebhookPath,
    #[error("unsupported GitHub App event: {0}")]
    UnsupportedEvent(String),
    #[error("unsupported GitHub App permission level {permission}={level}")]
    UnsupportedPermission { permission: String, level: String },
    #[error("GitHub App Manifest conversion code is invalid")]
    InvalidCode,
    #[error("GitHub App Manifest conversion failed: {0}")]
    Conversion(String),
    #[error("manifest serialization failed: {0}")]
    Serialization(String),
}
