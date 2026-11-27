use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use async_trait::async_trait;
use prs_core::{CancellationToken, CoreError};
use prs_model::{
    PullRequestDetails, PullRequestReference, RemoteReviewComment, ReviewComment,
    ReviewCommentSide, ReviewProvider as ProviderKind,
};
use prs_providers::{
    AzureDevOpsReviewProviderClient, CredentialProvider, GitHubReviewProviderClient, ProviderError,
    ReviewCommentFormatter, ReviewCredential, ReviewProvider, ReviewProviderHttp,
};
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread")]
async fn converts_unauthorized_provider_response_into_authentication_request() {
    let server = MockServer::start(|_| MockResponse::json_status(401, r#"{"message":"no"}"#));
    let reference = github_pull_request(&server).reference;
    let client = github_client(&server, NoCredentialProvider);

    let error = client.get_pull_request(&reference).await.unwrap_err();

    match error {
        ProviderError::Authentication {
            reference: rejected_reference,
            message,
        } => {
            assert_eq!(*rejected_reference, reference);
            assert!(message.contains("gh auth login"));
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn publishes_with_api_credential_and_standard_headers() {
    let server = MockServer::start(|_| MockResponse::json(200, r#"{"id":99}"#));
    let client = github_client(
        &server,
        FixedCredentialProvider(ReviewCredential::new("Bearer", "generic-test-token")),
    );
    let pull_request = github_pull_request(&server);
    let comment = ReviewComment {
        file_path: "src/Foo.cs".to_string(),
        line_number: 8,
        side: ReviewCommentSide::Right,
        body: "Authenticated comment.".to_string(),
        ..ReviewComment::default()
    };

    let remote_id = client
        .publish_comment(&pull_request, &comment)
        .await
        .unwrap();

    assert_eq!(remote_id, "99");
    let request = server.single_request();
    assert_eq!(
        request.header("authorization"),
        Some("Bearer generic-test-token")
    );
    assert_eq!(request.header("accept"), Some("application/json"));
    assert_eq!(
        request.header("user-agent"),
        Some("PullRequest-Scanner/1.0")
    );
    assert_eq!(request.header("x-github-api-version"), Some("2022-11-28"));
    let body: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["body"], "Authenticated comment.");
    assert_eq!(body["commit_id"], "generic-head-sha");
    assert_eq!(body["path"], "src/Foo.cs");
    assert_eq!(body["side"], "RIGHT");
    assert_eq!(body["line"], 8);
}

#[tokio::test(flavor = "multi_thread")]
async fn publishes_azure_devops_thread_with_diff_suggestion() {
    let server =
        MockServer::start(|_| MockResponse::json(200, r#"{"id":31,"comments":[{"id":4}]}"#));
    let client = azure_devops_client(&server, NoCredentialProvider);
    let comment = ReviewComment {
        file_path: r"src\Foo.cpp".to_string(),
        line_number: 42,
        side: ReviewCommentSide::Right,
        body: "Use the bounded value.".to_string(),
        source_line_text: "auto value = input;".to_string(),
        suggested_replacement: "auto bounded = input;".to_string(),
        ..ReviewComment::default()
    };

    let remote_id = client
        .publish_comment(&azure_devops_pull_request(&server), &comment)
        .await
        .unwrap();

    assert_eq!(remote_id, "31:4");
    let request = server.single_request();
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.path,
        "/org/project/_apis/git/repositories/repo/pullRequests/7/threads?api-version=7.1"
    );
    let body: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["threadContext"]["filePath"], "/src/Foo.cpp");
    assert_eq!(body["threadContext"]["rightFileStart"]["line"], 42);
    assert_eq!(
        body["comments"][0]["content"],
        "Use the bounded value.\n\nSuggested replacement:\n```diff\n- auto value = input;\n+ auto bounded = input;\n```"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn deletes_github_published_comment_with_remote_id() {
    let server = MockServer::start(|_| MockResponse::empty(204));
    let client = github_client(&server, NoCredentialProvider);

    client
        .delete_comment(
            &github_pull_request(&server),
            &ReviewComment {
                remote_id: Some("99".to_string()),
                ..ReviewComment::default()
            },
        )
        .await
        .unwrap();

    let request = server.single_request();
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path, "/api/v3/repos/owner/repo/pulls/comments/99");
}

#[tokio::test(flavor = "multi_thread")]
async fn deletes_azure_devops_comment_by_thread_and_comment_id() {
    let server = MockServer::start(|_| MockResponse::empty(200));
    let client = azure_devops_client(&server, NoCredentialProvider);

    client
        .delete_comment(
            &azure_devops_pull_request(&server),
            &ReviewComment {
                remote_id: Some("31:4".to_string()),
                ..ReviewComment::default()
            },
        )
        .await
        .unwrap();

    let request = server.single_request();
    assert_eq!(request.method, "DELETE");
    assert_eq!(
        request.path,
        "/org/project/_apis/git/repositories/repo/pullRequests/7/threads/31/comments/4?api-version=7.1"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn parses_github_commit_iterations() {
    let server = MockServer::start(|_| {
        MockResponse::json(
            200,
            r#"[
              {
                "sha": "1111111111111111111111111111111111111111",
                "parents": [{ "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }],
                "author": { "login": "first" },
                "commit": {
                  "message": "First update\n\nDetails",
                  "author": { "name": "First", "date": "2026-11-20T10:00:00Z" }
                }
              },
              {
                "sha": "2222222222222222222222222222222222222222",
                "parents": [{ "sha": "1111111111111111111111111111111111111111" }],
                "author": { "login": "second" },
                "commit": {
                  "message": "Second update",
                  "author": { "name": "Second", "date": "2026-11-21T10:00:00Z" }
                }
              }
            ]"#,
        )
    });
    let client = github_client(&server, NoCredentialProvider);

    let iterations = client
        .get_iterations(&github_pull_request(&server).reference)
        .await
        .unwrap();

    assert_eq!(iterations.len(), 2);
    assert_eq!(
        iterations[0].base_commit_sha,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(
        iterations[1].base_commit_sha,
        "1111111111111111111111111111111111111111"
    );
    assert_eq!(
        iterations[1].head_commit_sha,
        "2222222222222222222222222222222222222222"
    );
    assert!(iterations[1].label.contains("Commit 2: Second update"));
}

#[tokio::test(flavor = "multi_thread")]
async fn parses_azure_devops_iteration_deltas() {
    let server = MockServer::start(|_| {
        MockResponse::json(
            200,
            r#"{
              "value": [
                {
                  "id": 1,
                  "description": "Initial update",
                  "createdDate": "2026-11-20T10:00:00Z",
                  "commonRefCommit": { "commitId": "base-1" },
                  "sourceRefCommit": { "commitId": "head-1" },
                  "createdBy": { "displayName": "First" }
                },
                {
                  "id": 2,
                  "description": "Address feedback",
                  "createdDate": "2026-11-21T10:00:00Z",
                  "targetRefCommit": { "commitId": "target-2" },
                  "sourceRefCommit": { "commitId": "head-2" },
                  "createdBy": { "displayName": "Second" }
                }
              ]
            }"#,
        )
    });
    let client = azure_devops_client(&server, NoCredentialProvider);

    let iterations = client
        .get_iterations_for_pull_request(&azure_devops_pull_request(&server))
        .await
        .unwrap();

    assert_eq!(iterations.len(), 2);
    assert_eq!(iterations[0].base_commit_sha, "base-1");
    assert_eq!(iterations[1].base_commit_sha, "head-1");
    assert_eq!(iterations[1].head_commit_sha, "head-2");
    assert!(iterations[1].label.contains("Address feedback"));
}

#[tokio::test(flavor = "multi_thread")]
async fn parses_provider_pull_request_metadata() {
    let github = MockServer::start(|_| {
        MockResponse::json(
            200,
            r###"{
              "title": "Merged PR",
              "body": "## Summary\n\nBuild-only cleanup.",
              "state": "closed",
              "merged": true,
              "updated_at": "2026-11-21T10:00:00Z",
              "user": { "login": "author" },
              "head": { "sha": "head", "ref": "feature" },
              "base": {
                "sha": "base",
                "ref": "main",
                "repo": { "clone_url": "https://example.test/repo.git" }
              }
            }"###,
        )
    });
    let github_client = github_client(&github, NoCredentialProvider);
    let github_details = github_client
        .get_pull_request(&github_pull_request(&github).reference)
        .await
        .unwrap();
    assert!(github_details.is_closed);
    assert!(github_details.is_merged);
    assert_eq!(
        github_details.description,
        "## Summary\n\nBuild-only cleanup."
    );

    let azure_devops = MockServer::start(|_| {
        MockResponse::json(
            200,
            r###"{
              "title": "Build cleanup",
              "description": "## Summary\n\nADO description.",
              "status": "completed",
              "sourceRefName": "refs/heads/feature",
              "targetRefName": "refs/heads/main",
              "createdBy": { "displayName": "Author" },
              "lastMergeSourceCommit": { "commitId": "head" },
              "lastMergeTargetCommit": { "commitId": "base" },
              "repository": { "remoteUrl": "https://example.test/repo" },
              "closedDate": "2026-11-21T10:00:00Z"
            }"###,
        )
    });
    let azure_client = azure_devops_client(&azure_devops, NoCredentialProvider);
    let azure_details = azure_client
        .get_pull_request(&azure_devops_pull_request(&azure_devops).reference)
        .await
        .unwrap();
    assert_eq!(azure_details.description, "## Summary\n\nADO description.");
    assert_eq!(azure_details.title, "Build cleanup");
    assert!(azure_details.is_closed);
    assert!(azure_details.is_merged);
}

#[tokio::test(flavor = "multi_thread")]
async fn parses_github_inline_conversation_and_review_comments() {
    let server = MockServer::start(|request| {
        if request
            .path
            .starts_with("/api/v3/repos/owner/repo/pulls/7/comments")
        {
            MockResponse::json(
                200,
                r#"[
                  {
                    "id": 11,
                    "body": "Check this condition.",
                    "path": "src/Foo.cs",
                    "line": 18,
                    "side": "RIGHT",
                    "user": { "login": "reviewer" },
                    "created_at": "2026-11-21T10:00:00Z",
                    "html_url": "https://github.example/comment/11"
                  }
                ]"#,
            )
        } else if request
            .path
            .starts_with("/api/v3/repos/owner/repo/issues/7/comments")
        {
            MockResponse::json(
                200,
                r#"[
                  {
                    "id": 12,
                    "body": "General note.",
                    "user": { "login": "owner" },
                    "created_at": "2026-11-21T10:01:00Z",
                    "html_url": "https://github.example/comment/12"
                  }
                ]"#,
            )
        } else {
            MockResponse::json(
                200,
                r#"[
                  {
                    "id": 13,
                    "body": "Review summary.",
                    "user": { "login": "reviewer" },
                    "submitted_at": "2026-11-21T10:02:00Z",
                    "html_url": "https://github.example/review/13"
                  }
                ]"#,
            )
        }
    });
    let client = github_client(&server, NoCredentialProvider);

    let comments = client
        .get_comments(&github_pull_request(&server))
        .await
        .unwrap();

    assert_eq!(comments.len(), 3);
    let inline = comments
        .iter()
        .find(|comment| comment.file_path == "src/Foo.cs")
        .unwrap();
    assert_eq!(inline.line_number, Some(18));
    assert_eq!(inline.side, Some(ReviewCommentSide::Right));
    assert_eq!(inline.author, "reviewer");
}

#[tokio::test(flavor = "multi_thread")]
async fn parses_azure_devops_preview_and_stable_comment_shapes() {
    let server = MockServer::start(|_| {
        MockResponse::json(
            200,
            r#"{
              "value": [
                {
                  "id": 31,
                  "status": "active",
                  "threadContext": {
                    "filePath": "/src/Foo.cpp",
                    "rightFileStart": { "line": 42, "offset": 1 }
                  },
                  "comments": [
                    {
                      "id": 1,
                      "commentType": "text",
                      "content": "Add a lifetime check.",
                      "author": { "displayName": "Reviewer One" },
                      "publishedDate": "2026-11-21T10:00:00Z",
                      "isDeleted": false
                    }
                  ]
                },
                {
                  "id": 32,
                  "status": "fixed",
                  "comments": [
                    {
                      "id": 1,
                      "commentType": 1,
                      "content": "General feedback.",
                      "author": { "displayName": "Reviewer Two" },
                      "publishedDate": "2026-11-21T10:01:00Z",
                      "isDeleted": false
                    }
                  ]
                }
              ]
            }"#,
        )
    });
    let client = azure_devops_client(&server, NoCredentialProvider);

    let comments = client
        .get_comments(&azure_devops_pull_request(&server))
        .await
        .unwrap();

    assert_eq!(comments.len(), 2);
    let inline = comments
        .iter()
        .find(|comment| comment.file_path == "src/Foo.cpp")
        .unwrap();
    assert_eq!(inline.line_number, Some(42));
    assert!(!inline.is_resolved);
    assert_eq!(
        comments
            .iter()
            .filter(|comment| comment.is_resolved)
            .count(),
        1
    );
    let request = server.single_request();
    assert!(request.path.ends_with("$top=1000&api-version=7.1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn parses_github_replies_into_parent_threads() {
    let server = MockServer::start(|request| {
        if request
            .path
            .starts_with("/api/v3/repos/owner/repo/pulls/7/comments")
        {
            MockResponse::json(
                200,
                r#"[
                  {
                    "id": 21,
                    "body": "Root",
                    "path": "src/Foo.cs",
                    "line": 8,
                    "side": "RIGHT",
                    "user": { "login": "reviewer" },
                    "created_at": "2026-11-21T10:00:00Z"
                  },
                  {
                    "id": 22,
                    "in_reply_to_id": 21,
                    "body": "Reply",
                    "path": "src/Foo.cs",
                    "line": 8,
                    "side": "RIGHT",
                    "user": { "login": "author" },
                    "created_at": "2026-11-21T10:01:00Z"
                  }
                ]"#,
            )
        } else {
            MockResponse::json(200, "[]")
        }
    });
    let client = github_client(&server, NoCredentialProvider);

    let comments = client
        .get_comments(&github_pull_request(&server))
        .await
        .unwrap();
    let reply = comments
        .iter()
        .find(|comment| comment.remote_id == "22")
        .unwrap();

    assert_eq!(reply.thread_id, "21");
    assert_eq!(reply.parent_remote_id.as_deref(), Some("21"));
}

#[tokio::test(flavor = "multi_thread")]
async fn posts_inline_replies_to_provider_threads() {
    let github = MockServer::start(|_| MockResponse::json(200, r#"{"id":23}"#));
    let github_client = github_client(&github, NoCredentialProvider);
    let github_id = github_client
        .reply_to_comment(
            &github_pull_request(&github),
            &RemoteReviewComment {
                remote_id: "22".to_string(),
                thread_id: "21".to_string(),
                file_path: "src/Foo.cs".to_string(),
                line_number: Some(8),
                ..RemoteReviewComment::default()
            },
            "Follow-up",
        )
        .await
        .unwrap();
    assert_eq!(github_id, "23");
    let github_request = github.single_request();
    assert_eq!(github_request.method, "POST");
    let github_body: Value = serde_json::from_str(&github_request.body).unwrap();
    assert_eq!(github_body["body"], "Follow-up");
    assert_eq!(github_body["in_reply_to_id"], 21);

    let azure = MockServer::start(|_| MockResponse::json(200, r#"{"id":5}"#));
    let azure_client = azure_devops_client(&azure, NoCredentialProvider);
    let azure_id = azure_client
        .reply_to_comment(
            &azure_devops_pull_request(&azure),
            &RemoteReviewComment {
                remote_id: "31:4".to_string(),
                thread_id: "31".to_string(),
                file_path: "src/Foo.cpp".to_string(),
                line_number: Some(42),
                ..RemoteReviewComment::default()
            },
            "Follow-up",
        )
        .await
        .unwrap();
    assert_eq!(azure_id, "5");
    let azure_request = azure.single_request();
    assert!(azure_request.path.contains("/threads/31/comments"));
    let azure_body: Value = serde_json::from_str(&azure_request.body).unwrap();
    assert_eq!(azure_body["parentCommentId"], 4);
    assert_eq!(azure_body["content"], "Follow-up");
}

#[tokio::test(flavor = "multi_thread")]
async fn paginates_github_comments() {
    let server = MockServer::start(|request| {
        if request
            .path
            .starts_with("/api/v3/repos/owner/repo/pulls/7/comments")
        {
            let page_two = request.path.contains("page=2");
            let count = if page_two { 1 } else { 100 };
            let start = if page_two { 100 } else { 0 };
            let comments: Vec<_> = (start..start + count)
                .map(|index| {
                    json!({
                        "id": index + 1,
                        "body": format!("Comment {index}"),
                        "path": "src/Foo.cs",
                        "line": 8,
                        "side": "RIGHT",
                        "created_at": "2026-11-21T10:00:00Z"
                    })
                })
                .collect();
            MockResponse::json_value(200, Value::Array(comments))
        } else {
            MockResponse::json(200, "[]")
        }
    });
    let client = github_client(&server, NoCredentialProvider);

    let comments = client
        .get_comments(&github_pull_request(&server))
        .await
        .unwrap();

    assert_eq!(comments.len(), 101);
    assert!(server.requests().iter().any(|request| {
        request
            .path
            .contains("/pulls/7/comments?per_page=100&page=2")
    }));
}

#[tokio::test(flavor = "multi_thread")]
async fn formats_native_and_explicit_diff_suggestions() {
    let comment = ReviewComment {
        body: "Use the bounded value.".to_string(),
        source_line_text: "    return value + 1;".to_string(),
        selection_start: Some(11),
        selection_length: 5,
        selected_text: "value".to_string(),
        side: ReviewCommentSide::Right,
        suggested_replacement: "bounded".to_string(),
        ..ReviewComment::default()
    };

    let github = ReviewCommentFormatter::create_published_body(ProviderKind::GitHub, &comment);
    assert!(github.contains("Selected code:\n> value"));
    assert!(github.contains("```suggestion\n    return bounded + 1;\n```"));

    let azure = ReviewCommentFormatter::create_published_body(ProviderKind::AzureDevOps, &comment);
    assert!(azure.contains(
        "Suggested replacement:\n```diff\n-     return value + 1;\n+     return bounded + 1;\n```"
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn percent_encodes_repository_path_segments() {
    let server = MockServer::start(|_| {
        MockResponse::json(
            200,
            r#"{
              "title": "Encoded",
              "body": "",
              "state": "open",
              "merged": false,
              "head": { "sha": "head", "ref": "feature" },
              "base": { "sha": "base", "ref": "main" }
            }"#,
        )
    });
    let client = github_client(&server, NoCredentialProvider);
    let mut details = github_pull_request(&server);
    details.reference.organization = "owner:name".to_string();
    details.reference.repository = "repo/child@one".to_string();

    client.get_pull_request(&details.reference).await.unwrap();

    assert_eq!(
        server.single_request().path,
        "/api/v3/repos/owner%3Aname/repo%2Fchild%40one/pulls/7"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_malformed_provider_payload_without_panicking() {
    let server = MockServer::start(|_| MockResponse::json(200, r#"{"head":null}"#));
    let client = github_client(&server, NoCredentialProvider);

    let error = client
        .get_pull_request(&github_pull_request(&server).reference)
        .await
        .unwrap_err();

    assert!(matches!(error, ProviderError::Message(_)));
}

#[test]
fn credential_debug_output_redacts_the_parameter() {
    let credential = ReviewCredential::new("Bearer", "generic-sensitive-value");

    let rendered = format!("{credential:?}");

    assert!(rendered.contains("Bearer"));
    assert!(rendered.to_ascii_lowercase().contains("redacted"));
    assert!(!rendered.contains("generic-sensitive-value"));
}

fn github_client<C>(_server: &MockServer, credentials: C) -> GitHubReviewProviderClient
where
    C: CredentialProvider + 'static,
{
    GitHubReviewProviderClient::new(ReviewProviderHttp::new(reqwest::Client::new(), credentials))
}

fn azure_devops_client<C>(server: &MockServer, credentials: C) -> AzureDevOpsReviewProviderClient
where
    C: CredentialProvider + 'static,
{
    let _ = server;
    AzureDevOpsReviewProviderClient::new(ReviewProviderHttp::new(
        reqwest::Client::new(),
        credentials,
    ))
}

fn github_pull_request(server: &MockServer) -> PullRequestDetails {
    PullRequestDetails {
        reference: PullRequestReference {
            provider: ProviderKind::GitHub,
            host: "github.example".to_string(),
            organization: "owner".to_string(),
            project: "owner".to_string(),
            repository: "repo".to_string(),
            number: 7,
            web_url: "https://github.example/owner/repo/pull/7".to_string(),
            api_base_url: format!("{}/api/v3", server.url()),
            clone_url: "https://github.example/owner/repo.git".to_string(),
            session_key: "github:github.example/owner/repo#7".to_string(),
        },
        head_commit_sha: "generic-head-sha".to_string(),
        base_commit_sha: "generic-base-sha".to_string(),
        ..PullRequestDetails::default()
    }
}

fn azure_devops_pull_request(server: &MockServer) -> PullRequestDetails {
    PullRequestDetails {
        reference: PullRequestReference {
            provider: ProviderKind::AzureDevOps,
            host: "dev.azure.com".to_string(),
            organization: "org".to_string(),
            project: "project".to_string(),
            repository: "repo".to_string(),
            number: 7,
            web_url: "https://dev.azure.com/org/project/_git/repo/pullrequest/7".to_string(),
            api_base_url: format!("{}/org/project", server.url()),
            clone_url: "https://dev.azure.com/org/project/_git/repo".to_string(),
            session_key: "ado:org/project/repo#7".to_string(),
        },
        head_commit_sha: "generic-head-sha".to_string(),
        base_commit_sha: "generic-base-sha".to_string(),
        ..PullRequestDetails::default()
    }
}

#[derive(Clone)]
struct NoCredentialProvider;

#[async_trait]
impl CredentialProvider for NoCredentialProvider {
    async fn get_credential(
        &self,
        _reference: &PullRequestReference,
        _cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<ReviewCredential>, CoreError> {
        Ok(None)
    }
}

struct FixedCredentialProvider(ReviewCredential);

#[async_trait]
impl CredentialProvider for FixedCredentialProvider {
    async fn get_credential(
        &self,
        _reference: &PullRequestReference,
        _cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<ReviewCredential>, CoreError> {
        Ok(Some(self.0.clone()))
    }
}

#[derive(Clone, Debug)]
struct MockRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

impl MockRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

struct MockResponse {
    status: u16,
    body: String,
}

impl MockResponse {
    fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_string(),
        }
    }

    fn json_status(status: u16, body: &str) -> Self {
        Self::json(status, body)
    }

    fn json_value(status: u16, body: Value) -> Self {
        Self {
            status,
            body: body.to_string(),
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            body: String::new(),
        }
    }
}

struct MockServer {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<MockRequest>>>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl MockServer {
    fn start<F>(handler: F) -> Self
    where
        F: Fn(&MockRequest) -> MockResponse + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_stopped = Arc::clone(&stopped);
        let thread_requests = Arc::clone(&requests);
        let handler = Arc::new(handler);
        let worker = thread::spawn(move || {
            while !thread_stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if thread_stopped.load(Ordering::Acquire) {
                            break;
                        }
                        let requests = Arc::clone(&thread_requests);
                        let handler = Arc::clone(&handler);
                        thread::spawn(move || {
                            if let Ok(request) = read_request(&mut stream) {
                                requests.lock().unwrap().push(request.clone());
                                let response = handler(&request);
                                write_response(&mut stream, response);
                            }
                        });
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            address,
            requests,
            stopped,
            worker: Some(worker),
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn requests(&self) -> Vec<MockRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn single_request(&self) -> MockRequest {
        let requests = self.requests();
        assert_eq!(requests.len(), 1);
        requests[0].clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<MockRequest> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut buffer = [0_u8; 4096];
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Err(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                "request ended before headers",
            ));
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = find_bytes(&bytes, b"\r\n\r\n") {
            break index + 4;
        }
    };

    let header_text = String::from_utf8_lossy(&bytes[..header_end]);
    let mut lines = header_text.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_string();
    let path = request_line.next().unwrap_or_default().to_string();
    let mut headers = HashMap::new();
    for line in lines.take_while(|line| !line.is_empty()) {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let mut buffer = [0_u8; 4096];
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let body = String::from_utf8_lossy(
        &bytes[header_end..std::cmp::min(bytes.len(), header_end + content_length)],
    )
    .to_string();
    Ok(MockRequest {
        method,
        path,
        headers,
        body,
    })
}

fn write_response(stream: &mut TcpStream, response: MockResponse) {
    let reason = match response.status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Response",
    };
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        reason,
        response.body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(response.body.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
