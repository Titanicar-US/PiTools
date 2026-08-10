use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{StatusCode, header::HeaderMap};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use url::Url;

const MAX_APP_INSTALLATION_PAGES: u32 = 100;

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

    /// Validate the configured App identity and report visible installations.
    ///
    /// This is intentionally an App-authenticated probe rather than an
    /// installation-token probe: it detects a wrong App ID/key pair and gives
    /// operators evidence that the App is installed somewhere before they
    /// troubleshoot repository-level webhook behavior.
    pub async fn app_status(&self) -> Result<GitHubAppStatus, GitHubAuthError> {
        let jwt = self.app_jwt(SystemTime::now())?;
        self.app_status_with_jwt(&jwt).await
    }

    async fn app_status_with_jwt(
        &self,
        jwt: &SecretString,
    ) -> Result<GitHubAppStatus, GitHubAuthError> {
        let identity: AppIdentityResponse = self.app_get("app", jwt).await?;
        if identity.id != self.app_id {
            return Err(GitHubAuthError::AppIdentityMismatch {
                configured: self.app_id,
                reported: identity.id,
            });
        }
        let installations = self.app_installations(jwt).await?;
        let installation_count = installations.len();
        let mut installation_accounts: Vec<String> = installations
            .into_iter()
            .filter_map(|installation| installation.account?.login)
            .collect();
        installation_accounts.sort_unstable();
        Ok(GitHubAppStatus {
            app_id: identity.id,
            app_name: identity.name,
            installation_count,
            installation_accounts,
        })
    }

    async fn app_get<T: DeserializeOwned>(
        &self,
        path: &str,
        jwt: &SecretString,
    ) -> Result<T, GitHubAuthError> {
        self.app_get_with_headers(path, jwt)
            .await
            .map(|(value, _)| value)
    }

    async fn app_installations(
        &self,
        jwt: &SecretString,
    ) -> Result<Vec<AppInstallationResponse>, GitHubAuthError> {
        let mut page = 1;
        let mut installations = Vec::new();
        loop {
            let path = format!("app/installations?per_page=100&page={page}");
            let (page_installations, headers) = self
                .app_get_with_headers::<Vec<AppInstallationResponse>>(&path, jwt)
                .await?;
            installations.extend(page_installations);
            if !has_next_page(&headers) {
                return Ok(installations);
            }
            page += 1;
            if page > MAX_APP_INSTALLATION_PAGES {
                return Err(GitHubAuthError::AppInstallationPaginationLimit {
                    max_pages: MAX_APP_INSTALLATION_PAGES,
                });
            }
        }
    }

    async fn app_get_with_headers<T: DeserializeOwned>(
        &self,
        path: &str,
        jwt: &SecretString,
    ) -> Result<(T, HeaderMap), GitHubAuthError> {
        let url = self
            .api_base
            .join(path)
            .map_err(|error| GitHubAuthError::Client(error.to_string()))?;
        let response = self
            .client
            .get(url)
            .bearer_auth(jwt.expose_secret())
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .send()
            .await
            .map_err(|error| GitHubAuthError::Request(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(GitHubAuthError::Api {
                status: status.as_u16(),
                body: response
                    .text()
                    .await
                    .unwrap_or_else(|_| "unreadable response".into()),
            });
        }
        let headers = response.headers().clone();
        let value = response
            .json::<T>()
            .await
            .map_err(|error| GitHubAuthError::Response(error.to_string()))?;
        Ok((value, headers))
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubAppStatus {
    pub app_id: u64,
    pub app_name: String,
    pub installation_count: usize,
    pub installation_accounts: Vec<String>,
}

fn has_next_page(headers: &HeaderMap) -> bool {
    headers
        .get(reqwest::header::LINK)
        .and_then(|value| value.to_str().ok())
        .map(|link| {
            link.split(',').any(|entry| {
                entry.split(';').skip(1).any(|parameter| {
                    parameter
                        .trim()
                        .strip_prefix("rel=")
                        .map(|value| value.trim_matches('"') == "next")
                        .unwrap_or(false)
                })
            })
        })
        .unwrap_or(false)
}

impl GitHubAppStatus {
    pub fn new(
        app_id: u64,
        app_name: impl Into<String>,
        installation_accounts: Vec<String>,
    ) -> Self {
        let installation_count = installation_accounts.len();
        Self {
            app_id,
            app_name: app_name.into(),
            installation_count,
            installation_accounts,
        }
    }

    pub fn render(&self) -> String {
        let accounts = if self.installation_accounts.is_empty() {
            "none".to_owned()
        } else {
            self.installation_accounts.join(",")
        };
        format!(
            "github_app_id={} github_app_name={} installations={} installation_accounts={accounts}",
            self.app_id, self.app_name, self.installation_count
        )
    }
}

#[derive(Debug, Deserialize)]
struct AppIdentityResponse {
    id: u64,
    name: String,
}

#[derive(Debug, Deserialize)]
struct AppInstallationResponse {
    account: Option<AppInstallationAccount>,
}

#[derive(Debug, Deserialize)]
struct AppInstallationAccount {
    login: Option<String>,
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
    #[error("configured GitHub App ID {configured} did not match API identity {reported}")]
    AppIdentityMismatch { configured: u64, reported: u64 },
    #[error("GitHub App installation pagination exceeded {max_pages} pages")]
    AppInstallationPaginationLimit { max_pages: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path, query_param},
    };

    #[tokio::test]
    async fn app_status_validates_identity_and_lists_installations() {
        let server = MockServer::start().await;
        let authenticated_get = || {
            Mock::given(method("GET"))
                .and(header("authorization", "Bearer test-app-jwt"))
                .and(header("x-github-api-version", "2022-11-28"))
        };
        authenticated_get()
            .and(path("/app"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 42,
                "name": "PiTools",
                "slug": "pitools"
            })))
            .mount(&server)
            .await;
        authenticated_get()
            .and(path("/app/installations"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": 7, "account": {"login": "Titanicar-US", "type": "Organization"}},
                {"id": 8}
            ])))
            .mount(&server)
            .await;

        let auth = GitHubAppAuth {
            app_id: 42,
            private_key_pem: SecretString::from("test-only-key"),
            client: reqwest::Client::new(),
            api_base: Url::parse(&format!("{}/", server.uri())).expect("mock URL is valid"),
        };
        let status = auth
            .app_status_with_jwt(&SecretString::from("test-app-jwt"))
            .await
            .expect("app status succeeds");

        assert_eq!(status.app_id, 42);
        assert_eq!(status.app_name, "PiTools");
        assert_eq!(status.installation_count, 2);
        assert_eq!(status.installation_accounts, vec!["Titanicar-US"]);
        assert!(status.render().contains("installations=2"));
    }

    #[tokio::test]
    async fn app_status_follows_installation_pagination_links() {
        let server = MockServer::start().await;
        let authenticated_get = || {
            Mock::given(method("GET"))
                .and(header("authorization", "Bearer test-app-jwt"))
                .and(header("x-github-api-version", "2022-11-28"))
        };
        authenticated_get()
            .and(path("/app"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 42,
                "name": "PiTools"
            })))
            .mount(&server)
            .await;
        authenticated_get()
            .and(path("/app/installations"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header(
                        "link",
                        format!(
                            "<{}/app/installations?per_page=100&page=2>; rel=\"next\"",
                            server.uri()
                        ),
                    )
                    .set_body_json(json!([
                        {"id": 7, "account": {"login": "Titanicar-US"}}
                    ])),
            )
            .mount(&server)
            .await;
        authenticated_get()
            .and(path("/app/installations"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": 8, "account": {"login": "PiTools-Test"}}
            ])))
            .mount(&server)
            .await;

        let auth = GitHubAppAuth {
            app_id: 42,
            private_key_pem: SecretString::from("test-only-key"),
            client: reqwest::Client::new(),
            api_base: Url::parse(&format!("{}/", server.uri())).expect("mock URL is valid"),
        };
        let status = auth
            .app_status_with_jwt(&SecretString::from("test-app-jwt"))
            .await
            .expect("paginated App status succeeds");

        assert_eq!(status.installation_count, 2);
        assert_eq!(
            status.installation_accounts,
            vec!["PiTools-Test", "Titanicar-US"]
        );
    }

    #[tokio::test]
    async fn app_status_rejects_a_mismatched_app_identity_before_listing_installations() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/app"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 99,
                "name": "Another App"
            })))
            .mount(&server)
            .await;
        let auth = GitHubAppAuth {
            app_id: 42,
            private_key_pem: SecretString::from("test-only-key"),
            client: reqwest::Client::new(),
            api_base: Url::parse(&format!("{}/", server.uri())).expect("mock URL is valid"),
        };

        let error = auth
            .app_status_with_jwt(&SecretString::from("test-app-jwt"))
            .await
            .expect_err("mismatched App identity must fail closed");
        assert!(matches!(
            error,
            GitHubAuthError::AppIdentityMismatch {
                configured: 42,
                reported: 99
            }
        ));
    }
}
