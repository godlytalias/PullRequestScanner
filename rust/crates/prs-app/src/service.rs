use chrono::Utc;
use prs_core::url::PullRequestUrlError;
use prs_core::{
    CancellationToken, CoreError, DiffBlock, GitDiffService, LocalRepositoryService, ProcessError,
    ProcessRunner, PullRequestReference, RemoteReviewComment, ReviewActivityService, ReviewComment,
    ReviewCommentStatus, ReviewSessionState, SymbolLocation, SymbolNavigationService,
};
use prs_providers::{
    CliCredentialProvider, ProviderError, ReviewProvider, ReviewProviderFactory, ReviewProviderHttp,
};
use prs_store::{ReviewActivityStore, ReviewStateStore, StoreError};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

const APPLICATION_DIRECTORY_NAME: &str = "PullRequestScanner";
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommentPublishResult {
    pub published: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestReviewRange {
    pub base_commit_sha: String,
    pub head_commit_sha: String,
    pub iteration_id: Option<String>,
}

#[derive(Debug, Error)]
pub enum ReviewSessionServiceError {
    #[error(transparent)]
    Url(#[from] PullRequestUrlError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Core(CoreError),
    #[error(transparent)]
    Provider(ProviderError),
    #[error("{message}")]
    Authentication {
        reference: Box<PullRequestReference>,
        message: String,
    },
    #[error("the review operation was cancelled")]
    Cancelled,
    #[error("this pull request has no review iterations yet")]
    NoReviewIterations,
    #[error("the comment '{comment_id}' is no longer part of this saved review")]
    CommentNotFound { comment_id: String },
    #[error(
        "LOCALAPPDATA is unavailable; provide the Pull Request Scanner application data root explicitly"
    )]
    LocalApplicationDataUnavailable,
    #[error("could not initialize the review provider HTTP client: {0}")]
    HttpClientInitialization(#[source] reqwest::Error),
}

impl ReviewSessionServiceError {
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    #[must_use]
    pub const fn is_authentication(&self) -> bool {
        matches!(self, Self::Authentication { .. })
    }

    #[must_use]
    pub const fn authentication_reference(&self) -> Option<&PullRequestReference> {
        match self {
            Self::Authentication { reference, .. } => Some(reference),
            _ => None,
        }
    }
}

impl From<CoreError> for ReviewSessionServiceError {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::Authentication { reference, message } => {
                Self::Authentication { reference, message }
            }
            CoreError::Process(ProcessError::Cancelled { .. }) => Self::Cancelled,
            other => Self::Core(other),
        }
    }
}

impl From<ProviderError> for ReviewSessionServiceError {
    fn from(error: ProviderError) -> Self {
        match error {
            ProviderError::Authentication { reference, message } => {
                Self::Authentication { reference, message }
            }
            other => Self::Provider(other),
        }
    }
}

pub struct AppServices {
    review_sessions: ReviewSessionService,
}

impl AppServices {
    pub fn from_local_app_data() -> Result<Self, ReviewSessionServiceError> {
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .ok_or(ReviewSessionServiceError::LocalApplicationDataUnavailable)?;
        Self::from_local_data_root(PathBuf::from(local_app_data).join(APPLICATION_DIRECTORY_NAME))
    }

    pub fn from_local_data_root(
        local_data_root: impl Into<PathBuf>,
    ) -> Result<Self, ReviewSessionServiceError> {
        let local_data_root = local_data_root.into();
        let process_runner = ProcessRunner::new();
        let credential_provider = Arc::new(CliCredentialProvider::new());
        let client = reqwest::Client::builder()
            .timeout(PROVIDER_TIMEOUT)
            .build()
            .map_err(ReviewSessionServiceError::HttpClientInitialization)?;
        let provider_http = ReviewProviderHttp::new(client, Arc::clone(&credential_provider));
        let provider_factory = ReviewProviderFactory::from_http(provider_http);
        let state_store = ReviewStateStore::new(local_data_root.join("Reviews"));
        let activity = ReviewActivityService::new(ReviewActivityStore::new(
            local_data_root
                .join("Activity")
                .join("review-activity.json"),
        ));
        let repository_service = LocalRepositoryService::new_with_search_roots(
            process_runner.clone(),
            Arc::clone(&credential_provider),
            local_data_root.join("Repositories"),
            repository_search_roots(),
        );
        let diff_service = GitDiffService::new(process_runner.clone());
        let symbol_navigation_service = SymbolNavigationService::new(process_runner);

        Ok(Self {
            review_sessions: ReviewSessionService::new(
                provider_factory,
                repository_service,
                diff_service,
                symbol_navigation_service,
                state_store,
                credential_provider,
                activity,
            ),
        })
    }

