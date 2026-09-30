//! Null models: what the domain already accounts for without the candidate.
//!
//! A null model states the alternative a candidate has to beat — an existing
//! unit carrying the same pattern, a randomized co-occurrence baseline, a
//! zero-association coupling. None of them accepts or rejects anything; each
//! records what it found and leaves the decision explicit.
//!
//! [`exact_pattern`] holds the body shared by the role and transformation
//! nulls. Its module docs say why the motif null is not routed through it.

pub(crate) mod context;
pub(crate) mod coupling;
pub(crate) mod domain;
pub(crate) mod exact_pattern;
pub(crate) mod models;
pub(crate) mod motif;
pub(crate) mod ranking;
pub(crate) mod role;
pub(crate) mod transformation;
pub(crate) mod weight;
