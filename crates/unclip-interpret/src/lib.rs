//! Built-in semantic interpreters.
//!
//! Interpreters assign provisional human-readable meaning to empirical
//! structures. Model access is supplied by the engine through
//! [`unclip_plugin::InterpretationIo`], keeping provider clients and credentials
//! out of this crate.

#![forbid(unsafe_code)]

use std::sync::Arc;

use unclip_plugin::{Registry, Result};

mod llm_label;
pub use llm_label::{LlmLabel, LlmLabelInterpreter};

/// Register every built-in interpreter.
pub fn register_all(registry: &mut Registry) -> Result<()> {
    registry.register_interpreter(Arc::new(LlmLabelInterpreter::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_explicit() {
        let mut registry = Registry::default();
        register_all(&mut registry).unwrap();
        let ids = registry
            .interpreters()
            .map(|plugin| plugin.descriptor().id.0.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["interpret.llm-label"]);
        let _: serde_json::Value = serde_json::from_str(
            registry
                .interpreters()
                .next()
                .unwrap()
                .descriptor()
                .params_schema,
        )
        .unwrap();
    }
}
