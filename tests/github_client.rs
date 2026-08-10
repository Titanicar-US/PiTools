use pitools::github::{
    auth::InstallationToken,
    client::{GitHubClient, GitHubClientError, actions_job_id_from_details_url},
};
use secrecy::SecretString;
use serde_json::json;
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, header, method, path, query_param},
};

const TOKEN: &str = "installation-secret-token";

fn installation_token() -> InstallationToken {
    InstallationToken {
        token: SecretString::from(TOKEN),
        expires_at: "2030-01-01T00:00:00Z".into(),
        permissions: json!({"pull_requests": "read"}),
        repository_selection: Some("selected".into()),
    }
}

fn client(server: &MockServer) -> GitHubClient {
    GitHubClient::new(
        Url::parse(&format!("{}/", server.uri())).expect("mock URL is valid"),
        installation_token(),
    )
    .expect("client is valid")
}

#[tokio::test]
async fn reads_typed_pull_request_reconciliation_data() {
    let server = MockServer::start().await;
    let authenticated_get = || {
        Mock::given(method("GET"))
            .and(header("authorization", format!("Bearer {TOKEN}")))
            .and(header("x-github-api-version", "2022-11-28"))
    };

    authenticated_get()
        .and(path("/repos/acme/widgets/pulls/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 7,
            "draft": false,
            "mergeable": true,
            "head": {"sha": "head-sha"},
            "base": {"sha": "base-at-open", "ref": "main"}
        })))
        .mount(&server)
        .await;
    authenticated_get()
        .and(path("/repos/acme/widgets/branches/main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "main",
            "protected": true,
            "commit": {"sha": "base-current"}
        })))
        .mount(&server)
        .await;
    authenticated_get()
        .and(path("/repos/acme/widgets/branches/main/protection"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "required_status_checks": {
                "strict": true,
                "contexts": ["legacy/status"],
                "checks": [{"context": "build", "app_id": 42}]
            }
        })))
        .mount(&server)
        .await;
    authenticated_get()
        .and(path("/repos/acme/widgets/commits/head-sha/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "state": "success",
            "sha": "head-sha",
            "statuses": [{
                "id": 91,
                "context": "legacy/status",
                "state": "success",
                "target_url": "https://ci.example/status/91"
            }]
        })))
        .mount(&server)
        .await;
    authenticated_get()
        .and(path("/repos/acme/widgets/commits/head-sha/check-runs"))
        .and(query_param("filter", "latest"))
        .and(query_param("per_page", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 1,
            "check_runs": [{
                "id": 81,
                "name": "build",
                "status": "completed",
                "conclusion": "success",
                "details_url": "https://ci.example/check/81",
                "app": {"id": 42}
            }]
        })))
        .mount(&server)
        .await;
    for (endpoint, body) in [
        (
            "/repos/acme/widgets/pulls/7/reviews",
            json!([{
                "id": 51,
                "body": "approved",
                "state": "APPROVED",
                "submitted_at": "2026-08-10T00:00:00Z",
                "user": {"login": "reviewer", "type": "User"}
            }]),
        ),
        (
            "/repos/acme/widgets/issues/7/comments",
            json!([{
                "id": 61,
                "body": "please update docs",
                "user": {"login": "maintainer", "type": "User"}
            }]),
        ),
        (
            "/repos/acme/widgets/pulls/7/comments",
            json!([{
                "id": 71,
                "body": "fix this line",
                "user": {"login": "review-bot[bot]", "type": "Bot"}
            }]),
        ),
    ] {
        authenticated_get()
            .and(path(endpoint))
            .and(query_param("per_page", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }

    let data = client(&server)
        .read_pull_request("acme", "widgets", 7)
        .await
        .expect("typed reads succeed");

    assert_eq!(data.pull_request.head.sha, "head-sha");
    assert_eq!(data.base_branch.commit.sha, "base-current");
    assert!(data.base_branch.protected);
    assert_eq!(data.reviews[0].user.login, "reviewer");
    assert_eq!(data.issue_comments[0].id, 61);
    assert_eq!(data.review_comments[0].id, 71);
    assert_eq!(data.check_runs[0].app.as_ref().map(|app| app.id), Some(42));
    assert_eq!(data.combined_status.statuses[0].context, "legacy/status");
    assert_eq!(
        data.branch_protection
            .required_status_checks
            .as_ref()
            .expect("required checks")
            .checks[0]
            .context,
        "build"
    );
}

#[tokio::test]
async fn inventories_open_pull_requests_for_recovery_after_a_missed_webhook() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/pulls"))
        .and(query_param("state", "open"))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "id": 3,
            "number": 7,
            "title": "PR",
            "html_url": "https://github.com/acme/widgets/pull/7",
            "state": "open",
            "draft": false,
            "head": {"sha": "head", "ref": "feature", "repo": {"full_name": "acme/widgets"}},
            "base": {"sha": "base", "ref": "main", "repo": {"full_name": "acme/widgets"}},
            "user": {"login": "author", "type": "User"},
            "updated_at": "2026-08-10T01:02:03Z"
        }])))
        .mount(&server)
        .await;

    let pull_requests = client(&server)
        .list_open_pull_requests("acme", "widgets")
        .await
        .expect("inventory succeeds");
    assert_eq!(pull_requests.len(), 1);
    assert_eq!(pull_requests[0].head.reference.as_deref(), Some("feature"));
    assert_eq!(pull_requests[0].user.login, "author");
}

