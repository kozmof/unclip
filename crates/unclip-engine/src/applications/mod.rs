//! Applying a candidate to a domain to build the counterfactual it is tested
//! against.
//!
//! Each module validates the candidate shape it applies and constructs the
//! counterfactual snapshot; none of them mutates a stored domain. A domain
//! successor is only ever written by the revision ladder, after held-out
//! evidence has been recorded.

pub(crate) mod candidate;
pub(crate) mod coupling;
pub(crate) mod motif;
pub(crate) mod role;
pub(crate) mod transformation;
