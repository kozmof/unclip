//! Null models: what the domain already accounts for without the candidate.
//!
//! A null model states the alternative a candidate has to beat — an existing
//! unit carrying the same pattern, a randomized co-occurrence baseline, a
//! zero-association coupling. None of them accepts or rejects anything; each
//! records what it found and leaves the decision explicit.
//!
//! [`exact_pattern`] holds the body shared by the role and transformation
//! nulls. Its module docs say why the motif null is not routed through it.

pub mod context;
pub mod coupling;
pub mod domain;
pub mod exact_pattern;
pub mod models;
pub mod motif;
pub mod ranking;
pub mod role;
pub mod transformation;
pub mod weight;
