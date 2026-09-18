//! Built-in semantic inferrers.

#![forbid(unsafe_code)]

use std::sync::Arc;

use unclip_plugin::{Registry, Result};

mod manual;
pub use manual::ManualInferrer;

/// Register every built-in inferrer.
pub fn register_all(registry: &mut Registry) -> Result<()> {
    registry.register_inferrer(Arc::new(ManualInferrer::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_explicit() {
        let mut registry = Registry::default();
        register_all(&mut registry).unwrap();
        assert_eq!(registry.inferrers().count(), 1);
    }
}
