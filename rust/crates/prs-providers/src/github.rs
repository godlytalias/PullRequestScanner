use async_trait::async_trait;
use prs_model::{
    PullRequestDetails, PullRequestIteration, PullRequestReference, RemoteReviewComment,
    ReviewComment, ReviewCommentSide, ReviewProvider as ProviderKind,
};
use reqwest::Method;
use serde_json::{Value, json};

use crate::util::{
    bool_at, date_at, date_at_path, endpoint, id_at, set_query, string_at, string_value, usize_at,
};
use crate::{ProviderError, ReviewCommentFormatter, ReviewProvider, ReviewProviderHttp};

#[derive(Clone)]
pub struct GitHubReviewProviderClient {
    http: ReviewProviderHttp,
}

impl GitHubReviewProviderClient {
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
        let base_uri = endpoint(
            &reference.api_base_url,
            &[
                "repos",
                &reference.organization,
                &reference.repository,
                "pulls",
                &number,
                "commits",
            ],
            None,
        )?;
        let mut iterations = Vec::new();

        for page in 1..=100 {
            let request_uri = set_query(&base_uri, format!("per_page=100&page={page}"))?;
            let document = self.http.get_json(reference, &request_uri).await?;
            let commits = document.as_array().ok_or_else(|| {
                ProviderError::Message(
                    "GitHub returned an invalid pull request commits response.".into(),
                )
            })?;

            for commit in commits {
                let head_sha = string_value(commit, "sha");
                if head_sha.trim().is_empty() {
                    continue;
                }

                let number = u32::try_from(iterations.len() + 1).unwrap_or(u32::MAX);
                let parent_sha = commit
                    .get("parents")
                    .and_then(Value::as_array)
                    .and_then(|parents| parents.first())
                    .map(|parent| string_value(parent, "sha"))
                    .unwrap_or_default();
                let base_sha = if !parent_sha.trim().is_empty() {
                    parent_sha
                } else {
                    iterations
                        .last()
                        .map(|iteration: &PullRequestIteration| iteration.head_commit_sha.clone())
                        .filter(|sha| !sha.trim().is_empty())
                        .unwrap_or_else(|| fallback_base_sha.to_string())
                };
                let message = string_at(commit, &["commit", "message"]).unwrap_or_default();
                let normalized_message = message.replace('\r', "");
                let subject = normalized_message
                    .split('\n')
                    .find(|line| !line.is_empty())
                    .unwrap_or("Commit");
                let author = string_at(commit, &["author", "login"])
                    .or_else(|| string_at(commit, &["commit", "author", "name"]))
                    .unwrap_or_default();
                let abbreviated_sha = head_sha.chars().take(8).collect::<String>();

                iterations.push(PullRequestIteration {
                    id: head_sha.clone(),
                    number,
                    label: format!("Commit {number}: {subject} ({abbreviated_sha})"),
                    author: author.to_string(),
                    base_commit_sha: base_sha,
                    head_commit_sha: head_sha,
                    created_at_utc: date_at_path(commit, &["commit", "author", "date"]),
                    is_all_changes: false,
                });
            }

            if commits.len() < 100 {
                break;
            }
        }

        Ok(iterations)
    }

    async fn get_paged_comments(
        &self,
        reference: &PullRequestReference,
        base_uri: &str,
        kind: GitHubCommentKind,
    ) -> Result<Vec<RemoteReviewComment>, ProviderError> {
        let mut comments = Vec::new();
        for page in 1..=100 {
            let request_uri = set_query(base_uri, format!("per_page=100&page={page}"))?;
            let document = self.http.get_json(reference, &request_uri).await?;
            let values = document.as_array().ok_or_else(|| {
                ProviderError::Message(
                    "GitHub returned an invalid pull request comments response.".into(),
                )
            })?;

            for value in values {
                let comment = match kind {
                    GitHubCommentKind::Inline => parse_inline_comment(value),
                    GitHubCommentKind::Issue => parse_general_comment(value, "issue-comment"),
                    GitHubCommentKind::Review => parse_general_comment(value, "review"),
                };
                if !comment.body.trim().is_empty() {
                    comments.push(comment);
                }
            }

            if values.len() < 100 {
                break;
            }
        }
        Ok(comments)
    }
}