    #[must_use]
    pub const fn review_sessions(&self) -> &ReviewSessionService {
        &self.review_sessions
    }

    #[must_use]
    pub fn review_activity(&self) -> &ReviewActivityService {
        self.review_sessions.activity()
    }
}

#[derive(Clone)]
pub struct ReviewSessionService {
    inner: Arc<ReviewSessionServiceInner>,
}

struct ReviewSessionServiceInner {
    provider_factory: ReviewProviderFactory,
    repository_service: LocalRepositoryService,
    diff_service: GitDiffService,
    symbol_navigation_service: SymbolNavigationService,
    state_store: ReviewStateStore,
    credential_provider: Arc<CliCredentialProvider>,
    activity: ReviewActivityService,
}

impl ReviewSessionService {
    #[allow(clippy::too_many_arguments)]
    fn new(
        provider_factory: ReviewProviderFactory,
        repository_service: LocalRepositoryService,
        diff_service: GitDiffService,
        symbol_navigation_service: SymbolNavigationService,
        state_store: ReviewStateStore,
        credential_provider: Arc<CliCredentialProvider>,
        activity: ReviewActivityService,
    ) -> Self {
        Self {
            inner: Arc::new(ReviewSessionServiceInner {
                provider_factory,
                repository_service,
                diff_service,
                symbol_navigation_service,
                state_store,
                credential_provider,
                activity,
            }),
        }
    }

    #[must_use]
    pub fn state_store(&self) -> &ReviewStateStore {
        &self.inner.state_store
    }

    #[must_use]
    pub fn activity(&self) -> &ReviewActivityService {
        &self.inner.activity
    }

    #[must_use]
    pub fn provider_factory(&self) -> &ReviewProviderFactory {
        &self.inner.provider_factory
    }

    #[must_use]
    pub fn repository_service(&self) -> &LocalRepositoryService {
        &self.inner.repository_service
    }

    #[must_use]
    pub fn diff_service(&self) -> &GitDiffService {
        &self.inner.diff_service
    }

    #[must_use]
    pub fn symbol_navigation_service(&self) -> &SymbolNavigationService {
        &self.inner.symbol_navigation_service
    }

