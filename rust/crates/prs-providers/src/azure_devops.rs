use async_trait::async_trait;
use prs_model::{
    PullRequestDetails, PullRequestIteration, PullRequestReference, RemoteReviewComment,
    ReviewComment, ReviewCommentSide, ReviewProvider as ProviderKind,
};
use reqwest::Method;
use serde_json::{Map, Value, json};

use crate::util::{bool_at, date_at, endpoint, id_at, string_at, string_value, u32_at, usize_at};
use crate::{ProviderError, ReviewCommentFormatter, ReviewProvider, ReviewProviderHttp};

const API_VERSION_QUERY: Option<&str> = Some("api-version=7.1");

#[derive(Clone)]
pub struct AzureDevOpsReviewProviderClient {
    http: ReviewProviderHttp,
}

impl AzureDevOpsReviewProviderClient {
    #[must_use]
    pub fn new(http: ReviewProviderHttp) -> Self {
        Self { http }
    }

    pub async fn get_iterations_for_pull_request(
        &self,
        pull_request: &PullRequestDetails,
    ) -> Result<Vec<PullRequestIteration>, ProviderError> {
        self.get_iterations_internal(&pull_request.reference, &pull_request.base_commit_sha)
            .await
    }

    async fn get_iterations_internal(
        &self,
        reference: &PullRequestReference,
        fallback_base_sha: &str,
    ) -> Result<Vec<PullRequestIteration>, ProviderError> {
        let number = reference.number.to_string();
        let request_uri = endpoint(
            &reference.api_base_url,
            &[
                "_apis",
                "git",
                "repositories",
                &reference.repository,
                "pullRequests",
                &number,
                "iterations",
            ],
            Some("includeCommits=true&api-version=7.1"),
        )?;
        let document = self.http.get_json(reference, &request_uri).await?;
        let values = document
            .get("value")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProviderError::Message(
                    "Azure DevOps returned an invalid pull request iterations response.".into(),
                )
            })?;

        let mut iterations = Vec::new();
        for element in values {
            let head_sha = string_at(element, &["sourceRefCommit", "commitId"]).unwrap_or("");
            if head_sha.trim().is_empty() {
                continue;
            }

            let number = u32_at(element, "id")
                .unwrap_or_else(|| u32::try_from(iterations.len() + 1).unwrap_or(u32::MAX));
            let base_sha = iterations
                .last()
                .map(|iteration: &PullRequestIteration| iteration.head_commit_sha.clone())
                .or_else(|| {
                    string_at(element, &["commonRefCommit", "commitId"]).map(str::to_string)
                })
                .or_else(|| {
                    string_at(element, &["targetRefCommit", "commitId"]).map(str::to_string)
                })
                .unwrap_or_else(|| fallback_base_sha.to_string());
            let description = string_value(element, "description");
            let label = if description.trim().is_empty() {
                format!("Iteration {number}")
            } else {
                format!("Iteration {number}: {description}")
            };

            iterations.push(PullRequestIteration {
                id: number.to_string(),
                number,
                label,
                author: string_at(element, &["createdBy", "displayName"])
                    .or_else(|| string_at(element, &["updatedBy", "displayName"]))
                    .unwrap_or_default()
                    .to_string(),
                base_commit_sha: base_sha,
                head_commit_sha: head_sha.to_string(),
                created_at_utc: date_at(element, "createdDate"),
                is_all_changes: false,
            });
        }

        iterations.sort_by_key(|iteration| iteration.number);
        Ok(iterations)
    }
}

#[async_trait]
impl ReviewProvider for AzureDevOpsReviewProviderClient {
    async fn get_pull_request(
        &self,
        reference: &PullRequestReference,
    ) -> Result<PullRequestDetails, ProviderError> {
        let number = reference.number.to_string();
        let request_uri = endpoint(
            &reference.api_base_url,
            &[
                "_apis",
                "git",
                "repositories",
                &reference.repository,
                "pullRequests",
                &number,
            ],
            API_VERSION_QUERY,
        )?;
        let root = self.http.get_json(reference, &request_uri).await?;

        let head_sha = string_at(&root, &["lastMergeSourceCommit", "commitId"]).unwrap_or("");
        let base_sha = string_at(&root, &["lastMergeTargetCommit", "commitId"]).unwrap_or("");
        if head_sha.trim().is_empty() || base_sha.trim().is_empty() {
            return Err(ProviderError::Message(
                "Azure DevOps returned a pull request without source and target commit IDs.".into(),
            ));
        }

        let state = string_value(&root, "status");
        let closed_date = date_at(&root, "closedDate");
        let source_branch = string_value(&root, "sourceRefName");
        let target_branch = string_value(&root, "targetRefName");
        let clone_url =
            string_at(&root, &["repository", "remoteUrl"]).unwrap_or(&reference.clone_url);

        Ok(PullRequestDetails {
            reference: reference.clone(),
            title: string_value(&root, "title"),
            description: string_value(&root, "description"),
            author: string_at(&root, &["createdBy", "displayName"])
                .unwrap_or_default()
                .to_string(),
            state: state.clone(),
            is_closed: state.eq_ignore_ascii_case("completed")
                || state.eq_ignore_ascii_case("abandoned"),
            is_merged: state.eq_ignore_ascii_case("completed"),
            source_branch: source_branch.clone(),
            target_branch: target_branch.clone(),
            head_commit_sha: head_sha.to_string(),
            base_commit_sha: base_sha.to_string(),
            source_fetch_ref: source_branch,
            target_fetch_ref: target_branch,
            clone_url: clone_url.to_string(),
            updated_at_utc: closed_date.or_else(|| date_at(&root, "creationDate")),
        })
    }

