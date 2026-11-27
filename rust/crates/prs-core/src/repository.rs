use crate::error::CoreError;
use crate::process::{CancellationToken, ProcessError, ProcessResult, ProcessRunner};
use async_trait::async_trait;
use base64::Engine;
use percent_encoding::percent_decode_str;
use prs_model::{PullRequestDetails, PullRequestReference, ReviewProvider};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use url::Url;

#[derive(Clone, PartialEq, Eq)]
pub struct ReviewCredential {
    pub scheme: String,
    pub parameter: String,
}

impl ReviewCredential {
    #[must_use]
    pub fn new(scheme: impl Into<String>, parameter: impl Into<String>) -> Self {
        Self {
            scheme: scheme.into(),
            parameter: parameter.into(),
        }
    }
}

impl fmt::Debug for ReviewCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReviewCredential")
            .field("scheme", &self.scheme)
            .field("parameter", &"<redacted>")
            .finish()
    }
}

#[async_trait]
pub trait CredentialProvider: Send + Sync {
    async fn get_credential(
        &self,
        reference: &PullRequestReference,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<ReviewCredential>, CoreError>;
}

#[async_trait]
impl<T: CredentialProvider + ?Sized> CredentialProvider for Arc<T> {
    async fn get_credential(
        &self,
        reference: &PullRequestReference,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<ReviewCredential>, CoreError> {
        self.as_ref()
            .get_credential(reference, cancellation_token)
            .await
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoCredentialProvider;

#[async_trait]
impl CredentialProvider for NoCredentialProvider {
    async fn get_credential(
        &self,
        _reference: &PullRequestReference,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<ReviewCredential>, CoreError> {
        ensure_not_cancelled(cancellation_token)?;
        Ok(None)
    }
}

#[derive(Clone)]
pub struct StaticCredentialProvider {
    credential: Option<ReviewCredential>,
}

impl StaticCredentialProvider {
    #[must_use]
    pub const fn new(credential: Option<ReviewCredential>) -> Self {
        Self { credential }
    }
}

impl fmt::Debug for StaticCredentialProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StaticCredentialProvider")
            .field("has_credential", &self.credential.is_some())
            .finish()
    }
}

#[async_trait]
impl CredentialProvider for StaticCredentialProvider {
    async fn get_credential(
        &self,
        _reference: &PullRequestReference,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<ReviewCredential>, CoreError> {
        ensure_not_cancelled(cancellation_token)?;
        Ok(self.credential.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalRepository {
    pub path: PathBuf,
    pub reused_existing_checkout: bool,
}

impl LocalRepository {
    #[must_use]
    pub const fn new(path: PathBuf, reused_existing_checkout: bool) -> Self {
        Self {
            path,
            reused_existing_checkout,
        }
    }
}

pub struct RemoteUrlNormalizer;

impl RemoteUrlNormalizer {
    #[must_use]
    pub fn normalize(value: Option<&str>) -> String {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return String::new();
        };

        let scp_converted;
        let input = if value
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("git@"))
        {
            match value.find(':') {
                Some(colon_index) if colon_index > 0 => {
                    scp_converted = format!(
                        "https://{}/{}",
                        &value[4..colon_index],
                        &value[colon_index + 1..]
                    );
                    &scp_converted
                }
                _ => value,
            }
        } else {
            value
        };

        if let Ok(uri) = Url::parse(input)
            && let Some(host) = uri.host_str()
        {
            let path = percent_decode_str(uri.path())
                .decode_utf8_lossy()
                .replace('\\', "/");
            return format!(
                "{}/{}",
                host,
                trim_git_suffix(&path).trim_start_matches('/')
            )
            .to_lowercase();
        }

        trim_git_suffix(&input.replace('\\', "/")).to_lowercase()
    }
}

#[must_use]
pub fn normalize_remote_url(value: impl AsRef<str>) -> String {
    RemoteUrlNormalizer::normalize(Some(value.as_ref()))
}

fn trim_git_suffix(value: &str) -> &str {
    let value = value.trim_end_matches('/');
    if value.len() >= 4
        && value
            .get(value.len() - 4..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".git"))
    {
        &value[..value.len() - 4]
    } else {
        value
    }
}

pub struct GitAuthentication;

impl GitAuthentication {
    #[must_use]
    pub fn format_authorization_header(
        provider: ReviewProvider,
        credential: &ReviewCredential,
    ) -> String {
        if provider == ReviewProvider::GitHub && credential.scheme.eq_ignore_ascii_case("Bearer") {
            let credentials = format!("x-access-token:{}", credential.parameter)
                .chars()
                .map(|character| {
                    if character.is_ascii() {
                        character as u8
                    } else {
                        b'?'
                    }
                })
                .collect::<Vec<_>>();
            let basic = base64::engine::general_purpose::STANDARD.encode(credentials);
            format!("Authorization: Basic {basic}")
        } else {
            format!(
                "Authorization: {} {}",
                credential.scheme, credential.parameter
            )
        }
    }

    #[must_use]
    pub fn indicates_authentication_failure(result: &ProcessResult) -> bool {
        let error =
            format!("{}\n{}", result.standard_output, result.standard_error).to_ascii_lowercase();
        [
            "invalid credentials",
            "authentication failed",
            "could not read username",
            "could not read password",
            "terminal prompts disabled",
            "authentication required",
            "http 401",
            "http 403",
            "repository not found",
        ]
        .iter()
        .any(|message| error.contains(message))
    }
}

pub struct LocalRepositoryService {
    process_runner: ProcessRunner,
    credential_provider: Arc<dyn CredentialProvider>,
    cache_root: PathBuf,
    search_roots: Vec<PathBuf>,
}

impl fmt::Debug for LocalRepositoryService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalRepositoryService")
            .field("cache_root", &self.cache_root)
            .field("search_roots", &self.search_roots)
            .finish_non_exhaustive()
    }
}

impl LocalRepositoryService {
    #[must_use]
    pub fn new<P, C>(process_runner: ProcessRunner, credential_provider: C, cache_root: P) -> Self
    where
        P: Into<PathBuf>,
        C: CredentialProvider + 'static,
    {
        Self::new_with_search_roots(
            process_runner,
            credential_provider,
            cache_root,
            default_search_roots(),
        )
    }

