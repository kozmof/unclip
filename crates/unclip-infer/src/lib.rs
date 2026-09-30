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

/// Every inferrer accepts `file`, whatever else it declares.
///
/// An inferrer's parameters reach two places: the plugin itself, and the
/// [`InferenceIo`] implementation the harness supplied. `file` belongs to the
/// second — it names the document a recorded model response is read from,
/// which is how a run stays reproducible without a network call. It has to
/// appear in each inferrer's schema because the schema governs the whole
/// parameter object: `additionalProperties: false` on a schema that omits it
/// rejects the one key the stage itself honours.
///
/// Every inferrer here declares it, and `stage_params_are_declared` asserts
/// that, so adding an inferrer that forgets it fails rather than working until
/// someone sets `file`.
///
/// [`InferenceIo`]: unclip_plugin::InferenceIo
pub(crate) const PARAMS_SCHEMA_IO_ONLY: &str = r#"{
    "type":"object",
    "properties":{"file":{"type":"string","minLength":1}},
    "additionalProperties":false
}"#;

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

    /// `Engine::infer` validates parameters against the declared schema, so an
    /// inferrer whose schema omits `file` refuses the key the inference stage
    /// itself reads — and refuses it only once someone sets it. Every builtin
    /// declares it; this is what keeps the next one from forgetting.
    #[test]
    fn stage_params_are_declared_by_every_inferrer() {
        let mut registry = Registry::default();
        register_all(&mut registry).unwrap();
        for inferrer in registry.inferrers() {
            let descriptor = inferrer.descriptor();
            unclip_plugin::validate_params(
                descriptor.params_schema,
                &serde_json::json!({"file": "observations/recorded.yaml"}),
            )
            .unwrap_or_else(|violation| {
                panic!(
                    "{} must accept the shared `file` parameter: {violation}",
                    descriptor.id
                )
            });
        }
    }
}
