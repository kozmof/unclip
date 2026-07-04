//! Stable error contract for persistence operations.

use sea_orm::DbErr;
use unclip_core::CoreError;

/// Failures exposed by persistence repositories.
///
/// Expected domain and database failures remain inspectable while unexpected
/// implementation failures retain their context and source chain.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// A requested branch does not exist.
    #[error("branch not found: {path}")]
    NotFound { path: String },
    /// A branch with the same path already exists.
    #[error("branch already exists: {path}")]
    AlreadyExists { path: String },
    /// Optimistic concurrency detected a stale branch value.
    #[error("branch was modified by another process; reload and retry: {path}")]
    Conflict { path: String },
    /// A bounded query exceeded the safe hydration limit.
    #[error(
        "query matched more than {limit} branches; narrow the filters or use paginated access"
    )]
    QueryTooBroad { limit: u64 },
    /// A bulk query exceeded the safe in-memory result limit.
    #[error(
        "bulk query matched more than {limit} branches; narrow the filters or use a streaming workflow"
    )]
    BulkQueryTooBroad { limit: usize },
    /// A repository request was internally inconsistent.
    #[error("invalid repository request: {message}")]
    InvalidRequest { message: String },
    /// A domain value failed validation at the repository boundary.
    #[error(transparent)]
    InvalidDomain(#[from] CoreError),
    /// SQLite or SeaORM rejected an operation.
    #[error(transparent)]
    Database(#[from] DbErr),
    /// An invariant or serialization operation failed unexpectedly.
    #[error(transparent)]
    Unexpected(#[from] anyhow::Error),
}

/// Result type shared by store repository boundaries.
pub type StoreResult<T> = Result<T, StoreError>;
