#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

mod fold;
pub use fold::CodeFoldRegion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReviewProvider {
    #[default]
    GitHub,
    AzureDevOps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ChangedFileStatus {
    Added,
    #[default]
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DiffLineKind {
    #[default]
    Context,
    Added,
    Deleted,
    Hunk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReviewCommentSide {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReviewCommentStatus {
    #[default]
    Pending,
    Published,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
pub struct PullRequestReference {
    pub provider: ReviewProvider,
    pub host: String,
    pub organization: String,
    pub project: String,
    pub repository: String,
    pub number: u32,
    pub web_url: String,
    pub api_base_url: String,
    pub clone_url: String,
    pub session_key: String,
}

impl PullRequestReference {
    #[must_use]
    pub fn web_uri(&self) -> Option<Url> {
        Url::parse(&self.web_url).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
pub struct PullRequestDetails {
    pub reference: PullRequestReference,
    pub title: String,
    pub description: String,
    pub author: String,
    pub state: String,
    pub is_closed: bool,
    pub is_merged: bool,
    pub source_branch: String,
    pub target_branch: String,
    pub head_commit_sha: String,
    pub base_commit_sha: String,
    pub source_fetch_ref: String,
    pub target_fetch_ref: String,
    pub clone_url: String,
    pub updated_at_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
pub struct PullRequestIteration {
    pub id: String,
    pub number: u32,
    pub label: String,
    pub author: String,
    pub base_commit_sha: String,
    pub head_commit_sha: String,
    pub created_at_utc: Option<DateTime<Utc>>,
    pub is_all_changes: bool,
}

impl PullRequestIteration {
    pub const ALL_CHANGES_ID: &'static str = "all-changes";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
pub struct ChangedFile {
    pub path: String,
    pub previous_path: Option<String>,
    pub status: ChangedFileStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub file_path: String,
    pub old_line_number: Option<usize>,
    pub new_line_number: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
pub struct DiffBlock {
    pub file_path: String,
    pub previous_path: Option<String>,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
pub struct DiffLinePair {
    pub left_line: Option<DiffLine>,
    pub right_line: Option<DiffLine>,
    pub hunk_line: Option<DiffLine>,
}

impl DiffLinePair {
    #[must_use]
    pub fn is_hunk(&self) -> bool {
        self.hunk_line.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ReviewComment {
    pub id: String,
    pub file_path: String,
    pub line_number: usize,
    pub side: ReviewCommentSide,
    pub body: String,
    pub source_line_text: String,
    pub selection_start: Option<usize>,
    pub selection_length: usize,
    pub selected_text: String,
    pub suggested_replacement: String,
    pub iteration_id: Option<String>,
    pub commit_sha: Option<String>,
    pub status: ReviewCommentStatus,
    pub remote_id: Option<String>,
    pub error: Option<String>,
    pub created_at_utc: DateTime<Utc>,
    pub published_at_utc: Option<DateTime<Utc>>,
}

impl Default for ReviewComment {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            file_path: String::new(),
            line_number: 0,
            side: ReviewCommentSide::Right,
            body: String::new(),
            source_line_text: String::new(),
            selection_start: None,
            selection_length: 0,
            selected_text: String::new(),
            suggested_replacement: String::new(),
            iteration_id: None,
            commit_sha: None,
            status: ReviewCommentStatus::Pending,
            remote_id: None,
            error: None,
            created_at_utc: Utc::now(),
            published_at_utc: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
pub struct RemoteReviewComment {
    pub id: String,
    pub remote_id: String,
    pub thread_id: String,
    pub parent_remote_id: Option<String>,
    pub author: String,
    pub body: String,
    pub file_path: String,
    pub line_number: Option<usize>,
    pub side: Option<ReviewCommentSide>,
    pub is_resolved: bool,
    pub web_url: String,
    pub created_at_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ReviewSessionState {
    pub schema_version: i32,
    pub details: PullRequestDetails,
    pub local_repository_path: Option<String>,
    pub reused_existing_repository: bool,
    pub notes: String,
    pub viewed_files: Vec<String>,
    pub changed_files: Vec<ChangedFile>,
    pub comments: Vec<ReviewComment>,
    pub remote_comments: Vec<RemoteReviewComment>,
    pub iterations: Vec<PullRequestIteration>,
    pub selected_file_path: Option<String>,
    pub selected_iteration_id: Option<String>,
    pub last_reviewed_iteration_id: Option<String>,
    pub last_reviewed_at_utc: Option<DateTime<Utc>>,
    pub is_archived: bool,
    pub last_opened_utc: DateTime<Utc>,
    pub completed_at_utc: Option<DateTime<Utc>>,
}

impl Default for ReviewSessionState {
    fn default() -> Self {
        Self {
            schema_version: 5,
            details: PullRequestDetails::default(),
            local_repository_path: None,
            reused_existing_repository: false,
            notes: String::new(),
            viewed_files: Vec::new(),
            changed_files: Vec::new(),
            comments: Vec::new(),
            remote_comments: Vec::new(),
            iterations: Vec::new(),
            selected_file_path: None,
            selected_iteration_id: None,
            last_reviewed_iteration_id: None,
            last_reviewed_at_utc: None,
            is_archived: false,
            last_opened_utc: Utc::now(),
            completed_at_utc: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MarkdownBlockKind {
    #[default]
    Paragraph,
    Heading1,
    Heading2,
    Heading3,
    Bullet,
    Numbered,
    Quote,
    Code,
    HorizontalRule,
    Heading4,
    Heading5,
    Heading6,
    TaskChecked,
    TaskUnchecked,
    Table,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MarkdownInlineKind {
    #[default]
    Text,
    Bold,
    Code,
    Link,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
pub struct MarkdownInlineRun {
    pub kind: MarkdownInlineKind,
    pub text: String,
    pub navigate_uri: Option<Url>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
pub struct MarkdownBlock {
    pub kind: MarkdownBlockKind,
    pub text: String,
    pub source_line_number: usize,
    pub source_line_count: usize,
    pub language: Option<String>,
    pub list_marker: Option<String>,
    pub inline_runs: Option<Vec<MarkdownInlineRun>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReviewActivityKind {
    #[default]
    Opened,
    CommentPublished,
    ReplyPublished,
    MarkedReviewed,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ReviewActivityRecord {
    pub id: String,
    pub session_key: String,
    pub provider: ReviewProvider,
    pub repository: String,
    pub pull_request_number: u32,
    pub pull_request_title: String,
    pub kind: ReviewActivityKind,
    pub at_utc: DateTime<Utc>,
    pub comment_delta: i32,
}

impl Default for ReviewActivityRecord {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            session_key: String::new(),
            provider: ReviewProvider::GitHub,
            repository: String::new(),
            pull_request_number: 0,
            pull_request_title: String::new(),
            kind: ReviewActivityKind::Opened,
            at_utc: Utc::now(),
            comment_delta: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ReviewActivityStoreState {
    pub schema_version: i32,
    pub records: Vec<ReviewActivityRecord>,
}

impl Default for ReviewActivityStoreState {
    fn default() -> Self {
        Self {
            schema_version: 1,
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct SymbolLocation {
    pub symbol: String,
    pub file_path: String,
    pub line_number: usize,
    pub line_text: String,
    pub score: i32,
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("unsupported or invalid pull request URL")]
    InvalidUrl,
    #[error("the URL is missing its pull request number")]
    MissingPullRequestNumber,
}
