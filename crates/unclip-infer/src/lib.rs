//! Built-in semantic inferrers.

#![forbid(unsafe_code)]

use unclip_plugin::{Registry, Result};

/// Register every built-in inferrer.
pub fn register_all(_registry: &mut Registry) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_explicit_and_currently_empty() {
        let mut registry = Registry::default();
        register_all(&mut registry).unwrap();
        assert_eq!(registry.inferrers().count(), 0);
    }
}
