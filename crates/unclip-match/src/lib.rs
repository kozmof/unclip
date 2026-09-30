//! unclip-match — pattern dictionary types and fast multi-pattern scanning.
//!
//! SQLite is the structured truth; daachorse is an in-memory matcher built
//! from database state.
//!
//! This crate is a leaf. It defines what a pattern and its target *are*
//! ([`PatternEntry`], [`PatternTarget`]) and how to find them in text
//! ([`Matcher`]), and it knows nothing about branches, observations, or
//! storage. Both subsystems scan text — the possibility space suggests o2m
//! values from prose, and `unclip-infer` derives observations from pattern
//! evidence — so the matcher sits under both instead of inside either.
//!
//! Validation lives with the vocabulary that defines it: `unclip-core` owns
//! `validate_pattern_entry`, because a `Branch` target has to satisfy the
//! branch-path rules, and `unclip-core` re-exports these types so its own
//! callers reach them unchanged.

#![forbid(unsafe_code)]

pub mod dictionary;
pub mod matcher;

pub use dictionary::{HitRef, PatternEntry, PatternHit, PatternTarget};
pub use matcher::Matcher;
