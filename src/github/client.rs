use std::{collections::BTreeSet, time::Duration};

use reqwest::header::{self, HeaderMap};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use url::Url;

use super::auth::InstallationToken;

const MAX_PAGINATION_PAGES: usize = 100;
const MAX_REPOSITORY_POLICY_BYTES: usize = 256 * 1024;
const MAX_ACTIONS_LOG_BYTES: usize = 128 * 1024;
const MAX_CHECK_ANNOTATION_BYTES: usize = 256 * 1024;
const MAX_CHECK_RUN_ANNOTATIONS: usize = 100;

/// Extract the Actions workflow job ID embedded in a GitHub check-run URL.
///
/// Check-run IDs and Actions job IDs are different resources. GitHub's job
/// log endpoint accepts the latter, while a check run's `details_url` carries
/// the `/job/<id>` path for Actions-backed checks.
pub fn actions_job_id_from_details_url(details_url: Option<&str>) -> Option<i64> {
    let url = Url::parse(details_url?).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let mut segments = url.path_segments()?;
    while let Some(segment) = segments.next() {
        if segment == "job" {
            let job_id = segments.next()?.parse::<i64>().ok()?;
            return (job_id > 0).then_some(job_id);
        }
    }
    None
}

#[derive(Clone)]
pub struct GitHubClient {
    client: reqwest::Client,
    api_base: Url,
    token: InstallationToken,
}

impl GitHubClient {
    pub fn new(api_base: Url, token: InstallationToken) -> Result<Self, GitHubClientError> {
        let client = reqwest::Client::builder()
            .user_agent("PiTools/0.1")
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| GitHubClientError::Client(error.to_string()))?;
        Ok(Self {
            client,
            api_base,
            token,
        })
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, GitHubClientError> {
        let url = self
            .api_base
            .join(path)
            .map_err(|error| GitHubClientError::Client(error.to_string()))?;
        let response = self.send_get(url).await?;
        decode_response(response, self.token.token.expose_secret()).await
    }

    async fn send_get(&self, url: Url) -> Result<reqwest::Response, GitHubClientError> {
        self.client
            .get(url)
            .bearer_auth(self.token.token.expose_secret())
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .send()
            .await
            .map_err(|error| {
                GitHubClientError::Request(redact(
                    error.to_string(),
                    self.token.token.expose_secret(),
                ))
            })
    }

    pub async fn post_json<T: DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, GitHubClientError> {
        let url = self
            .api_base
            .join(path)
            .map_err(|error| GitHubClientError::Client(error.to_string()))?;
        let response = self
            .client
            .post(url)
            .bearer_auth(self.token.token.expose_secret())
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .json(body)
            .send()
            .await
            .map_err(|error| GitHubClientError::Request(error.to_string()))?;
        decode_response(response, self.token.token.expose_secret()).await
    }

    pub async fn patch_json<T: DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, GitHubClientError> {
        let url = self
            .api_base
            .join(path)
            .map_err(|error| GitHubClientError::Client(error.to_string()))?;
        let response = self
            .client
            .patch(url)
            .bearer_auth(self.token.token.expose_secret())
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .json(body)
            .send()
            .await
            .map_err(|error| GitHubClientError::Request(error.to_string()))?;
        decode_response(response, self.token.token.expose_secret()).await
    }

    async fn get_paginated<T, P>(&self, path: &str) -> Result<Vec<T>, GitHubClientError>
    where
        T: DeserializeOwned,
        P: DeserializeOwned + PageItems<T>,
    {
        let mut url = self
            .api_base
            .join(path)
            .map_err(|error| GitHubClientError::Client(error.to_string()))?;
        let mut visited = BTreeSet::new();
        let mut items = Vec::new();

        for _ in 0..MAX_PAGINATION_PAGES {
            if !visited.insert(url.as_str().to_owned()) {
                return Err(GitHubClientError::PaginationLoop);
            }
            let response = self.send_get(url).await?;
            let next = next_link(response.headers())?;
            let page = decode_response::<P>(response, self.token.token.expose_secret()).await?;
            items.extend(page.into_items());

            let Some(candidate) = next else {
                return Ok(items);
            };
            if !self.is_safe_pagination_url(&candidate) {
                return Err(GitHubClientError::UnsafePagination);
            }
            url = candidate;
        }

        Err(GitHubClientError::PaginationLimit)
    }