    #[must_use]
    pub fn new_with_search_roots<P, C, I, S>(
        process_runner: ProcessRunner,
        credential_provider: C,
        cache_root: P,
        search_roots: I,
    ) -> Self
    where
        P: Into<PathBuf>,
        C: CredentialProvider + 'static,
        I: IntoIterator<Item = S>,
        S: Into<PathBuf>,
    {
        let cache_root = absolute_path(expand_environment_variables(cache_root.into()));
        let mut normalized_roots: Vec<PathBuf> = Vec::new();
        for search_root in search_roots {
            let search_root = search_root.into();
            if search_root.as_os_str().is_empty() {
                continue;
            }
            let search_root = absolute_path(expand_environment_variables(search_root));
            if !normalized_roots
                .iter()
                .any(|root| paths_equal(root, &search_root))
            {
                normalized_roots.push(search_root);
            }
        }
        Self {
            process_runner,
            credential_provider: Arc::new(credential_provider),
            cache_root,
            search_roots: normalized_roots,
        }
    }

    #[must_use]
    pub fn with_search_roots<P, C, I, S>(
        process_runner: ProcessRunner,
        credential_provider: C,
        cache_root: P,
        search_roots: I,
    ) -> Self
    where
        P: Into<PathBuf>,
        C: CredentialProvider + 'static,
        I: IntoIterator<Item = S>,
        S: Into<PathBuf>,
    {
        Self::new_with_search_roots(
            process_runner,
            credential_provider,
            cache_root,
            search_roots,
        )
    }

