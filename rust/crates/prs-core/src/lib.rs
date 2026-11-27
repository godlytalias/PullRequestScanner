#![forbid(unsafe_code)]

pub mod diff;
pub mod error;
pub mod markdown;
pub mod navigation;
pub mod process;
pub mod repository;
pub mod services;
pub mod url;

pub use diff::{
    GitDiffService, build_folds, build_side_by_side, parse_name_status, parse_unified_diff,
};
pub use error::CoreError;
pub use navigation::{
    SymbolCandidate, SymbolCandidateExtractor, SymbolNavigationService, extract_symbol_candidates,
};
pub use process::{CancellationToken, ProcessError, ProcessOptions, ProcessResult, ProcessRunner};
pub use prs_model::*;
pub use repository::{
    CredentialProvider, GitAuthentication, LocalRepository, LocalRepositoryService,
    NoCredentialProvider, RemoteUrlNormalizer, ReviewCredential, StaticCredentialProvider,
};
pub use services::{
    ReviewActivityDay, ReviewActivityService, ReviewActivityServiceError, ReviewSessionService,
    ReviewSessionServiceError,
};
