//! unclip-match — fast multi-pattern scanning over language surfaces.
//!
//! SQLite is the structured truth; daachorse is an in-memory matcher built
//! from database state (DRAFT §17).

pub mod dictionary;
pub mod matcher;
pub mod suggest;

pub use dictionary::{PatternEntry, PatternHit, PatternTarget};
pub use matcher::Matcher;
pub use suggest::{branch_text, suggest_o2m};