    pub async fn prepare(
        &self,
        pull_request: &PullRequestDetails,
        preferred_path: Option<&Path>,
    ) -> Result<LocalRepository, CoreError> {
        self.prepare_async(pull_request, preferred_path, None).await
    }

    pub async fn prepare_async(
        &self,
        pull_request: &PullRequestDetails,
        preferred_path: Option<&Path>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<LocalRepository, CoreError> {
        ensure_not_cancelled(cancellation_token)?;
        let git_environment = create_git_environment(
            pull_request,
            self.credential_provider.as_ref(),
            cancellation_token,
        )
        .await?;

        if let Some(preferred_path) = preferred_path {
            let preferred_root = self
                .get_repository_root(preferred_path, cancellation_token)
                .await?;
            self.ensure_matching_remote(
                &preferred_root,
                pull_request_clone_url(pull_request),
                cancellation_token,
            )
            .await?;
            self.fetch_review_commits(
                &preferred_root,
                pull_request,
                &git_environment,
                cancellation_token,
            )
            .await?;
            return Ok(LocalRepository::new(preferred_root, true));
        }

        let cache_path = self.cache_path(pull_request);
        if let Some(existing) = self
            .find_matching_checkout(pull_request, &cache_path, cancellation_token)
            .await?
        {
            self.fetch_review_commits(
                &existing.path,
                pull_request,
                &git_environment,
                cancellation_token,
            )
            .await?;
            return Ok(existing);
        }

        ensure_not_cancelled(cancellation_token)?;
        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| CoreError::Io {
                    operation: format!("create cache directory '{}'", parent.display()),
                    source,
                })?;
        }
        self.clone(
            &cache_path,
            pull_request,
            &git_environment,
            cancellation_token,
        )
        .await?;
        self.fetch_review_commits(
            &cache_path,
            pull_request,
            &git_environment,
            cancellation_token,
        )
        .await?;
        Ok(LocalRepository::new(cache_path, false))
    }

    #[must_use]
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    #[must_use]
    pub fn search_roots(&self) -> &[PathBuf] {
        &self.search_roots
    }

    #[must_use]
    pub fn cache_path(&self, pull_request: &PullRequestDetails) -> PathBuf {
        self.cache_root
            .join(create_hash(&RemoteUrlNormalizer::normalize(Some(
                pull_request_clone_url(pull_request),
            ))))
    }

    #[must_use]
    pub fn is_managed_cache_path(&self, repository_path: impl AsRef<OsStr>) -> bool {
        let path = absolute_path(PathBuf::from(repository_path.as_ref()));
        path != self.cache_root
            && path
                .parent()
                .is_some_and(|parent| paths_equal(parent, &self.cache_root))
    }

    pub async fn delete_cached_repository(
        &self,
        repository_path: Option<&Path>,
    ) -> Result<(), CoreError> {
        self.delete_cached_repository_async(repository_path, None)
            .await
    }

    pub async fn delete_cached_repository_async(
        &self,
        repository_path: Option<&Path>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), CoreError> {
        let Some(repository_path) = repository_path else {
            return Ok(());
        };
        let path = absolute_path(repository_path.to_path_buf());
        if !self.is_managed_cache_path(&path) {
            return Err(CoreError::operation(
                "Refusing to delete a repository outside the managed PullRequest Scanner cache.",
            ));
        }
        ensure_not_cancelled(cancellation_token)?;
        match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                tokio::fs::remove_file(&path)
                    .await
                    .map_err(|source| CoreError::Io {
                        operation: format!("remove cache symlink '{}'", path.display()),
                        source,
                    })?;
                return Ok(());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(CoreError::Io {
                    operation: format!("inspect cached repository '{}'", path.display()),
                    source,
                });
            }
        }

