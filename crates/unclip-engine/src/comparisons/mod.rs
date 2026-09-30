//! Comparators: the typed difference between two measurements of one kind.
//!
//! A comparator declares the measurement kinds it understands and is rejected
//! by the engine if a profile points it at another. Each module here is named
//! for the kind it compares, not for the word "comparison": `ranking` holds the
//! rank comparators, `scalar` the numeric difference.
//!
//! None of these reduces a pair to a score. A pair that cannot be meaningfully
//! compared — a sparse reading on either side — is reported as such rather than
//! as a zero delta.

pub(crate) mod distribution;
pub(crate) mod event;
pub(crate) mod graph;
pub(crate) mod independence;
pub(crate) mod matrix;
pub(crate) mod partition;
pub(crate) mod profile;
pub(crate) mod ranking;
pub(crate) mod scalar;
pub(crate) mod spectral;
pub(crate) mod structured;
