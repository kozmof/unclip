//! One error type for every measurement failure this crate can produce.
//!
//! Each calculation keeps its own precise error enum — `SpectralError` states
//! whether a solver failed to converge, `TemporalError` names the offending
//! sequence index. Callers that drive several calculations, notably sensors,
//! need a single type to return, and flattening to a string at that point
//! discards exactly the distinctions the per-calculation enums exist to make.
//!
//! `MeasureError` is that single type. It converts from every calculation
//! error, so a sensor can use `?` throughout and still let its caller match on
//! what actually went wrong.

use crate::{
    CanonicalCorrelationError, CrossDomainCommunityError, CrossDomainInteractionMovementError,
    CrossDomainMutualInformationError, CrossProductTransferError, InvalidCommunityThreshold,
    InvalidIndependentBehavior, InvalidRegimePartition, PairwiseMatrixError, SpectralError,
    TemporalError, TrajectoryAlignmentError,
};

/// Any failure raised by a measurement calculation in this crate.
///
/// Every variant is `#[error(transparent)]`: the wrapper adds no prose of its
/// own, so a `MeasureError`'s message is exactly the underlying error's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MeasureError {
    #[error(transparent)]
    CanonicalCorrelation(#[from] CanonicalCorrelationError),
    #[error(transparent)]
    CommunityThreshold(#[from] InvalidCommunityThreshold),
    #[error(transparent)]
    CrossDomainCommunity(#[from] CrossDomainCommunityError),
    #[error(transparent)]
    CrossDomainInteractionMovement(#[from] CrossDomainInteractionMovementError),
    #[error(transparent)]
    CrossDomainMutualInformation(#[from] CrossDomainMutualInformationError),
    #[error(transparent)]
    CrossProductTransfer(#[from] CrossProductTransferError),
    #[error(transparent)]
    IndependentBehavior(#[from] InvalidIndependentBehavior),
    #[error(transparent)]
    PairwiseMatrix(#[from] PairwiseMatrixError),
    #[error(transparent)]
    RegimePartition(#[from] InvalidRegimePartition),
    #[error(transparent)]
    Spectral(#[from] SpectralError),
    #[error(transparent)]
    Temporal(#[from] TemporalError),
    #[error(transparent)]
    TrajectoryAlignment(#[from] TrajectoryAlignmentError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_preserves_the_underlying_message_and_identity() {
        let error = MeasureError::from(SpectralError::DidNotConverge { sweeps: 7 });
        assert_eq!(
            error.to_string(),
            SpectralError::DidNotConverge { sweeps: 7 }.to_string()
        );
        assert_eq!(
            error,
            MeasureError::Spectral(SpectralError::DidNotConverge { sweeps: 7 })
        );
    }

    /// The point of the type: two different failures stay distinguishable.
    #[test]
    fn distinct_calculations_remain_distinguishable_after_wrapping() {
        let spectral = MeasureError::from(SpectralError::NonFiniteResult);
        let temporal = MeasureError::from(TemporalError::InvalidChangeThreshold);
        assert_ne!(spectral, temporal);
        assert!(matches!(spectral, MeasureError::Spectral(_)));
        assert!(matches!(temporal, MeasureError::Temporal(_)));
    }
}