#[async_trait]
impl ReviewProvider for GitHubReviewProviderClient {
    async fn get_pull_request(
        &self,
        reference: &PullRequestReference,
    ) -> Result<PullRequestDetails, ProviderError> {
        let number = reference.number.to_string();
        let request_uri = endpoint(
            &reference.api_base_url,
            &[
                "repos",
                &reference.organization,
                &reference.repository,
                "pulls",
                &number,
            ],
            None,
        )?;
        let root = self.http.get_json(reference, &request_uri).await?;

        let head_sha = string_at(&root, &["head", "sha"]).unwrap_or_default();
        let base_sha = string_at(&root, &["base", "sha"]).unwrap_or_default();
        if head_sha.trim().is_empty() || base_sha.trim().is_empty() {
            return Err(ProviderError::Message(
                "GitHub returned a pull request without source and target commit IDs.".into(),
            ));
        }

        let state = string_value(&root, "state");
        let is_merged = bool_at(&root, "merged").unwrap_or(false);
        let source_branch = string_at(&root, &["head", "ref"]).unwrap_or_default();
        let target_branch = string_at(&root, &["base", "ref"]).unwrap_or_default();
        let clone_url =
            string_at(&root, &["base", "repo", "clone_url"]).unwrap_or(&reference.clone_url);

        Ok(PullRequestDetails {
            reference: reference.clone(),
            title: string_value(&root, "title"),
            description: string_value(&root, "body"),
            author: string_at(&root, &["user", "login"])
                .unwrap_or_default()
                .to_string(),
            state: state.clone(),
            is_closed: is_merged || state.eq_ignore_ascii_case("closed"),
            is_merged,
            source_branch: source_branch.to_string(),
            target_branch: target_branch.to_string(),
            head_commit_sha: head_sha.to_string(),
            base_commit_sha: base_sha.to_string(),
            source_fetch_ref: format!("refs/pull/{}/head", reference.number),
            target_fetch_ref: format!("refs/heads/{target_branch}"),
            clone_url: clone_url.to_string(),
            updated_at_utc: date_at(&root, "updated_at"),
        })
    }

    async fn get_iterations(
        &self,
        reference: &PullRequestReference,
    ) -> Result<Vec<PullRequestIteration>, ProviderError> {
        self.get_iterations_internal(reference, "").await
    }

