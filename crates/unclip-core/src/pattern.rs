//! Pattern-dictionary validation.
//!
//! The data types themselves — [`PatternEntry`] and [`PatternTarget`] — live in
//! `unclip-match`, the leaf crate that owns the automaton, and are re-exported
//! here so the store and the CLI reach them through core as before.
//!
//! They used to live here, which made `unclip-match` depend on `unclip-core`
//! and so put the possibility space's foundation underneath the leveling stack
//! by way of `unclip-infer`. Only the *validation* belongs to core, and only
//! because a `Branch` target has to satisfy the branch-path rules that core
//! defines; matching a string against a dictionary does not.

pub use unclip_match::{PatternEntry, PatternTarget};

use crate::error::{CoreError, Result};
use crate::validate::{validate_domain_string, validate_path, MAX_DOMAIN_STRING_BYTES};

/// Validate a pattern dictionary entry before persistence or matching.
pub fn validate_pattern_entry(entry: &PatternEntry) -> Result<()> {
    let invalid = |reason: &str| CoreError::InvalidPattern(reason.to_string());

    if entry.pattern.trim().is_empty() {
        return Err(invalid("pattern must not be empty or whitespace-only"));
    }
    if entry.pattern.len() > MAX_DOMAIN_STRING_BYTES {
        return Err(invalid("pattern is oversized"));
    }
    if entry.pattern.chars().any(char::is_control) {
        return Err(invalid("pattern must not contain control characters"));
    }

    match &entry.target {
        PatternTarget::O2m { name, value } | PatternTarget::O2o { name, value } => {
            validate_domain_string(name).map_err(|_| {
                invalid("target name must not be empty or contain control characters")
            })?;
            validate_domain_string(value).map_err(|_| {
                invalid("target value must not be empty or contain control characters")
            })?;
        }
        PatternTarget::Branch { path } | PatternTarget::CollapsePattern { path } => {
            validate_path(path)
                .map_err(|_| invalid("branch target must be a valid absolute branch path"))?;
        }
    }
    Ok(())
}