    async fn get_bounded_json<T: DeserializeOwned>(
        &self,
        path: &str,
        max_bytes: usize,
        too_large: GitHubClientError,
    ) -> Result<T, GitHubClientError> {
        let url = self
            .api_base
            .join(path)
            .map_err(|error| GitHubClientError::Client(error.to_string()))?;
        let response = self.send_get(url).await?;
        let body = read_bounded_body(
            response,
            self.token.token.expose_secret(),
            max_bytes,
            too_large,
        )
        .await?;
        serde_json::from_slice(&body)
            .map_err(|error| GitHubClientError::Response(error.to_string()))
    }

    fn is_safe_pagination_url(&self, candidate: &Url) -> bool {
        candidate.scheme() == self.api_base.scheme()
            && candidate.host_str() == self.api_base.host_str()
            && candidate.port_or_known_default() == self.api_base.port_or_known_default()
            && candidate.username().is_empty()
            && candidate.password().is_none()
            && candidate.path().starts_with(self.api_base.path())
    }

    pub async fn get_pull_request(
        &self,
        owner: &str,
        repository: &str,
        pull_request_number: i32,
    ) -> Result<GitHubPullRequest, GitHubClientError> {
        self.get_json(&format!(
            "repos/{owner}/{repository}/pulls/{pull_request_number}"
        ))
        .await
    }

    pub async fn list_open_pull_requests(
        &self,
        owner: &str,
        repository: &str,
    ) -> Result<Vec<GitHubOpenPullRequest>, GitHubClientError> {
        self.get_paginated::<GitHubOpenPullRequest, Vec<GitHubOpenPullRequest>>(&format!(
            "repos/{owner}/{repository}/pulls?state=open&per_page=100&page=1"
        ))
        .await
    }

    pub async fn list_pull_request_reviews(
        &self,
        owner: &str,
        repository: &str,
        pull_request_number: i32,
    ) -> Result<Vec<PullRequestReview>, GitHubClientError> {
        self.get_paginated::<PullRequestReview, Vec<PullRequestReview>>(&format!(
            "repos/{owner}/{repository}/pulls/{pull_request_number}/reviews?per_page=100&page=1"
        ))
        .await
    }

    pub async fn list_issue_comments(
        &self,
        owner: &str,
        repository: &str,
        pull_request_number: i32,
    ) -> Result<Vec<PullRequestComment>, GitHubClientError> {
        self.get_paginated::<PullRequestComment, Vec<PullRequestComment>>(&format!(
            "repos/{owner}/{repository}/issues/{pull_request_number}/comments?per_page=100&page=1"
        ))
        .await
    }

    pub async fn list_review_comments(
        &self,
        owner: &str,
        repository: &str,
        pull_request_number: i32,
    ) -> Result<Vec<PullRequestComment>, GitHubClientError> {
        self.get_paginated::<PullRequestComment, Vec<PullRequestComment>>(&format!(
            "repos/{owner}/{repository}/pulls/{pull_request_number}/comments?per_page=100&page=1"
        ))
        .await
    }

    pub async fn list_check_runs(
        &self,
        owner: &str,
        repository: &str,
        head_sha: &str,
    ) -> Result<Vec<GitHubCheckRun>, GitHubClientError> {
        self.get_paginated::<GitHubCheckRun, CheckRunsPage>(&format!(
            "repos/{owner}/{repository}/commits/{head_sha}/check-runs?filter=latest&per_page=100&page=1"
        ))
        .await
    }

    pub async fn get_check_run(
        &self,
        owner: &str,
        repository: &str,
        check_run_id: i64,
    ) -> Result<GitHubCheckRun, GitHubClientError> {
        self.get_json(&format!(
            "repos/{owner}/{repository}/check-runs/{check_run_id}"
        ))
        .await
    }

    pub async fn download_workflow_job_logs(
        &self,
        owner: &str,
        repository: &str,
        job_id: i64,
    ) -> Result<String, GitHubClientError> {
        let url = self
            .api_base
            .join(&format!(
                "repos/{owner}/{repository}/actions/jobs/{job_id}/logs"
            ))
            .map_err(|error| GitHubClientError::Client(error.to_string()))?;
        let response = self.send_get(url).await?;
        let body = read_bounded_body(
            response,
            self.token.token.expose_secret(),
            MAX_ACTIONS_LOG_BYTES,
            GitHubClientError::WorkflowLogTooLarge,
        )
        .await?;
        String::from_utf8(body).map_err(|_| GitHubClientError::WorkflowLogNotUtf8)
    }

