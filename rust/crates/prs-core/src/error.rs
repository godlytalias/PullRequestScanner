use crate::process::ProcessError;
use prs_model::PullRequestReference;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("{0}")]
    Operation(String),
    #[error("{message}")]
    Authentication {
        reference: Box<PullRequestReference>,
        message: String,
    },
    #[error(transparent)]
    Process(#[from] ProcessError),
    #[error("{operation}: {source}")]
    Io {
        operation: String,
        #[source]
        source: std::io::Error,
    },
}

impl CoreError {
    #[must_use]
    pub fn operation(message: impl Into<String>) -> Self {
        Self::Operation(message.into())
    }

    #[must_use]
    pub fn is_authentication(&self) -> bool {
        matches!(self, Self::Authentication { .. })
    }

    #[must_use]
    pub fn authentication(reference: &PullRequestReference, message: impl Into<String>) -> Self {
        Self::Authentication {
            reference: Box::new(reference.clone()),
            message: message.into(),
        }
    }
}
