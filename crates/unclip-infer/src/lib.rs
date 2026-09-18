//! Built-in semantic inferrers.

#![forbid(unsafe_code)]

use std::sync::Arc;

use unclip_plugin::{Registry, Result};

mod manual;
mod pattern;
mod rank_pattern;
pub use manual::ManualInferrer;
pub use pattern::PatternInferrer;
pub use rank_pattern::RankPatternInferrer;

/// Register every built-in inferrer.
pub fn register_all(registry: &mut Registry) -> Result<()> {
    registry.register_inferrer(Arc::new(ManualInferrer::default()))?;
    registry.register_inferrer(Arc::new(PatternInferrer::default()))?;
    registry.register_inferrer(Arc::new(RankPatternInferrer::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_explicit() {
        let mut registry = Registry::default();
        register_all(&mut registry).unwrap();
        assert_eq!(registry.inferrers().count(), 3);
    }
}