    pub async fn list_check_run_annotations(
        &self,
        owner: &str,
        repository: &str,
        check_run_id: i64,
    ) -> Result<Vec<GitHubCheckAnnotation>, GitHubClientError> {
        let annotations: Vec<GitHubCheckAnnotation> = self
            .get_bounded_json(
                &format!(
                    "repos/{owner}/{repository}/check-runs/{check_run_id}/annotations?per_page=100&page=1"
                ),
                MAX_CHECK_ANNOTATION_BYTES,
                GitHubClientError::CheckAnnotationsTooLarge,
            )
            .await?;
        Ok(annotations
            .into_iter()
            .take(MAX_CHECK_RUN_ANNOTATIONS)
            .collect())
    }

    pub async fn get_combined_status(
        &self,
        owner: &str,
        repository: &str,
        head_sha: &str,
    ) -> Result<CombinedStatus, GitHubClientError> {
        self.get_json(&format!(
            "repos/{owner}/{repository}/commits/{head_sha}/status"
        ))
        .await
    }

    pub async fn get_branch(
        &self,
        owner: &str,
        repository: &str,
        branch: &str,
    ) -> Result<Branch, GitHubClientError> {
        self.get_json(&format!("repos/{owner}/{repository}/branches/{branch}"))
            .await
    }

    pub async fn get_branch_protection(
        &self,
        owner: &str,
        repository: &str,
        branch: &str,
    ) -> Result<BranchProtection, GitHubClientError> {
        let url = self
            .api_base
            .join(&format!(
                "repos/{owner}/{repository}/branches/{branch}/protection"
            ))
            .map_err(|error| GitHubClientError::Client(error.to_string()))?;
        let response = self.send_get(url).await?;
        decode_response(response, self.token.token.expose_secret()).await
    }

