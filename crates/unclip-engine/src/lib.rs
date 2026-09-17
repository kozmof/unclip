//! Stage orchestration and run planning for the semantic leveling engine.

#![forbid(unsafe_code)]

use unclip_plugin::{EngineProfile, Registry, Result, RunPlan};

/// Construct the runtime registry using explicit first-party registration.
pub fn builtin_registry() -> Result<Registry> {
    let mut registry = Registry::default();
    unclip_infer::register_all(&mut registry)?;
    unclip_sensors::register_all(&mut registry)?;
    Ok(registry)
}

/// Owns the plugin registry used to resolve reproducible run plans.
pub struct Engine {
    registry: Registry,
}

impl Engine {
    pub fn with_builtins() -> Result<Self> {
        Ok(Self {
            registry: builtin_registry()?,
        })
    }

    pub fn new(registry: Registry) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn plan(&self, profile: &EngineProfile) -> Result<RunPlan> {
        self.registry.resolve(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_builtin_profile_resolves() {
        let engine = Engine::with_builtins().unwrap();
        let plan = engine.plan(&EngineProfile::default()).unwrap();
        assert!(plan.sensors.is_empty());
        assert!(plan.inferrers.is_empty());
        assert!(plan.comparators.is_empty());
    }
}
