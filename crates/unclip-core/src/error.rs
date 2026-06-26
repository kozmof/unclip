use thiserror::Error;

/// Domain-level errors raised by core validation and model logic.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("invalid branch `{path}`: {reason}")]
    InvalidBranch { path: String, reason: String },
}

pub type Result<T> = std::result::Result<T, CoreError>;