#[tokio::test]
async fn paginates_list_reads_without_dropping_requested_results() {
    let server = MockServer::start().await;
    let next = format!(
        "<{}/repos/acme/widgets/pulls/7/reviews?per_page=100&page=2>; rel=\"next\"",
        server.uri()
    );
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/pulls/7/reviews"))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", next)
                .set_body_json(json!([{
                    "id": 1,
                    "body": null,
                    "state": "COMMENTED",
                    "submitted_at": "2026-08-09T00:00:00Z",
                    "user": {"login": "first", "type": "User"}
                }])),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/pulls/7/reviews"))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "id": 2,
            "body": null,
            "state": "APPROVED",
            "submitted_at": "2026-08-10T00:00:00Z",
            "user": {"login": "second", "type": "User"}
        }])))
        .mount(&server)
        .await;

    let reviews = client(&server)
        .list_pull_request_reviews("acme", "widgets", 7)
        .await
        .expect("all pages are read");

    assert_eq!(
        reviews.iter().map(|review| review.id).collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[tokio::test]
async fn rejects_off_origin_pagination_without_forwarding_authorization() {
    let github = MockServer::start().await;
    let attacker = MockServer::start().await;
    let next = format!("<{}/steal?page=2>; rel=\"next\"", attacker.uri());
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/pulls/7/reviews"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", next)
                .set_body_json(json!([])),
        )
        .mount(&github)
        .await;

    let error = client(&github)
        .list_pull_request_reviews("acme", "widgets", 7)
        .await
        .expect_err("off-origin pagination must fail closed");

    assert!(matches!(error, GitHubClientError::UnsafePagination));
    assert!(
        attacker
            .received_requests()
            .await
            .expect("request log")
            .is_empty()
    );
}

#[tokio::test]
async fn redacts_installation_token_from_api_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/pulls/7"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_string(format!("upstream echoed Authorization: Bearer {TOKEN}")),
        )
        .mount(&server)
        .await;

    let error = client(&server)
        .get_pull_request("acme", "widgets", 7)
        .await
        .expect_err("HTTP error is surfaced");
    let displayed = error.to_string();

    assert!(displayed.contains("HTTP 500"));
    assert!(!displayed.contains(TOKEN));
    assert!(displayed.contains("[REDACTED]"));
}

#[tokio::test]
async fn reads_the_repository_policy_as_bounded_raw_text() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/contents/.pitools.yml"))
        .and(query_param("ref", "main"))
        .and(header("authorization", format!("Bearer {TOKEN}")))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "automation_actors:\n  - review-bot[bot]\nvalidation_commands:\n  - git_diff_check\n",
        ))
        .mount(&server)
        .await;

    let policy = client(&server)
        .get_repository_policy("acme", "widgets", "main")
        .await
        .expect("policy read succeeds")
        .expect("policy exists");
    assert!(policy.contains("review-bot[bot]"));
}

#[tokio::test]
async fn reads_an_allowlisted_repository_file_at_the_pull_request_head() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/contents/src/main.rs"))
        .and(query_param("ref", "head-sha"))
        .and(header("authorization", format!("Bearer {TOKEN}")))
        .respond_with(ResponseTemplate::new(200).set_body_string("fn main() {}\n"))
        .mount(&server)
        .await;

    let content = client(&server)
        .get_repository_file("acme", "widgets", "src/main.rs", "head-sha", 1024)
        .await
        .expect("repository file read succeeds")
        .expect("repository file exists");
    assert_eq!(content, "fn main() {}\n");
}

