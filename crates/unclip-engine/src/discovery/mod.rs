//! Candidate generators: what the domain does not yet account for.
//!
//! Each generator reads established evidence and proposes candidates, never
//! accepting one. A candidate is a proposal with its supporting evidence
//! attached; whether it survives is settled by the experiment and revision
//! stages, not here.
//!
//! [`residual`] is the general persistent-residual generator; the others are
//! named for the structure they look for.

pub mod cross_domain;
pub mod motif;
pub mod pairwise;
pub mod relation;
pub mod residual;
pub mod structure;
pub mod temporal;
