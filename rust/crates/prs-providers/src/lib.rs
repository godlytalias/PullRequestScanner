#![forbid(unsafe_code)]

use async_trait::async_trait;
use prs_model::{
    PullRequestDetails, PullRequestIteration, PullRequestReference, RemoteReviewComment,
    ReviewComment,
};
use thiserror::Error;

mod azure_devops;
mod credential;
mod formatter;
mod github;
mod http;
mod util;

pub use azure_devops::AzureDevOpsReviewProviderClient;
pub use credential::CliCredentialProvider;
pub use formatter::ReviewCommentFormatter;
pub use github::GitHubReviewProviderClient;
pub use http::ReviewProviderHttp;
pub use prs_core::{CredentialProvider, ReviewCredential};

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("{0}")]
    Message(String),
    #[error("authentication required for {reference:?}: {message}")]
    Authentication {
        reference: Box<PullRequestReference>,
        message: String,
    },
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[async_trait]
pub trait ReviewProvider: Send + Sync {
    async fn get_pull_request(
        &self,
        reference: &PullRequestReference,
    ) -> Result<PullRequestDetails, ProviderError>;

    async fn get_iterations(
        &self,
        reference: &PullRequestReference,
    ) -> Result<Vec<PullRequestIteration>, ProviderError>;

    async fn get_comments(
        &self,
        pull_request: &PullRequestDetails,
    ) -> Result<Vec<RemoteReviewComment>, ProviderError>;

    async fn publish_comment(
        &self,
        pull_request: &PullRequestDetails,
        comment: &ReviewComment,
    ) -> Result<String, ProviderError>;

    async fn delete_comment(
        &self,
        pull_request: &PullRequestDetails,
        comment: &ReviewComment,
    ) -> Result<(), ProviderError>;

    async fn reply_to_comment(
        &self,
        pull_request: &PullRequestDetails,
        comment: &RemoteReviewComment,
        body: &str,
    ) -> Result<String, ProviderError>;
}

pub struct ReviewProviderFactory {
    github: GitHubReviewProviderClient,
    azure_devops: AzureDevOpsReviewProviderClient,
}

impl ReviewProviderFactory {
    #[must_use]
    pub fn new(
        github: GitHubReviewProviderClient,
        azure_devops: AzureDevOpsReviewProviderClient,
    ) -> Self {
        Self {
            github,
            azure_devops,
        }
    }

    #[must_use]
    pub fn from_http(http: ReviewProviderHttp) -> Self {
        Self::new(
            GitHubReviewProviderClient::new(http.clone()),
            AzureDevOpsReviewProviderClient::new(http),
        )
    }

    #[must_use]
    pub fn get(&self, provider: prs_model::ReviewProvider) -> &dyn ReviewProvider {
        match provider {
            prs_model::ReviewProvider::GitHub => &self.github,
            prs_model::ReviewProvider::AzureDevOps => &self.azure_devops,
        }
    }

    #[must_use]
    pub fn github(&self) -> &GitHubReviewProviderClient {
        &self.github
    }

    #[must_use]
    pub fn azure_devops(&self) -> &AzureDevOpsReviewProviderClient {
        &self.azure_devops
    }
}