        let deletion_path = path.clone();
        tokio::task::spawn_blocking(move || remove_writable_directory(&deletion_path))
            .await
            .map_err(|error| {
                CoreError::operation(format!("cache deletion task failed: {error}"))
            })??;
        Ok(())
    }

    async fn find_matching_checkout(
        &self,
        pull_request: &PullRequestDetails,
        cache_path: &Path,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<LocalRepository>, CoreError> {
        if cache_path.is_dir()
            && self
                .try_remote_matches(
                    cache_path,
                    pull_request_clone_url(pull_request),
                    cancellation_token,
                )
                .await?
        {
            return Ok(Some(LocalRepository::new(cache_path.to_path_buf(), false)));
        }

        for search_root in &self.search_roots {
            if !search_root.is_dir() {
                continue;
            }
            let mut directories =
                tokio::fs::read_dir(search_root)
                    .await
                    .map_err(|source| CoreError::Io {
                        operation: format!(
                            "enumerate repository search root '{}'",
                            search_root.display()
                        ),
                        source,
                    })?;
            let mut candidates = Vec::new();
            while let Some(entry) =
                directories
                    .next_entry()
                    .await
                    .map_err(|source| CoreError::Io {
                        operation: format!(
                            "enumerate repository search root '{}'",
                            search_root.display()
                        ),
                        source,
                    })?
            {
                let file_type = entry.file_type().await.map_err(|source| CoreError::Io {
                    operation: format!("inspect search candidate '{}'", entry.path().display()),
                    source,
                })?;
                if file_type.is_dir() {
                    candidates.push(entry.path());
                }
            }
            candidates.sort();
            for candidate in candidates {
                if self
                    .try_remote_matches(
                        &candidate,
                        pull_request_clone_url(pull_request),
                        cancellation_token,
                    )
                    .await?
                {
                    return Ok(Some(LocalRepository::new(candidate, true)));
                }
            }
        }
        Ok(None)
    }

    async fn get_repository_root(
        &self,
        path: &Path,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<PathBuf, CoreError> {
        let full_path = absolute_path(expand_environment_variables(path.to_path_buf()));
        if !full_path.is_dir() {
            return Err(CoreError::operation(format!(
                "The selected repository path does not exist: {}",
                full_path.display()
            )));
        }
        let result = self
            .run_git(
                None,
                [
                    "-C",
                    full_path.to_string_lossy().as_ref(),
                    "rev-parse",
                    "--show-toplevel",
                ],
                None,
                cancellation_token,
            )
            .await?;
        if !result.succeeded() {
            return Err(CoreError::operation(format!(
                "The selected path is not a Git checkout: {}",
                full_path.display()
            )));
        }
        Ok(absolute_path(PathBuf::from(result.standard_output.trim())))
    }

    async fn ensure_matching_remote(
        &self,
        repository_path: &Path,
        expected_url: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), CoreError> {
        if self
            .try_remote_matches(repository_path, expected_url, cancellation_token)
            .await?
        {
            return Ok(());
        }
        let remote = self
            .run_git(
                Some(repository_path),
                ["remote", "get-url", "origin"],
                None,
                cancellation_token,
            )
            .await?;
        let actual = if remote.succeeded() && !remote.standard_output.trim().is_empty() {
            remote.standard_output.trim()
        } else {
            "origin is not configured"
        };
        Err(CoreError::operation(format!(
            "The selected checkout '{}' does not use the pull request repository ({actual} vs. {expected_url}).",
            repository_path.display()
        )))
    }

    async fn try_remote_matches(
        &self,
        repository_path: &Path,
        expected_url: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<bool, CoreError> {
        if !repository_path.is_dir() {
            return Ok(false);
        }
        let result = self
            .run_git(
                Some(repository_path),
                ["remote", "get-url", "origin"],
                None,
                cancellation_token,
            )
            .await?;
        Ok(result.succeeded()
            && RemoteUrlNormalizer::normalize(Some(&result.standard_output))
                == RemoteUrlNormalizer::normalize(Some(expected_url)))
    }

    async fn fetch_review_commits(
        &self,
        repository_path: &Path,
        pull_request: &PullRequestDetails,
        git_environment: &HashMap<String, String>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), CoreError> {
        let namespace = create_hash(&pull_request.reference.session_key);
        let head_ref = format!("refs/pullrequest-scanner/review/{namespace}/head");
        let base_ref = format!("refs/pullrequest-scanner/review/{namespace}/base");
        let mut failures = Vec::new();
        let fetch = self
            .run_git(
                Some(repository_path),
                [
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "origin",
                    &format!("+{}:{head_ref}", pull_request.source_fetch_ref),
                    &format!("+{}:{base_ref}", pull_request.target_fetch_ref),
                ],
                Some(git_environment),
                cancellation_token,
            )
            .await?;
        if fetch.succeeded() {
            ensure_not_cancelled(cancellation_token)?;
        } else {
            failures.push(fetch.clone());
            if GitAuthentication::indicates_authentication_failure(&fetch) {
                return Err(fetch_authentication_or_operation_error(
                    &pull_request.reference,
                    &failures,
                ));
            }

            let by_commit = self
                .run_git(
                    Some(repository_path),
                    [
                        "fetch",
                        "--quiet",
                        "--no-tags",
                        "origin",
                        &format!("+{}:{head_ref}", pull_request.head_commit_sha),
                        &format!("+{}:{base_ref}", pull_request.base_commit_sha),
                    ],
                    Some(git_environment),
                    cancellation_token,
                )
                .await?;
            failures.push(by_commit.clone());
            if !by_commit.succeeded() {
                return Err(fetch_authentication_or_operation_error(
                    &pull_request.reference,
                    &failures,
                ));
            }
        }

        self.ensure_commit_exists(
            repository_path,
            &pull_request.base_commit_sha,
            cancellation_token,
        )
        .await?;
        self.ensure_commit_exists(
            repository_path,
            &pull_request.head_commit_sha,
            cancellation_token,
        )
        .await
    }

    async fn ensure_commit_exists(
        &self,
        repository_path: &Path,
        commit: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), CoreError> {
        let result = self
            .run_git(
                Some(repository_path),
                ["cat-file", "-e", &format!("{commit}^{{commit}}")],
                None,
                cancellation_token,
            )
            .await?;
        if !result.succeeded() {
            return Err(CoreError::operation(format!(
                "Commit {commit} was not available after fetching the repository."
            )));
        }
        Ok(())
    }

    async fn clone(
        &self,
        target: &Path,
        pull_request: &PullRequestDetails,
        git_environment: &HashMap<String, String>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), CoreError> {
        if !self.is_managed_cache_path(target) {
            return Err(CoreError::operation(
                "Refusing to clone outside the managed PullRequest Scanner cache.",
            ));
        }
        let clone_url = pull_request_clone_url(pull_request);
        let mut failures = Vec::new();
        for arguments in clone_argument_sets(clone_url, target) {
            self.remove_failed_clone_target(target, cancellation_token)
                .await?;
            let clone = self
                .run_git(None, arguments, Some(git_environment), cancellation_token)
                .await?;
            if clone.succeeded() {
                return Ok(());
            }
            failures.push(clone.clone());
            if GitAuthentication::indicates_authentication_failure(&clone) {
                break;
            }
        }

        self.remove_failed_clone_target(target, cancellation_token)
            .await?;
        if failures
            .iter()
            .any(GitAuthentication::indicates_authentication_failure)
        {
            return Err(CoreError::authentication(
                &pull_request.reference,
                format!(
                    "{:?} Git access requires sign-in. Open the sign-in flow and retry the pull request.",
                    pull_request.reference.provider
                ),
            ));
        }
        let details = failures
            .iter()
            .map(format_error)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join("\n");
        Err(CoreError::operation(format!(
            "Git could not clone {clone_url}. {details}"
        )))
    }

    async fn remove_failed_clone_target(
        &self,
        target: &Path,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), CoreError> {
        ensure_not_cancelled(cancellation_token)?;
        match tokio::fs::symlink_metadata(target).await {
            Ok(metadata) if metadata.file_type().is_symlink() => tokio::fs::remove_file(target)
                .await
                .map_err(|source| CoreError::Io {
                    operation: format!("remove failed clone '{}'", target.display()),
                    source,
                }),
            Ok(_) => {
                let target = target.to_path_buf();
                tokio::task::spawn_blocking(move || remove_writable_directory(&target))
                    .await
                    .map_err(|error| {
                        CoreError::operation(format!("failed-clone cleanup task failed: {error}"))
                    })?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(CoreError::Io {
                operation: format!("inspect failed clone '{}'", target.display()),
                source,
            }),
        }
    }

    async fn run_git<I, A>(
        &self,
        repository_path: Option<&Path>,
        arguments: I,
        environment: Option<&HashMap<String, String>>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ProcessResult, CoreError>
    where
        I: IntoIterator<Item = A>,
        A: AsRef<OsStr>,
    {
        let mut git_arguments = Vec::<std::ffi::OsString>::new();
        if let Some(repository_path) = repository_path {
            git_arguments.push("-C".into());
            git_arguments.push(repository_path.as_os_str().to_os_string());
        }
        for argument in arguments {
            git_arguments.push(argument.as_ref().to_os_string());
        }
        self.process_runner
            .run_async("git", git_arguments, None, environment, cancellation_token)
            .await
            .map_err(map_git_process_error)
    }
}

async fn create_git_environment(
    pull_request: &PullRequestDetails,
    credential_provider: &dyn CredentialProvider,
    cancellation_token: Option<&CancellationToken>,
) -> Result<HashMap<String, String>, CoreError> {
    let mut environment = HashMap::from([("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())]);
    let credential = credential_provider
        .get_credential(&pull_request.reference, cancellation_token)
        .await?;
    if let Some(credential) = credential {
        let clone_url = pull_request_clone_url(pull_request);
        let clone_uri = Url::parse(clone_url).map_err(|_| {
            CoreError::operation(format!("The repository clone URL is invalid: {clone_url}"))
        })?;
        let host = clone_uri.host_str().ok_or_else(|| {
            CoreError::operation(format!("The repository clone URL is invalid: {clone_url}"))
        })?;
        let authority = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_string()
        } + &clone_uri
            .port()
            .map(|port| format!(":{port}"))
            .unwrap_or_default();
        environment.insert("GIT_CONFIG_COUNT".to_string(), "1".to_string());
        environment.insert(
            "GIT_CONFIG_KEY_0".to_string(),
            format!("http.{}://{authority}/.extraHeader", clone_uri.scheme()),
        );
        environment.insert(
            "GIT_CONFIG_VALUE_0".to_string(),
            GitAuthentication::format_authorization_header(
                pull_request.reference.provider,
                &credential,
            ),
        );
    }
    Ok(environment)
}

fn fetch_authentication_or_operation_error(
    reference: &PullRequestReference,
    failures: &[ProcessResult],
) -> CoreError {
    if failures
        .iter()
        .any(GitAuthentication::indicates_authentication_failure)
    {
        return CoreError::authentication(
            reference,
            format!(
                "{:?} Git access requires sign-in. Open the sign-in flow and retry the pull request.",
                reference.provider
            ),
        );
    }
    let details = failures
        .iter()
        .map(format_error)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join("\n");
    CoreError::operation(format!(
        "Git could not fetch the pull request commits. {details}"
    ))
}

fn format_error(result: &ProcessResult) -> String {
    let error = result.standard_error.trim();
    if error.is_empty() {
        format!("git exited with code {}.", result.exit_code)
    } else {
        error.to_string()
    }
}

fn create_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest[..10]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn pull_request_clone_url(pull_request: &PullRequestDetails) -> &str {
    if pull_request.clone_url.trim().is_empty() {
        &pull_request.reference.clone_url
    } else {
        &pull_request.clone_url
    }
}

fn clone_argument_sets(clone_url: &str, target: &Path) -> [Vec<String>; 2] {
    let target = target.to_string_lossy().into_owned();
    [
        vec![
            "clone".to_string(),
            "--filter=blob:none".to_string(),
            "--no-checkout".to_string(),
            "--quiet".to_string(),
            clone_url.to_string(),
            target.clone(),
        ],
        vec![
            "clone".to_string(),
            "--no-checkout".to_string(),
            "--quiet".to_string(),
            clone_url.to_string(),
            target,
        ],
    ]
}

fn map_git_process_error(error: ProcessError) -> CoreError {
    if error.is_not_found() {
        CoreError::operation(
            "Git is required to prepare the repository, but the git executable was not found on PATH.",
        )
    } else {
        CoreError::Process(error)
    }
}

fn ensure_not_cancelled(cancellation_token: Option<&CancellationToken>) -> Result<(), CoreError> {
    if let Some(token) = cancellation_token
        && token.is_cancelled()
    {
        return Err(ProcessError::Cancelled {
            program: "git".to_string(),
        }
        .into());
    }
    Ok(())
}

fn default_search_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else {
        return Vec::new();
    };
    let home = PathBuf::from(home);
    vec![home.join("repos"), home.join("src")]
}

fn expand_environment_variables(path: PathBuf) -> PathBuf {
    let input = path.to_string_lossy();
    let mut output = String::with_capacity(input.len());
    let mut cursor = input.as_ref();
    while let Some(start) = cursor.find('%') {
        output.push_str(&cursor[..start]);
        let remainder = &cursor[start + 1..];
        if let Some(end) = remainder.find('%') {
            let name = &remainder[..end];
            if let Some(value) = std::env::var_os(name) {
                output.push_str(&value.to_string_lossy());
            } else {
                output.push('%');
                output.push_str(name);
                output.push('%');
            }
            cursor = &remainder[end + 1..];
        } else {
            output.push('%');
            output.push_str(remainder);
            cursor = "";
        }
    }
    output.push_str(cursor);
    PathBuf::from(output)
}

fn absolute_path(path: PathBuf) -> PathBuf {
    std::path::absolute(&path).unwrap_or(path)
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn remove_writable_directory(path: &Path) -> Result<(), CoreError> {
    if !path.exists() {
        return Ok(());
    }
    make_writable_recursive(path).map_err(|source| CoreError::Io {
        operation: format!("make cached repository writable '{}'", path.display()),
        source,
    })?;
    std::fs::remove_dir_all(path).map_err(|source| CoreError::Io {
        operation: format!("delete cached repository '{}'", path.display()),
        source,
    })
}

fn make_writable_recursive(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    let mut permissions = metadata.permissions();
    if permissions.readonly() {
        #[cfg(windows)]
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        std::fs::set_permissions(path, permissions)?;
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            make_writable_recursive(&entry?.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = PathBuf::from("../../../target")
                .join("prs-core-tests")
                .join(format!(
                    "{}-{}-{label}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            let _ = remove_writable_directory(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = remove_writable_directory(&self.0);
        }
    }

    #[test]
    fn normalizes_common_git_remotes() {
        let cases = [
            (
                "git@github.com:octo-org/code-review-demo.git",
                "github.com/octo-org/code-review-demo",
            ),
            (
                "https://dev.azure.com/contoso/Project/_git/repository",
                "dev.azure.com/contoso/project/_git/repository",
            ),
            (
                "https://user@github.com/OWNER/Repo.git",
                "github.com/owner/repo",
            ),
            (
                "https://github.com/owner/repo%20name.git/",
                "github.com/owner/repo name",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(
                RemoteUrlNormalizer::normalize(Some(input)),
                expected,
                "{input}"
            );
        }
    }

    #[test]
    fn formats_and_redacts_credentials() {
        let credential = ReviewCredential::new("Bearer", "github-token");
        let header =
            GitAuthentication::format_authorization_header(ReviewProvider::GitHub, &credential);
        assert_eq!(
            header,
            format!(
                "Authorization: Basic {}",
                base64::engine::general_purpose::STANDARD.encode("x-access-token:github-token")
            )
        );
        assert!(!format!("{credential:?}").contains("github-token"));
    }

    #[test]
    fn preserves_azure_devops_authorization_scheme() {
        let credential = ReviewCredential::new("Basic", "ado-encoded-pat");
        assert_eq!(
            GitAuthentication::format_authorization_header(
                ReviewProvider::AzureDevOps,
                &credential
            ),
            "Authorization: Basic ado-encoded-pat"
        );
    }

    #[tokio::test]
    async fn builds_host_scoped_extra_header_in_process_environment_only() {
        let pull_request = PullRequestDetails {
            reference: PullRequestReference {
                provider: ReviewProvider::GitHub,
                ..Default::default()
            },
            clone_url: "https://github.example.test:8443/owner/repo.git".to_string(),
            ..Default::default()
        };
        let provider =
            StaticCredentialProvider::new(Some(ReviewCredential::new("Bearer", "secret-token")));
        let environment = create_git_environment(&pull_request, &provider, None)
            .await
            .unwrap();

        assert_eq!(
            environment.get("GIT_CONFIG_KEY_0").unwrap(),
            "http.https://github.example.test:8443/.extraHeader"
        );
        assert!(
            environment
                .get("GIT_CONFIG_VALUE_0")
                .unwrap()
                .starts_with("Authorization: Basic ")
        );
        assert_eq!(environment.get("GIT_TERMINAL_PROMPT").unwrap(), "0");
        assert_eq!(environment.get("GIT_CONFIG_COUNT").unwrap(), "1");
        for arguments in clone_argument_sets(&pull_request.clone_url, Path::new("target")) {
            assert!(
                arguments
                    .iter()
                    .all(|argument| !argument.contains("secret-token"))
            );
            assert!(arguments.iter().any(|argument| argument == "--no-checkout"));
        }
        assert!(
            clone_argument_sets(&pull_request.clone_url, Path::new("target"))[0]
                .contains(&"--filter=blob:none".to_string())
        );
        assert!(
            !clone_argument_sets(&pull_request.clone_url, Path::new("target"))[1]
                .contains(&"--filter=blob:none".to_string())
        );
    }

    #[test]
    fn detects_authentication_failures_without_classifying_missing_refs() {
        for (stderr, expected) in [
            (
                "remote: invalid credentials\nfatal: Authentication failed",
                true,
            ),
            ("fatal: could not read Username", true),
            ("fatal: repository not found", true),
            ("fatal: couldn't find remote ref refs/pull/1/head", false),
        ] {
            assert_eq!(
                GitAuthentication::indicates_authentication_failure(&ProcessResult {
                    exit_code: 128,
                    standard_output: String::new(),
                    standard_error: stderr.to_string(),
                }),
                expected
            );
        }
    }

    #[tokio::test]
    async fn deletes_only_managed_direct_children() {
        let root = TestDirectory::new("managed-cache");
        let cache = root.path().join("Cache");
        let managed = cache.join("managed");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(cache.join("managed").join("nested")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let service = LocalRepositoryService::new_with_search_roots(
            ProcessRunner::new(),
            NoCredentialProvider,
            cache,
            Vec::<PathBuf>::new(),
        );

        assert!(service.is_managed_cache_path(&managed));
        assert!(!service.is_managed_cache_path(managed.join("nested")));
        assert!(!service.is_managed_cache_path(&outside));
        assert!(
            service
                .delete_cached_repository(Some(&outside))
                .await
                .is_err()
        );
        assert!(outside.exists());
        service
            .delete_cached_repository(Some(&managed))
            .await
            .unwrap();
        assert!(!managed.exists());
    }
}