    async fn get_comments(
        &self,
        pull_request: &PullRequestDetails,
    ) -> Result<Vec<RemoteReviewComment>, ProviderError> {
        let reference = &pull_request.reference;
        let number = reference.number.to_string();
        let inline_uri = endpoint(
            &reference.api_base_url,
            &[
                "repos",
                &reference.organization,
                &reference.repository,
                "pulls",
                &number,
                "comments",
            ],
            None,
        )?;
        let issue_uri = endpoint(
            &reference.api_base_url,
            &[
                "repos",
                &reference.organization,
                &reference.repository,
                "issues",
                &number,
                "comments",
            ],
            None,
        )?;
        let review_uri = endpoint(
            &reference.api_base_url,
            &[
                "repos",
                &reference.organization,
                &reference.repository,
                "pulls",
                &number,
                "reviews",
            ],
            None,
        )?;

        let (mut inline, issue, reviews) = tokio::try_join!(
            self.get_paged_comments(reference, &inline_uri, GitHubCommentKind::Inline),
            self.get_paged_comments(reference, &issue_uri, GitHubCommentKind::Issue),
            self.get_paged_comments(reference, &review_uri, GitHubCommentKind::Review)
        )?;
        inline.extend(issue);
        inline.extend(reviews);
        inline.sort_by_key(|comment| std::cmp::Reverse(comment.created_at_utc));
        Ok(inline)
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
                "repos",
                &reference.organization,
                &reference.repository,
                "pulls",
                &number,
                "comments",
            ],
            None,
        )?;
        let payload = json!({
            "body": ReviewCommentFormatter::create_published_body(
                ProviderKind::GitHub,
                comment,
            ),
            "commit_id": comment
                .commit_sha
                .as_deref()
                .unwrap_or(&pull_request.head_commit_sha),
            "path": comment.file_path.replace('\\', "/"),
            "side": if comment.side == ReviewCommentSide::Left {
                "LEFT"
            } else {
                "RIGHT"
            },
            "line": comment.line_number,
        });

        let document = self
            .http
            .send_json(reference, Method::POST, &request_uri, &payload)
            .await?;
        id_at(&document, "id").ok_or_else(|| {
            ProviderError::Message("GitHub published the comment but did not return its ID.".into())
        })
    }

    async fn delete_comment(
        &self,
        pull_request: &PullRequestDetails,
        comment: &ReviewComment,
    ) -> Result<(), ProviderError> {
        let remote_id = comment.remote_id.as_deref().unwrap_or("").trim();
        if remote_id.is_empty() {
            return Err(ProviderError::Message(
                "This comment does not have a GitHub remote ID and can be deleted only locally."
                    .into(),
            ));
        }

        let reference = &pull_request.reference;
        let request_uri = endpoint(
            &reference.api_base_url,
            &[
                "repos",
                &reference.organization,
                &reference.repository,
                "pulls",
                "comments",
                remote_id,
            ],
            None,
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

        let reference = &pull_request.reference;
        let number = reference.number.to_string();
        let (request_uri, payload) =
            if !comment.file_path.trim().is_empty() && comment.line_number.is_some() {
                let thread_id = comment.thread_id.parse::<i64>().map_err(|_| {
                    ProviderError::Message(
                        "This GitHub comment does not have a numeric reply thread ID.".into(),
                    )
                })?;
                (
                    endpoint(
                        &reference.api_base_url,
                        &[
                            "repos",
                            &reference.organization,
                            &reference.repository,
                            "pulls",
                            &number,
                            "comments",
                        ],
                        None,
                    )?,
                    json!({
                        "body": body,
                        "in_reply_to_id": thread_id,
                    }),
                )
            } else {
                (
                    endpoint(
                        &reference.api_base_url,
                        &[
                            "repos",
                            &reference.organization,
                            &reference.repository,
                            "issues",
                            &number,
                            "comments",
                        ],
                        None,
                    )?,
                    json!({ "body": body }),
                )
            };

        let document = self
            .http
            .send_json(reference, Method::POST, &request_uri, &payload)
            .await?;
        id_at(&document, "id").ok_or_else(|| {
            ProviderError::Message("GitHub posted the reply but did not return its ID.".into())
        })
    }
}

#[derive(Clone, Copy)]
enum GitHubCommentKind {
    Inline,
    Issue,
    Review,
}

fn parse_inline_comment(element: &Value) -> RemoteReviewComment {
    let remote_id = id_at(element, "id").unwrap_or_default();
    let parent_remote_id = id_at(element, "in_reply_to_id").unwrap_or_default();
    let side = string_value(element, "side");
    RemoteReviewComment {
        id: format!("inline:{remote_id}"),
        remote_id: remote_id.clone(),
        thread_id: if parent_remote_id.trim().is_empty() {
            remote_id
        } else {
            parent_remote_id.clone()
        },
        parent_remote_id: (!parent_remote_id.trim().is_empty()).then_some(parent_remote_id),
        author: string_at(element, &["user", "login"])
            .unwrap_or("GitHub")
            .to_string(),
        body: string_value(element, "body"),
        file_path: string_value(element, "path").replace('\\', "/"),
        line_number: usize_at(element, "line").or_else(|| usize_at(element, "original_line")),
        side: Some(if side.eq_ignore_ascii_case("LEFT") {
            ReviewCommentSide::Left
        } else {
            ReviewCommentSide::Right
        }),
        is_resolved: false,
        web_url: string_value(element, "html_url"),
        created_at_utc: date_at(element, "created_at"),
    }
}

fn parse_general_comment(element: &Value, prefix: &str) -> RemoteReviewComment {
    let remote_id = id_at(element, "id").unwrap_or_default();
    RemoteReviewComment {
        id: format!("{prefix}:{remote_id}"),
        remote_id: remote_id.clone(),
        thread_id: format!("{prefix}:{remote_id}"),
        parent_remote_id: None,
        author: string_at(element, &["user", "login"])
            .unwrap_or("GitHub")
            .to_string(),
        body: string_value(element, "body"),
        file_path: String::new(),
        line_number: None,
        side: None,
        is_resolved: false,
        web_url: string_value(element, "html_url"),
        created_at_utc: date_at(element, "submitted_at").or_else(|| date_at(element, "created_at")),
    }
}