    async fn get_iterations(
        &self,
        reference: &PullRequestReference,
    ) -> Result<Vec<PullRequestIteration>, ProviderError> {
        self.get_iterations_internal(reference, "").await
    }

    async fn publish_comment(
        &self,
        pull_request: &PullRequestDetails,
        comment: &ReviewComment,
    ) -> Result<String, ProviderError> {
        let reference = &pull_request.reference;
        let number = reference.number.to_string();
        let request_uri = endpoint(
            &reference.api_base_url,
            &[
                "_apis",
                "git",
                "repositories",
                &reference.repository,
                "pullRequests",
                &number,
                "threads",
            ],
            API_VERSION_QUERY,
        )?;
        let side = if comment.side == ReviewCommentSide::Left {
            "leftFile"
        } else {
            "rightFile"
        };
        let position = json!({ "line": comment.line_number, "offset": 1 });
        let mut context = Map::new();
        context.insert(
            "filePath".to_string(),
            Value::String(format!(
                "/{}",
                comment.file_path.replace('\\', "/").trim_start_matches('/')
            )),
        );
        context.insert(format!("{side}Start"), position.clone());
        context.insert(format!("{side}End"), position);
        let payload = json!({
            "comments": [{
                "parentCommentId": 0,
                "content": ReviewCommentFormatter::create_published_body(
                    ProviderKind::AzureDevOps,
                    comment,
                ),
                "commentType": 1
            }],
            "status": 1,
            "threadContext": context
        });

        let document = self
            .http
            .send_json(reference, Method::POST, &request_uri, &payload)
            .await?;
        let thread_id = id_at(&document, "id").ok_or_else(|| {
            ProviderError::Message(
                "Azure DevOps created the thread but did not return its ID.".into(),
            )
        })?;
        let comment_id = document
            .get("comments")
            .and_then(Value::as_array)
            .and_then(|comments| comments.first())
            .and_then(|comment| id_at(comment, "id"))
            .unwrap_or_else(|| "0".to_string());
        Ok(format!("{thread_id}:{comment_id}"))
    }

