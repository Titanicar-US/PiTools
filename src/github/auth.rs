use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::StatusCode;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone)]
pub struct GitHubAppAuth {
    app_id: u64,
    private_key_pem: SecretString,
    client: reqwest::Client,
    api_base: Url,
}

impl std::fmt::Debug for GitHubAppAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitHubAppAuth")
            .field("app_id", &self.app_id)
            .field("private_key_pem", &"[REDACTED]")
            .field("api_base", &self.api_base)
            .finish()
    }
}

impl GitHubAppAuth {
    pub fn new(
        app_id: u64,
        private_key_pem: SecretString,
        api_base: Url,
    ) -> Result<Self, GitHubAuthError> {
        EncodingKey::from_rsa_pem(private_key_pem.expose_secret().as_bytes())
            .map_err(|error| GitHubAuthError::InvalidPrivateKey(error.to_string()))?;
        let client = reqwest::Client::builder()
            .user_agent("PiTools/0.1")
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| GitHubAuthError::Client(error.to_string()))?;
        Ok(Self {
            app_id,
            private_key_pem,
            client,
            api_base,
        })
    }

    pub fn app_jwt(&self, now: SystemTime) -> Result<SecretString, GitHubAuthError> {
        let issued_at = now
            .duration_since(UNIX_EPOCH)
            .map_err(|error| GitHubAuthError::Clock(error.to_string()))?
            .as_secs()
            .saturating_sub(60);
        let claims = AppClaims {
            iat: issued_at,
            exp: issued_at + 540,
            iss: self.app_id.to_string(),
        };
        let key = EncodingKey::from_rsa_pem(self.private_key_pem.expose_secret().as_bytes())
            .map_err(|error| GitHubAuthError::InvalidPrivateKey(error.to_string()))?;
        let mut header = Header::new(Algorithm::RS256);
        header.typ = Some("JWT".into());
        encode(&header, &claims, &key)
            .map(SecretString::from)
            .map_err(|error| GitHubAuthError::Jwt(error.to_string()))
    }

    pub async fn installation_token(
        &self,
        installation_id: i64,
    ) -> Result<InstallationToken, GitHubAuthError> {
        self.installation_token_with_scope(installation_id, &InstallationTokenScope::default())
            .await
    }

    pub fn api_base(&self) -> Url {
        self.api_base.clone()
    }

    pub async fn installation_token_with_scope(
        &self,
        installation_id: i64,
        scope: &InstallationTokenScope,
    ) -> Result<InstallationToken, GitHubAuthError> {
        let jwt = self.app_jwt(SystemTime::now())?;
        let url = self
            .api_base
            .join(&format!(
                "app/installations/{installation_id}/access_tokens"
            ))
            .map_err(|error| GitHubAuthError::Client(error.to_string()))?;
        let request = self
            .client
            .post(url)
            .bearer_auth(jwt.expose_secret())
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28");
        let response = if scope.is_empty() {
            request.send().await
        } else {
            request.json(scope).send().await
        }
        .map_err(|error| GitHubAuthError::Request(error.to_string()))?;
        let status = response.status();
        if status != StatusCode::CREATED {
            return Err(GitHubAuthError::Api {
                status: status.as_u16(),
                body: response
                    .text()
                    .await
                    .unwrap_or_else(|_| "unreadable response".into()),
            });
        }
        let response = response
            .json::<InstallationTokenResponse>()
            .await
            .map_err(|error| GitHubAuthError::Response(error.to_string()))?;
        Ok(InstallationToken {
            token: SecretString::from(response.token),
            expires_at: response.expires_at,
            permissions: response.permissions,
            repository_selection: response.repository_selection,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct InstallationTokenScope {
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub permissions: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub repositories: Vec<String>,
}

impl InstallationTokenScope {
    pub fn is_empty(&self) -> bool {
        self.permissions.is_empty() && self.repositories.is_empty()
    }
}

#[derive(Debug, Serialize)]
struct AppClaims {
    iat: u64,
    exp: u64,
    iss: String,
}

#[derive(Debug, Clone)]
pub struct InstallationToken {
    pub token: SecretString,
    pub expires_at: String,
    pub permissions: serde_json::Value,
    pub repository_selection: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InstallationTokenResponse {
    token: String,
    expires_at: String,
    permissions: serde_json::Value,
    repository_selection: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum GitHubAuthError {
    #[error("invalid GitHub App private key: {0}")]
    InvalidPrivateKey(String),
    #[error("GitHub client setup failed: {0}")]
    Client(String),
    #[error("system clock failed: {0}")]
    Clock(String),
    #[error("GitHub JWT generation failed: {0}")]
    Jwt(String),
    #[error("GitHub request failed: {0}")]
    Request(String),
    #[error("GitHub response decoding failed: {0}")]
    Response(String),
    #[error("GitHub returned HTTP {status}: {body}")]
    Api { status: u16, body: String },
}
