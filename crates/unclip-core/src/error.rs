use thiserror::Error;

/// Domain-level errors raised by core validation and model logic.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("frame `{0}` has no slot named `{1}`")]
    UnknownSlot(String, String),

    #[error("branch `{path}` violates frame slot `{slot}`: {reason}")]
    FrameViolation {
        path: String,
        slot: String,
        reason: String,
    },
}

pub type Result<T> = std::result::Result<T, CoreError>;