    pub async fn load(
        &self,
        session_key: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<ReviewSessionState>, ReviewSessionServiceError> {
        ensure_not_cancelled(cancellation_token)?;
        self.inner
            .state_store
            .load(session_key)
            .await
            .map_err(Into::into)
    }

    pub async fn load_all(
        &self,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Vec<ReviewSessionState>, ReviewSessionServiceError> {
        ensure_not_cancelled(cancellation_token)?;
        self.inner.state_store.load_all().await.map_err(Into::into)
    }

    pub async fn open(
        &self,
        pull_request_url: &str,
        local_repository_path: Option<&Path>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ReviewSessionState, ReviewSessionServiceError> {
        ensure_not_cancelled(cancellation_token)?;
        let reference = prs_core::url::parse(pull_request_url)?;
        let provider = self.inner.provider_factory.get(reference.provider);
        let details =
            call_provider(provider.get_pull_request(&reference), cancellation_token).await?;
        let mut state = self
            .inner
            .state_store
            .load(&reference.session_key)
            .await?
            .unwrap_or_default();
        state.details = details;
        if !state.details.is_closed && !state.details.is_merged {
            state.is_archived = false;
        }

        let preferred_path = preferred_repository_path(
            local_repository_path,
            state.local_repository_path.as_deref(),
        );
        let repository = self
            .inner
            .repository_service
            .prepare_async(
                &state.details,
                preferred_path.as_deref(),
                cancellation_token,
            )
            .await
            .map_err(ReviewSessionServiceError::from)?;
        state.local_repository_path = Some(repository.path.to_string_lossy().into_owned());
        state.reused_existing_repository = repository.reused_existing_checkout;
        state.iterations = call_provider(
            provider.get_iterations(&state.details.reference),
            cancellation_token,
        )
        .await?;
        normalize_selected_iteration(&mut state);
        let range = Self::active_range(&state);
        state.changed_files = self
            .inner
            .diff_service
            .list_changed_files_async(
                &state,
                Some(&range.base_commit_sha),
                Some(&range.head_commit_sha),
                cancellation_token,
            )
            .await
            .map_err(ReviewSessionServiceError::from)?;
        state.remote_comments =
            call_provider(provider.get_comments(&state.details), cancellation_token).await?;
        state.last_opened_utc = Utc::now();
        self.inner.state_store.save(&state).await?;
        ensure_not_cancelled(cancellation_token)?;
        self.inner.activity.record_opened(&state).await?;
        Ok(state)
    }

    pub async fn refresh_repository(
        &self,
        state: &mut ReviewSessionState,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), ReviewSessionServiceError> {
        ensure_not_cancelled(cancellation_token)?;
        let provider = self
            .inner
            .provider_factory
            .get(state.details.reference.provider);
        state.details = call_provider(
            provider.get_pull_request(&state.details.reference),
            cancellation_token,
        )
        .await?;

        let local_path = non_blank_path(state.local_repository_path.as_deref().map(Path::new));
        let preferred_path = match local_path {
            Some(path)
                if !state.reused_existing_repository
                    && self.inner.repository_service.is_managed_cache_path(&path) =>
            {
                None
            }
            other => other,
        };
        let repository = self
            .inner
            .repository_service
            .prepare_async(
                &state.details,
                preferred_path.as_deref(),
                cancellation_token,
            )
            .await
            .map_err(ReviewSessionServiceError::from)?;
        state.local_repository_path = Some(repository.path.to_string_lossy().into_owned());
        state.reused_existing_repository = repository.reused_existing_checkout;
        state.iterations = call_provider(
            provider.get_iterations(&state.details.reference),
            cancellation_token,
        )
        .await?;
        normalize_selected_iteration(state);
        let range = Self::active_range(state);
        state.changed_files = self
            .inner
            .diff_service
            .list_changed_files_async(
                state,
                Some(&range.base_commit_sha),
                Some(&range.head_commit_sha),
                cancellation_token,
            )
            .await
            .map_err(ReviewSessionServiceError::from)?;
        state.remote_comments =
            call_provider(provider.get_comments(&state.details), cancellation_token).await?;
        self.inner.state_store.save(state).await?;
        Ok(())
    }

    pub async fn refresh_summary(
        &self,
        state: &mut ReviewSessionState,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), ReviewSessionServiceError> {
        ensure_not_cancelled(cancellation_token)?;
        let provider = self
            .inner
            .provider_factory
            .get(state.details.reference.provider);
        state.details = call_provider(
            provider.get_pull_request(&state.details.reference),
            cancellation_token,
        )
        .await?;
        state.iterations = call_provider(
            provider.get_iterations(&state.details.reference),
            cancellation_token,
        )
        .await?;
        normalize_selected_iteration(state);
        self.inner.state_store.save(state).await?;
        Ok(())
    }

    pub async fn select_iteration(
        &self,
        state: &mut ReviewSessionState,
        iteration_id: Option<&str>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), ReviewSessionServiceError> {
        ensure_not_cancelled(cancellation_token)?;
        state.selected_iteration_id = iteration_id.map(str::to_owned);
        normalize_selected_iteration(state);
        let range = Self::active_range(state);
        state.changed_files = self
            .inner
            .diff_service
            .list_changed_files_async(
                state,
                Some(&range.base_commit_sha),
                Some(&range.head_commit_sha),
                cancellation_token,
            )
            .await
            .map_err(ReviewSessionServiceError::from)?;
        self.inner.state_store.save(state).await?;
        Ok(())
    }

    pub async fn mark_iteration_reviewed(
        &self,
        state: &mut ReviewSessionState,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), ReviewSessionServiceError> {
        ensure_not_cancelled(cancellation_token)?;
        let latest_id = state
            .iterations
            .last()
            .map(|iteration| iteration.id.clone())
            .ok_or(ReviewSessionServiceError::NoReviewIterations)?;
        state.last_reviewed_iteration_id = Some(latest_id);
        state.last_reviewed_at_utc = Some(Utc::now());
        self.inner.state_store.save(state).await?;
        self.inner.activity.record_reviewed(state).await?;
        Ok(())
    }

    #[must_use]
    pub fn active_range(state: &ReviewSessionState) -> PullRequestReviewRange {
        let selected = state.iterations.iter().find(|iteration| {
            state
                .selected_iteration_id
                .as_deref()
                .is_some_and(|selected_id| iteration.id == selected_id)
        });
        match selected {
            Some(iteration) => PullRequestReviewRange {
                base_commit_sha: iteration.base_commit_sha.clone(),
                head_commit_sha: iteration.head_commit_sha.clone(),
                iteration_id: Some(iteration.id.clone()),
            },
            None => PullRequestReviewRange {
                base_commit_sha: state.details.base_commit_sha.clone(),
                head_commit_sha: state.details.head_commit_sha.clone(),
                iteration_id: None,
            },
        }
    }

    pub async fn load_diff(
        &self,
        state: &ReviewSessionState,
        path_spec: &str,
        context_lines: i32,
        complete_file: bool,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Vec<DiffBlock>, ReviewSessionServiceError> {
        let range = Self::active_range(state);
        self.inner
            .diff_service
            .get_diff_async(
                state,
                path_spec,
                context_lines,
                complete_file,
                Some(&range.base_commit_sha),
                Some(&range.head_commit_sha),
                cancellation_token,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn read_file(
        &self,
        state: &ReviewSessionState,
        path: &str,
        commit: Option<&str>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String, ReviewSessionServiceError> {
        self.inner
            .diff_service
            .read_file_async(
                state,
                commit.unwrap_or(&state.details.head_commit_sha),
                path,
                cancellation_token,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn find_definition(
        &self,
        state: &ReviewSessionState,
        symbol: &str,
        preferred_path: &str,
        commit: Option<&str>,
        source_line_number: Option<usize>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<SymbolLocation>, ReviewSessionServiceError> {
        self.inner
            .symbol_navigation_service
            .find_definition_async(
                state,
                symbol,
                preferred_path,
                commit,
                source_line_number,
                cancellation_token,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn publish_pending_comments(
        &self,
        state: &mut ReviewSessionState,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<CommentPublishResult, ReviewSessionServiceError> {
        let provider = self
            .inner
            .provider_factory
            .get(state.details.reference.provider);
        publish_pending_comments_with_provider(
            provider,
            state,
            &self.inner.state_store,
            &self.inner.activity,
            cancellation_token,
        )
        .await
    }

    pub async fn delete_comment(
        &self,
        state: &mut ReviewSessionState,
        comment_id: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), ReviewSessionServiceError> {
        ensure_not_cancelled(cancellation_token)?;
        let comment_index = state
            .comments
            .iter()
            .position(|comment| comment.id == comment_id)
            .ok_or_else(|| ReviewSessionServiceError::CommentNotFound {
                comment_id: comment_id.to_owned(),
            })?;
        let comment = state.comments[comment_index].clone();
        let published_remote_id = (comment.status == ReviewCommentStatus::Published)
            .then_some(comment.remote_id.as_deref())
            .flatten()
            .map(str::trim)
            .filter(|remote_id| !remote_id.is_empty())
            .map(str::to_owned);

        if published_remote_id.is_some() {
            let provider = self
                .inner
                .provider_factory
                .get(state.details.reference.provider);
            call_provider(
                provider.delete_comment(&state.details, &comment),
                cancellation_token,
            )
            .await?;
        }

        state.comments.remove(comment_index);
        if let Some(remote_id) = published_remote_id {
            state
                .remote_comments
                .retain(|existing| existing.remote_id != remote_id);
        }
        self.inner.state_store.save(state).await?;
        Ok(())
    }

    pub async fn reply_to_comment(
        &self,
        state: &mut ReviewSessionState,
        parent_comment: &RemoteReviewComment,
        body: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String, ReviewSessionServiceError> {
        let provider = self
            .inner
            .provider_factory
            .get(state.details.reference.provider);
        let remote_id = call_provider(
            provider.reply_to_comment(&state.details, parent_comment, body),
            cancellation_token,
        )
        .await?;
        state.remote_comments =
            call_provider(provider.get_comments(&state.details), cancellation_token).await?;
        self.inner.state_store.save(state).await?;
        self.inner.activity.record_reply(state, &remote_id).await?;
        Ok(remote_id)
    }

    pub async fn complete_review(
        &self,
        state: &mut ReviewSessionState,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), ReviewSessionServiceError> {
        ensure_not_cancelled(cancellation_token)?;
        state.completed_at_utc = Some(Utc::now());
        self.inner.activity.record_completed(state).await?;

        let remaining = self.inner.state_store.load_all().await?;
        let cache_is_shared = remaining.iter().any(|other| {
            other.details.reference.session_key != state.details.reference.session_key
                && other
                    .local_repository_path
                    .as_deref()
                    .zip(state.local_repository_path.as_deref())
                    .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
        });

        if !state.reused_existing_repository && !cache_is_shared {
            self.inner
                .repository_service
                .delete_cached_repository_async(
                    state.local_repository_path.as_deref().map(Path::new),
                    cancellation_token,
                )
                .await
                .map_err(ReviewSessionServiceError::from)?;
        }

        self.inner.state_store.delete(state).await?;
        Ok(())
    }

    pub async fn record_opened(
        &self,
        state: &ReviewSessionState,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), ReviewSessionServiceError> {
        ensure_not_cancelled(cancellation_token)?;
        self.inner.activity.record_opened(state).await?;
        Ok(())
    }

    pub async fn save(
        &self,
        state: &ReviewSessionState,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), ReviewSessionServiceError> {
        ensure_not_cancelled(cancellation_token)?;
        self.inner.state_store.save(state).await?;
        Ok(())
    }

    pub async fn delete(
        &self,
        state: &ReviewSessionState,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), ReviewSessionServiceError> {
        ensure_not_cancelled(cancellation_token)?;
        self.inner.state_store.delete(state).await?;
        Ok(())
    }

    pub async fn authenticate(
        &self,
        reference: &PullRequestReference,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), ReviewSessionServiceError> {
        self.inner
            .credential_provider
            .authenticate_with_cancellation(reference, cancellation_token)
            .await
            .map_err(Into::into)
    }
}

async fn publish_pending_comments_with_provider(
    provider: &dyn ReviewProvider,
    state: &mut ReviewSessionState,
    state_store: &ReviewStateStore,
    activity: &ReviewActivityService,
    cancellation_token: Option<&CancellationToken>,
) -> Result<CommentPublishResult, ReviewSessionServiceError> {
    let indexes = pending_comment_indexes(&state.comments);
    let details = state.details.clone();
    let mut published = 0;
    let mut failed = 0;

    for index in indexes {
        ensure_not_cancelled(cancellation_token)?;
        let comment = state.comments[index].clone();
        match call_provider(
            provider.publish_comment(&details, &comment),
            cancellation_token,
        )
        .await
        {
            Ok(remote_id) => {
                let stored_comment = &mut state.comments[index];
                stored_comment.remote_id = Some(remote_id.clone());
                stored_comment.error = None;
                stored_comment.status = ReviewCommentStatus::Published;
                stored_comment.published_at_utc = Some(Utc::now());
                published += 1;
                state_store.save(state).await?;
                activity.record_published_comment(state, &remote_id).await?;
            }
            Err(error) if !error.is_cancelled() => {
                let stored_comment = &mut state.comments[index];
                stored_comment.status = ReviewCommentStatus::Failed;
                stored_comment.error = Some(error.to_string());
                failed += 1;
                state_store.save(state).await?;
                if error.is_authentication() {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }

    Ok(CommentPublishResult { published, failed })
}

fn pending_comment_indexes(comments: &[ReviewComment]) -> Vec<usize> {
    comments
        .iter()
        .enumerate()
        .filter(|(_, comment)| {
            matches!(
                comment.status,
                ReviewCommentStatus::Pending | ReviewCommentStatus::Failed
            )
        })
        .map(|(index, _)| index)
        .collect()
}

fn normalize_selected_iteration(state: &mut ReviewSessionState) {
    if state
        .selected_iteration_id
        .as_deref()
        .is_some_and(|selected_id| {
            !selected_id.trim().is_empty()
                && !state
                    .iterations
                    .iter()
                    .any(|iteration| iteration.id == selected_id)
        })
    {
        state.selected_iteration_id = None;
    }
}

fn preferred_repository_path(
    explicit_path: Option<&Path>,
    saved_path: Option<&str>,
) -> Option<PathBuf> {
    non_blank_path(explicit_path).or_else(|| non_blank_path(saved_path.map(Path::new)))
}

fn non_blank_path(path: Option<&Path>) -> Option<PathBuf> {
    path.filter(|path| !path.to_string_lossy().trim().is_empty())
        .map(Path::to_path_buf)
}

fn repository_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        roots.push(home.join("repos"));
        roots.push(home.join("src"));
    }
    if let Some(configured) = std::env::var_os("PULLREQUEST_SCANNER_REPO_ROOTS") {
        roots
            .extend(std::env::split_paths(&configured).filter(|path| !path.as_os_str().is_empty()));
    }
    roots
}

fn ensure_not_cancelled(
    cancellation_token: Option<&CancellationToken>,
) -> Result<(), ReviewSessionServiceError> {
    if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(ReviewSessionServiceError::Cancelled);
    }
    Ok(())
}

async fn call_provider<T>(
    future: impl Future<Output = Result<T, ProviderError>>,
    cancellation_token: Option<&CancellationToken>,
) -> Result<T, ReviewSessionServiceError> {
    ensure_not_cancelled(cancellation_token)?;
    if let Some(cancellation_token) = cancellation_token {
        tokio::select! {
            biased;
            result = future => result.map_err(Into::into),
            () = cancellation_token.cancelled() => Err(ReviewSessionServiceError::Cancelled),
        }
    } else {
        future.await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use prs_core::{
        PullRequestDetails, PullRequestIteration, PullRequestReference,
        ReviewProvider as ModelProvider,
    };
    use std::sync::Mutex;

    struct RecordingProvider {
        publish_calls: Mutex<Vec<String>>,
        failing_comment_id: Option<String>,
    }

    #[async_trait]
    impl ReviewProvider for RecordingProvider {
        async fn get_pull_request(
            &self,
            _reference: &PullRequestReference,
        ) -> Result<PullRequestDetails, ProviderError> {
            Err(ProviderError::Message("not used by this test".into()))
        }

        async fn get_iterations(
            &self,
            _reference: &PullRequestReference,
        ) -> Result<Vec<PullRequestIteration>, ProviderError> {
            Err(ProviderError::Message("not used by this test".into()))
        }

        async fn get_comments(
            &self,
            _pull_request: &PullRequestDetails,
        ) -> Result<Vec<RemoteReviewComment>, ProviderError> {
            Err(ProviderError::Message("not used by this test".into()))
        }

        async fn publish_comment(
            &self,
            _pull_request: &PullRequestDetails,
            comment: &ReviewComment,
        ) -> Result<String, ProviderError> {
            self.publish_calls
                .lock()
                .expect("publish call lock is available")
                .push(comment.id.clone());
            if self
                .failing_comment_id
                .as_deref()
                .is_some_and(|id| id == comment.id)
            {
                return Err(ProviderError::Message("publish failed".into()));
            }
            Ok(format!("remote:{}", comment.id))
        }

        async fn delete_comment(
            &self,
            _pull_request: &PullRequestDetails,
            _comment: &ReviewComment,
        ) -> Result<(), ProviderError> {
            Err(ProviderError::Message("not used by this test".into()))
        }

        async fn reply_to_comment(
            &self,
            _pull_request: &PullRequestDetails,
            _comment: &RemoteReviewComment,
            _body: &str,
        ) -> Result<String, ProviderError> {
            Err(ProviderError::Message("not used by this test".into()))
        }
    }

    fn review_state() -> ReviewSessionState {
        ReviewSessionState {
            details: PullRequestDetails {
                reference: PullRequestReference {
                    provider: ModelProvider::GitHub,
                    repository: "widget".into(),
                    number: 7,
                    session_key: "github:example.test/octo-org/widget#7".into(),
                    ..Default::default()
                },
                base_commit_sha: "base".into(),
                head_commit_sha: "head".into(),
                title: "Review widget".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn active_range_normalizes_unknown_iteration_to_all_changes() {
        let mut state = review_state();
        state.iterations = vec![
            PullRequestIteration {
                id: "one".into(),
                base_commit_sha: "base-0".into(),
                head_commit_sha: "head-1".into(),
                ..Default::default()
            },
            PullRequestIteration {
                id: "two".into(),
                base_commit_sha: "head-1".into(),
                head_commit_sha: "head-2".into(),
                ..Default::default()
            },
        ];
        state.selected_iteration_id = Some("two".into());

        assert_eq!(
            ReviewSessionService::active_range(&state),
            PullRequestReviewRange {
                base_commit_sha: "head-1".into(),
                head_commit_sha: "head-2".into(),
                iteration_id: Some("two".into()),
            }
        );

        state.selected_iteration_id = Some(PullRequestIteration::ALL_CHANGES_ID.into());
        normalize_selected_iteration(&mut state);
        assert_eq!(state.selected_iteration_id, None);
        assert_eq!(
            ReviewSessionService::active_range(&state),
            PullRequestReviewRange {
                base_commit_sha: "base".into(),
                head_commit_sha: "head".into(),
                iteration_id: None,
            }
        );
    }

    #[tokio::test]
    async fn publishing_visits_each_pending_or_failed_comment_once_and_continues_after_failure() {
        let temp = tempfile::tempdir().unwrap();
        let state_store = ReviewStateStore::new(temp.path().join("Reviews"));
        let activity_store =
            ReviewActivityStore::new(temp.path().join("Activity/review-activity.json"));
        let activity = ReviewActivityService::new(activity_store.clone());
        let provider = RecordingProvider {
            publish_calls: Mutex::new(Vec::new()),
            failing_comment_id: Some("failure".into()),
        };
        let mut state = review_state();
        state.comments = vec![
            ReviewComment {
                id: "first".into(),
                status: ReviewCommentStatus::Pending,
                ..Default::default()
            },
            ReviewComment {
                id: "already-published".into(),
                status: ReviewCommentStatus::Published,
                ..Default::default()
            },
            ReviewComment {
                id: "failure".into(),
                status: ReviewCommentStatus::Failed,
                ..Default::default()
            },
            ReviewComment {
                id: "last".into(),
                status: ReviewCommentStatus::Pending,
                ..Default::default()
            },
        ];

        let result = publish_pending_comments_with_provider(
            &provider,
            &mut state,
            &state_store,
            &activity,
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            result,
            CommentPublishResult {
                published: 2,
                failed: 1
            }
        );
        assert_eq!(
            *provider
                .publish_calls
                .lock()
                .expect("publish call lock is available"),
            ["first", "failure", "last"]
        );
        assert_eq!(state.comments[0].status, ReviewCommentStatus::Published);
        assert_eq!(state.comments[1].status, ReviewCommentStatus::Published);
        assert_eq!(state.comments[2].status, ReviewCommentStatus::Failed);
        assert_eq!(state.comments[3].status, ReviewCommentStatus::Published);
        assert_eq!(activity_store.load().await.unwrap().len(), 2);

        let saved = state_store
            .load(&state.details.reference.session_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.comments, state.comments);
    }
}