#[tokio::test]
async fn resolves_review_threads_through_the_graphql_api() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("reviewThreads"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "repository": {
                    "pullRequest": {
                        "reviewThreads": {
                            "nodes": [{
                                "id": "thread-node-1",
                                "comments": {"nodes": [{"databaseId": 71}]}
                            }]
                        }
                    }
                }
            }
        })))
        .mount(&server)
        .await;
    let thread = client(&server)
        .review_thread_id_for_comment("acme", "widgets", 7, 71)
        .await
        .expect("thread lookup")
        .expect("thread exists");
    assert_eq!(thread, "thread-node-1");

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("resolveReviewThread"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"resolveReviewThread": {"thread": {"isResolved": true}}}
        })))
        .mount(&server)
        .await;
    client(&server)
        .resolve_review_thread("thread-node-1")
        .await
        .expect("thread resolves");
}

#[tokio::test]
async fn replies_to_a_review_comment_through_the_review_reply_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/widgets/pulls/7/comments/71/replies"))
        .and(body_string_contains(
            "\"body\":\"PiTools applied the suggestion\"",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": 72,
            "body": "PiTools applied the suggestion",
            "html_url": "https://github.com/acme/widgets/pull/7#discussion_r72"
        })))
        .mount(&server)
        .await;

    let reply = client(&server)
        .reply_to_review_comment("acme", "widgets", 7, 71, "PiTools applied the suggestion")
        .await
        .expect("review reply succeeds");
    assert_eq!(reply.id, 72);
    assert_eq!(
        reply.body.as_deref(),
        Some("PiTools applied the suggestion")
    );
}

#[tokio::test]
async fn updates_a_pull_request_base_branch_with_a_typed_patch_request() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/widgets/pulls/7"))
        .and(body_string_contains("\"base\":\"feature/parent\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 7,
            "draft": false,
            "state": "open",
            "merged": false,
            "head": {"sha": "head", "ref": "feature/child", "repo": {"full_name": "acme/widgets"}},
            "base": {"sha": "parent", "ref": "feature/parent"}
        })))
        .mount(&server)
        .await;

    let updated = client(&server)
        .update_pull_request_base("acme", "widgets", 7, "feature/parent")
        .await
        .expect("base update succeeds");
    assert_eq!(updated.base.reference.as_deref(), Some("feature/parent"));
}

#[tokio::test]
async fn reads_bounded_check_run_output_for_ci_diagnosis() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/check-runs/81"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 81,
            "name": "build",
            "status": "completed",
            "conclusion": "failure",
            "details_url": "https://ci.example/check/81",
            "output": {
                "title": "Build failed",
                "summary": "compiler error",
                "text": "src/lib.rs:1:1 error"
            }
        })))
        .mount(&server)
        .await;

    let check = client(&server)
        .get_check_run("acme", "widgets", 81)
        .await
        .expect("check run read succeeds");
    let output = check.output.expect("output is present");
    assert_eq!(output.summary.as_deref(), Some("compiler error"));
    assert_eq!(output.text.as_deref(), Some("src/lib.rs:1:1 error"));
}

#[tokio::test]
async fn reads_actions_job_logs_and_check_run_annotations_for_ci_diagnosis() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/actions/jobs/399/logs"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("Run cargo test\nerror: assertion failed\n"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/check-runs/81/annotations"))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "path": "src/lib.rs",
                "start_line": 4,
                "end_line": 4,
                "annotation_level": "failure",
                "message": "assertion failed",
                "title": "cargo test"
            }
        ])))
        .mount(&server)
        .await;

    let logs = client(&server)
        .download_workflow_job_logs("acme", "widgets", 399)
        .await
        .expect("workflow logs download succeeds");
    assert!(logs.contains("assertion failed"));

    let annotations = client(&server)
        .list_check_run_annotations("acme", "widgets", 81)
        .await
        .expect("check annotations read succeeds");
    assert_eq!(annotations.len(), 1);
    assert_eq!(annotations[0].path, "src/lib.rs");
    assert_eq!(annotations[0].message, "assertion failed");
}

#[test]
fn extracts_actions_job_id_from_check_run_details_url() {
    assert_eq!(
        actions_job_id_from_details_url(Some(
            "https://github.com/acme/widgets/actions/runs/29679449/job/399444496",
        )),
        Some(399444496),
    );
    assert_eq!(
        actions_job_id_from_details_url(Some(
            "https://github.com/acme/widgets/actions/runs/29679449",
        )),
        None,
    );
    assert_eq!(
        actions_job_id_from_details_url(Some(
            "https://github.com/acme/widgets/actions/runs/29679449/job/0",
        )),
        None,
    );
}

#[tokio::test]
async fn rejects_oversized_actions_job_logs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/actions/jobs/399/logs"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(128 * 1024 + 1)))
        .mount(&server)
        .await;

    let error = client(&server)
        .download_workflow_job_logs("acme", "widgets", 399)
        .await
        .expect_err("oversized workflow logs must fail closed");
    assert!(matches!(error, GitHubClientError::WorkflowLogTooLarge));
}