    async fn get_comments(
        &self,
        pull_request: &PullRequestDetails,
    ) -> Result<Vec<RemoteReviewComment>, ProviderError> {
        let reference = &pull_request.reference;
        let number = reference.number.to_string();
        let request_uri = endpoint(
            &reference.api_base_url,
            &[
                "_apis",
                "git",
                "repositories",
                &reference.repository,
                "pullRequests",
                &number,
                "threads",
            ],
            Some("$top=1000&api-version=7.1"),
        )?;
        let document = self.http.get_json(reference, &request_uri).await?;
        let threads = document
            .get("value")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProviderError::Message(
                    "Azure DevOps returned an invalid pull request threads response.".into(),
                )
            })?;

        let mut remote_comments = Vec::new();
        for thread in threads {
            let thread_id = id_at(thread, "id").unwrap_or_default();
            let thread_status = string_value(thread, "status");
            let is_resolved = !thread_status.eq_ignore_ascii_case("active")
                && !thread_status.eq_ignore_ascii_case("pending")
                && !thread_status.eq_ignore_ascii_case("unknown");
            let (file_path, line_number, side) = parse_thread_context(thread);
            let comments = thread
                .get("comments")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for comment in comments {
                let body = string_value(&comment, "content");
                let deleted = bool_at(&comment, "isDeleted").unwrap_or(false);
                if body.trim().is_empty() || deleted || !is_text_comment(&comment) {
                    continue;
                }

                let comment_id = id_at(&comment, "id").unwrap_or_default();
                remote_comments.push(RemoteReviewComment {
                    id: format!("thread:{thread_id}:comment:{comment_id}"),
                    remote_id: format!("{thread_id}:{comment_id}"),
                    thread_id: thread_id.clone(),
                    parent_remote_id: format_parent_remote_id(
                        &thread_id,
                        id_at(&comment, "parentCommentId").as_deref(),
                    ),
                    author: string_at(&comment, &["author", "displayName"])
                        .unwrap_or("Azure DevOps")
                        .to_string(),
                    body,
                    file_path: file_path.clone(),
                    line_number,
                    side,
                    is_resolved,
                    web_url: reference.web_url.clone(),
                    created_at_utc: date_at(&comment, "publishedDate"),
                });
            }
        }

        remote_comments.sort_by_key(|comment| std::cmp::Reverse(comment.created_at_utc));
        Ok(remote_comments)
    }

    async fn delete_comment(
        &self,
        pull_request: &PullRequestDetails,
        comment: &ReviewComment,
    ) -> Result<(), ProviderError> {
        let id_parts: Vec<_> = comment
            .remote_id
            .as_deref()
            .unwrap_or("")
            .split(':')
            .filter(|part| !part.is_empty())
            .collect();
        if id_parts.len() != 2 {
            return Err(ProviderError::Message(
                "This Azure DevOps comment does not have both thread and comment IDs.".into(),
            ));
        }

        let reference = &pull_request.reference;
        let number = reference.number.to_string();
        let request_uri = endpoint(
            &reference.api_base_url,
            &[
                "_apis",
                "git",
                "repositories",
                &reference.repository,
                "pullRequests",
                &number,
                "threads",
                id_parts[0],
                "comments",
                id_parts[1],
            ],
            API_VERSION_QUERY,
        )?;
        self.http.delete(reference, &request_uri).await
    }

    async fn reply_to_comment(
        &self,
        pull_request: &PullRequestDetails,
        comment: &RemoteReviewComment,
        body: &str,
    ) -> Result<String, ProviderError> {
        if body.trim().is_empty() {
            return Err(ProviderError::Message("Reply text is required.".into()));
        }
        if comment.thread_id.trim().is_empty() {
            return Err(ProviderError::Message(
                "This Azure DevOps comment is not associated with a replyable thread.".into(),
            ));
        }

        let reference = &pull_request.reference;
        let number = reference.number.to_string();
        let request_uri = endpoint(
            &reference.api_base_url,
            &[
                "_apis",
                "git",
                "repositories",
                &reference.repository,
                "pullRequests",
                &number,
                "threads",
                &comment.thread_id,
                "comments",
            ],
            API_VERSION_QUERY,
        )?;
        let payload = json!({
            "parentCommentId": parse_comment_id(&comment.remote_id),
            "content": body,
            "commentType": 1
        });
        let document = self
            .http
            .send_json(reference, Method::POST, &request_uri, &payload)
            .await?;
        id_at(&document, "id").ok_or_else(|| {
            ProviderError::Message(
                "Azure DevOps posted the reply but did not return its ID.".into(),
            )
        })
    }
}

fn parse_comment_id(remote_id: &str) -> i32 {
    remote_id
        .rsplit(':')
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn format_parent_remote_id(thread_id: &str, parent_id: Option<&str>) -> Option<String> {
    parent_id
        .filter(|parent_id| !parent_id.trim().is_empty() && *parent_id != "0")
        .map(|parent_id| format!("{thread_id}:{parent_id}"))
}

fn parse_thread_context(thread: &Value) -> (String, Option<usize>, Option<ReviewCommentSide>) {
    let Some(context) = thread
        .get("threadContext")
        .filter(|context| !context.is_null())
    else {
        return (String::new(), None, None);
    };

    let path = string_value(context, "filePath")
        .trim_start_matches('/')
        .to_string();
    let right_line = nested_usize(context, "rightFileStart", "line");
    if right_line.is_some() {
        return (path, right_line, Some(ReviewCommentSide::Right));
    }
    let left_line = nested_usize(context, "leftFileStart", "line");
    (path, left_line, left_line.map(|_| ReviewCommentSide::Left))
}

fn nested_usize(value: &Value, object: &str, property: &str) -> Option<usize> {
    value
        .get(object)
        .filter(|nested| !nested.is_null())
        .and_then(|nested| usize_at(nested, property))
}

fn is_text_comment(comment: &Value) -> bool {
    let Some(comment_type) = comment.get("commentType") else {
        return true;
    };
    match comment_type {
        Value::String(value) => value.eq_ignore_ascii_case("text"),
        Value::Number(value) => value.as_i64() == Some(1),
        _ => false,
    }
}