    pub async fn get_repository_policy(
        &self,
        owner: &str,
        repository: &str,
        reference: &str,
    ) -> Result<Option<String>, GitHubClientError> {
        let mut url = self
            .api_base
            .join(&format!("repos/{owner}/{repository}/contents/.pitools.yml"))
            .map_err(|error| GitHubClientError::Client(error.to_string()))?;
        url.query_pairs_mut().append_pair("ref", reference);
        let response = self
            .client
            .get(url)
            .bearer_auth(self.token.token.expose_secret())
            .header(header::ACCEPT, "application/vnd.github.raw+json")
            .header("x-github-api-version", "2022-11-28")
            .send()
            .await
            .map_err(|error| {
                GitHubClientError::Request(redact(
                    error.to_string(),
                    self.token.token.expose_secret(),
                ))
            })?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            let status = response.status();
            return Err(GitHubClientError::Api {
                status: status.as_u16(),
                body: redact(
                    response
                        .text()
                        .await
                        .unwrap_or_else(|_| "unreadable response".into()),
                    self.token.token.expose_secret(),
                ),
            });
        }
        let mut body = Vec::new();
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| GitHubClientError::Response(error.to_string()))?
        {
            if body.len() + chunk.len() > MAX_REPOSITORY_POLICY_BYTES {
                return Err(GitHubClientError::RepositoryFileTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        String::from_utf8(body)
            .map(Some)
            .map_err(|error| GitHubClientError::Response(error.to_string()))
    }

    pub async fn get_repository_file(
        &self,
        owner: &str,
        repository: &str,
        path: &str,
        reference: &str,
        max_bytes: usize,
    ) -> Result<Option<String>, GitHubClientError> {
        validate_repository_file_path(path)?;
        if max_bytes == 0 {
            return Err(GitHubClientError::RepositoryFileTooLarge);
        }
        let mut url = self
            .api_base
            .join(&format!("repos/{owner}/{repository}/contents"))
            .map_err(|error| GitHubClientError::Client(error.to_string()))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| GitHubClientError::Client("GitHub API URL cannot be a base".into()))?;
            for segment in path.split('/') {
                segments.push(segment);
            }
        }
        url.query_pairs_mut().append_pair("ref", reference);
        let response = self
            .client
            .get(url)
            .bearer_auth(self.token.token.expose_secret())
            .header(header::ACCEPT, "application/vnd.github.raw+json")
            .header("x-github-api-version", "2022-11-28")
            .send()
            .await
            .map_err(|error| {
                GitHubClientError::Request(redact(
                    error.to_string(),
                    self.token.token.expose_secret(),
                ))
            })?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            let status = response.status();
            return Err(GitHubClientError::Api {
                status: status.as_u16(),
                body: redact(
                    response
                        .text()
                        .await
                        .unwrap_or_else(|_| "unreadable response".into()),
                    self.token.token.expose_secret(),
                ),
            });
        }
        let mut body = Vec::new();
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| GitHubClientError::Response(error.to_string()))?
        {
            if body.len() + chunk.len() > max_bytes {
                return Err(GitHubClientError::RepositoryFileTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        String::from_utf8(body)
            .map(Some)
            .map_err(|error| GitHubClientError::Response(error.to_string()))
    }

    pub async fn read_pull_request(
        &self,
        owner: &str,
        repository: &str,
        pull_request_number: i32,
    ) -> Result<PullRequestReadSet, GitHubClientError> {
        let pull_request = self
            .get_pull_request(owner, repository, pull_request_number)
            .await?;
        let base_branch_name = pull_request.base.reference.as_deref().ok_or_else(|| {
            GitHubClientError::Response("pull request base ref is missing".into())
        })?;
        let base_branch = self.get_branch(owner, repository, base_branch_name).await?;
        let branch_protection = if base_branch.protected {
            self.get_branch_protection(owner, repository, base_branch_name)
                .await?
        } else {
            BranchProtection::default()
        };
        let reviews = self
            .list_pull_request_reviews(owner, repository, pull_request_number)
            .await?;
        let issue_comments = self
            .list_issue_comments(owner, repository, pull_request_number)
            .await?;
        let review_comments = self
            .list_review_comments(owner, repository, pull_request_number)
            .await?;
        let check_runs = self
            .list_check_runs(owner, repository, &pull_request.head.sha)
            .await?;
        let combined_status = self
            .get_combined_status(owner, repository, &pull_request.head.sha)
            .await?;

        Ok(PullRequestReadSet {
            pull_request,
            base_branch,
            branch_protection,
            reviews,
            issue_comments,
            review_comments,
            check_runs,
            combined_status,
        })
    }

    pub async fn create_issue_comment(
        &self,
        owner: &str,
        repository: &str,
        issue_number: i32,
        body: &str,
    ) -> Result<GitHubComment, GitHubClientError> {
        self.post_json(
            &format!("repos/{owner}/{repository}/issues/{issue_number}/comments"),
            &IssueCommentRequest { body },
        )
        .await
    }

    pub async fn update_issue_comment(
        &self,
        owner: &str,
        repository: &str,
        comment_id: i64,
        body: &str,
    ) -> Result<GitHubComment, GitHubClientError> {
        self.patch_json(
            &format!("repos/{owner}/{repository}/issues/comments/{comment_id}"),
            &IssueCommentRequest { body },
        )
        .await
    }

    pub async fn reply_to_review_comment(
        &self,
        owner: &str,
        repository: &str,
        pull_request_number: i32,
        comment_id: i64,
        body: &str,
    ) -> Result<GitHubComment, GitHubClientError> {
        self.post_json(
            &format!(
                "repos/{owner}/{repository}/pulls/{pull_request_number}/comments/{comment_id}/replies"
            ),
            &IssueCommentRequest { body },
        )
        .await
    }

    pub async fn update_pull_request_base(
        &self,
        owner: &str,
        repository: &str,
        pull_request_number: i32,
        base_branch: &str,
    ) -> Result<GitHubPullRequest, GitHubClientError> {
        if base_branch.is_empty() || base_branch.contains(['\0', '\r', '\n']) {
            return Err(GitHubClientError::Response(
                "pull request base branch is invalid".into(),
            ));
        }
        self.patch_json(
            &format!("repos/{owner}/{repository}/pulls/{pull_request_number}"),
            &PullRequestBaseUpdate { base: base_branch },
        )
        .await
    }

    pub async fn review_thread_id_for_comment(
        &self,
        owner: &str,
        repository: &str,
        pull_request_number: i32,
        comment_id: i64,
    ) -> Result<Option<String>, GitHubClientError> {
        let response: GraphQlResponse<ReviewThreadQueryData> = self
            .post_json(
                "graphql",
                &GraphQlRequest {
                    query: REVIEW_THREADS_QUERY,
                    variables: serde_json::json!({
                        "owner": owner,
                        "repository": repository,
                        "number": pull_request_number,
                    }),
                },
            )
            .await?;
        let thread_id = response.into_data()?.and_then(|data| {
            data.repository
                .pull_request
                .review_threads
                .nodes
                .into_iter()
                .find(|thread| {
                    thread
                        .comments
                        .nodes
                        .iter()
                        .any(|comment| comment.database_id == comment_id)
                })
                .map(|thread| thread.id)
        });
        Ok(thread_id)
    }

    pub async fn resolve_review_thread(&self, thread_id: &str) -> Result<(), GitHubClientError> {
        let response: GraphQlResponse<ResolveThreadMutationData> = self
            .post_json(
                "graphql",
                &GraphQlRequest {
                    query: RESOLVE_REVIEW_THREAD_MUTATION,
                    variables: serde_json::json!({"threadId": thread_id}),
                },
            )
            .await?;
        let thread = response
            .into_data()?
            .and_then(|data| data.resolve_review_thread.thread)
            .ok_or_else(|| {
                GitHubClientError::Response("GitHub did not resolve the review thread".into())
            })?;
        if thread.is_resolved {
            Ok(())
        } else {
            Err(GitHubClientError::Response(
                "GitHub returned an unresolved review thread".into(),
            ))
        }
    }

    pub async fn create_check_run(
        &self,
        owner: &str,
        repository: &str,
        request: &CheckRunRequest,
    ) -> Result<CheckRun, GitHubClientError> {
        self.post_json(&format!("repos/{owner}/{repository}/check-runs"), request)
            .await
    }

    pub async fn update_check_run(
        &self,
        owner: &str,
        repository: &str,
        check_run_id: i64,
        request: &CheckRunUpdate,
    ) -> Result<CheckRun, GitHubClientError> {
        self.patch_json(
            &format!("repos/{owner}/{repository}/check-runs/{check_run_id}"),
            request,
        )
        .await
    }
}

#[derive(Debug, Clone, Serialize)]
struct IssueCommentRequest<'a> {
    body: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct PullRequestBaseUpdate<'a> {
    base: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubComment {
    pub id: i64,
    pub body: Option<String>,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckRunRequest {
    pub name: String,
    pub head_sha: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details_url: Option<String>,
    pub output: CheckRunOutput,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<CheckRunAction>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckRunUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<CheckRunOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<CheckRunAction>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckRunOutput {
    pub title: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckRunAction {
    pub label: String,
    pub description: String,
    pub identifier: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheckRun {
    pub id: i64,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitHubPullRequest {
    pub number: i32,
    pub draft: bool,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub merged: Option<bool>,
    #[serde(default)]
    pub mergeable_state: Option<String>,
    pub mergeable: Option<bool>,
    pub head: GitHubPullRequestBranch,
    pub base: GitHubPullRequestBranch,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitHubOpenPullRequest {
    pub id: i64,
    pub number: i32,
    pub title: String,
    pub html_url: String,
    pub state: String,
    pub draft: bool,
    #[serde(default)]
    pub merged: bool,
    pub head: GitHubPullRequestBranch,
    pub base: GitHubPullRequestBranch,
    pub user: GitHubCommentAuthor,
    #[serde(default)]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitHubPullRequestBranch {
    pub sha: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    #[serde(rename = "repo", default)]
    pub repository: Option<GitHubRepositoryRef>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitHubRepositoryRef {
    pub full_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitHubCommentAuthor {
    pub login: String,
    #[serde(rename = "type")]
    pub actor_type: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PullRequestReview {
    pub id: i64,
    pub body: Option<String>,
    pub state: String,
    pub submitted_at: Option<String>,
    #[serde(default)]
    pub commit_id: Option<String>,
    pub user: GitHubCommentAuthor,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PullRequestComment {
    pub id: i64,
    pub body: Option<String>,
    pub user: GitHubCommentAuthor,
    pub path: Option<String>,
    pub start_line: Option<u32>,
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitHubCheckRun {
    pub id: i64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub details_url: Option<String>,
    pub app: Option<CheckApp>,
    pub output: Option<GitHubCheckRunOutput>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitHubCheckRunOutput {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubCheckAnnotation {
    pub path: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub start_column: Option<u32>,
    pub end_column: Option<u32>,
    pub annotation_level: Option<String>,
    pub message: String,
    pub title: Option<String>,
    pub raw_details: Option<String>,
    pub blob_href: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CheckApp {
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CombinedStatus {
    pub state: String,
    pub sha: String,
    pub statuses: Vec<CommitStatus>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CommitStatus {
    pub id: i64,
    pub context: String,
    pub state: String,
    pub target_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub protected: bool,
    pub commit: BranchCommit,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BranchCommit {
    pub sha: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct BranchProtection {
    pub required_status_checks: Option<RequiredStatusChecks>,
    #[serde(default)]
    pub required_pull_request_reviews: Option<RequiredPullRequestReviews>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct RequiredPullRequestReviews {
    #[serde(default)]
    pub dismiss_stale_reviews: bool,
    #[serde(default)]
    pub require_code_owner_reviews: bool,
    #[serde(default)]
    pub required_approving_review_count: usize,
    #[serde(default)]
    pub require_last_push_approval: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RequiredStatusChecks {
    pub strict: bool,
    #[serde(default)]
    pub contexts: Vec<String>,
    #[serde(default)]
    pub checks: Vec<RequiredStatusCheck>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RequiredStatusCheck {
    pub context: String,
    pub app_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestReadSet {
    pub pull_request: GitHubPullRequest,
    pub base_branch: Branch,
    pub branch_protection: BranchProtection,
    pub reviews: Vec<PullRequestReview>,
    pub issue_comments: Vec<PullRequestComment>,
    pub review_comments: Vec<PullRequestComment>,
    pub check_runs: Vec<GitHubCheckRun>,
    pub combined_status: CombinedStatus,
}

trait PageItems<T> {
    fn into_items(self) -> Vec<T>;
}

impl<T> PageItems<T> for Vec<T> {
    fn into_items(self) -> Vec<T> {
        self
    }
}

#[derive(Deserialize)]
struct CheckRunsPage {
    check_runs: Vec<GitHubCheckRun>,
}

const REVIEW_THREADS_QUERY: &str = r#"
query($owner: String!, $repository: String!, $number: Int!) {
  repository(owner: $owner, name: $repository) {
    pullRequest(number: $number) {
      reviewThreads(first: 100) {
        nodes {
          id
          comments(first: 100) {
            nodes { databaseId }
          }
        }
      }
    }
  }
}
"#;

const RESOLVE_REVIEW_THREAD_MUTATION: &str = r#"
mutation($threadId: ID!) {
  resolveReviewThread(input: {threadId: $threadId}) {
    thread { isResolved }
  }
}
"#;

#[derive(Debug, Serialize)]
struct GraphQlRequest {
    query: &'static str,
    variables: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct GraphQlResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQlError>>,
}

impl<T> GraphQlResponse<T> {
    fn into_data(self) -> Result<Option<T>, GitHubClientError> {
        if let Some(errors) = self.errors.filter(|errors| !errors.is_empty()) {
            return Err(GitHubClientError::Response(
                errors
                    .into_iter()
                    .map(|error| error.message)
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        Ok(self.data)
    }
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct ReviewThreadQueryData {
    repository: ReviewThreadRepository,
}

#[derive(Debug, Deserialize)]
struct ReviewThreadRepository {
    #[serde(rename = "pullRequest")]
    pull_request: ReviewThreadPullRequest,
}

#[derive(Debug, Deserialize)]
struct ReviewThreadPullRequest {
    #[serde(rename = "reviewThreads")]
    review_threads: ReviewThreads,
}

#[derive(Debug, Deserialize)]
struct ReviewThreads {
    nodes: Vec<ReviewThread>,
}

#[derive(Debug, Deserialize)]
struct ReviewThread {
    id: String,
    comments: ReviewComments,
}

#[derive(Debug, Deserialize)]
struct ReviewComments {
    nodes: Vec<ReviewCommentId>,
}

#[derive(Debug, Deserialize)]
struct ReviewCommentId {
    #[serde(rename = "databaseId")]
    database_id: i64,
}

#[derive(Debug, Deserialize)]
struct ResolveThreadMutationData {
    #[serde(rename = "resolveReviewThread")]
    resolve_review_thread: ResolveReviewThread,
}

#[derive(Debug, Deserialize)]
struct ResolveReviewThread {
    thread: Option<ResolvedThread>,
}

#[derive(Debug, Deserialize)]
struct ResolvedThread {
    #[serde(rename = "isResolved")]
    is_resolved: bool,
}

impl PageItems<GitHubCheckRun> for CheckRunsPage {
    fn into_items(self) -> Vec<GitHubCheckRun> {
        self.check_runs
    }
}

fn next_link(headers: &HeaderMap) -> Result<Option<Url>, GitHubClientError> {
    for value in headers.get_all(header::LINK) {
        let value = value
            .to_str()
            .map_err(|_| GitHubClientError::Response("invalid pagination link header".into()))?;
        for link in value.split(',') {
            let mut parts = link.split(';').map(str::trim);
            let Some(target) = parts.next() else {
                continue;
            };
            if !parts.any(|part| part == "rel=\"next\"") {
                continue;
            }
            let target = target
                .strip_prefix('<')
                .and_then(|value| value.strip_suffix('>'))
                .ok_or_else(|| {
                    GitHubClientError::Response("invalid pagination link header".into())
                })?;
            return Url::parse(target)
                .map(Some)
                .map_err(|_| GitHubClientError::Response("invalid pagination link URL".into()));
        }
    }
    Ok(None)
}

async fn read_bounded_body(
    response: reqwest::Response,
    token: &str,
    max_bytes: usize,
    too_large: GitHubClientError,
) -> Result<Vec<u8>, GitHubClientError> {
    let status = response.status();
    if !status.is_success() {
        return Err(GitHubClientError::Api {
            status: status.as_u16(),
            body: redact(
                response
                    .text()
                    .await
                    .unwrap_or_else(|_| "unreadable response".into()),
                token,
            ),
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(too_large);
    }

    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| GitHubClientError::Response(redact(error.to_string(), token)))?
    {
        if body.len() + chunk.len() > max_bytes {
            return Err(too_large);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn decode_response<T: DeserializeOwned>(
    response: reqwest::Response,
    token: &str,
) -> Result<T, GitHubClientError> {
    let status = response.status();
    if !status.is_success() {
        return Err(GitHubClientError::Api {
            status: status.as_u16(),
            body: redact(
                response
                    .text()
                    .await
                    .unwrap_or_else(|_| "unreadable response".into()),
                token,
            ),
        });
    }
    response
        .json::<T>()
        .await
        .map_err(|error| GitHubClientError::Response(error.to_string()))
}

fn redact(value: String, token: &str) -> String {
    let redacted = if token.is_empty() {
        value
    } else {
        value.replace(token, "[REDACTED]")
    };
    redacted.chars().take(4096).collect()
}

fn validate_repository_file_path(path: &str) -> Result<(), GitHubClientError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(['\\', '\0', '\r', '\n'])
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(GitHubClientError::Response(
            "repository file path is not canonical".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum GitHubClientError {
    #[error("GitHub client setup failed: {0}")]
    Client(String),
    #[error("GitHub request failed: {0}")]
    Request(String),
    #[error("GitHub response decoding failed: {0}")]
    Response(String),
    #[error("GitHub returned HTTP {status}: {body}")]
    Api { status: u16, body: String },
    #[error("GitHub pagination link left the configured API base")]
    UnsafePagination,
    #[error("GitHub pagination repeated a page URL")]
    PaginationLoop,
    #[error("GitHub pagination exceeded the page limit")]
    PaginationLimit,
    #[error("GitHub repository policy file exceeds the size limit")]
    RepositoryFileTooLarge,
    #[error("GitHub Actions job log exceeds the size limit")]
    WorkflowLogTooLarge,
    #[error("GitHub Actions job log is not valid UTF-8")]
    WorkflowLogNotUtf8,
    #[error("GitHub check annotations exceed the size limit")]
    CheckAnnotationsTooLarge,
}
