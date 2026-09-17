//! Built-in deterministic calculation sensors.
//!
//! This crate intentionally has no async runtime, HTTP client, RNG, or clock
//! dependency. Sensors receive established inputs and operation tokens from the
//! engine.

#![forbid(unsafe_code)]

use std::sync::Arc;

use unclip_plugin::{Registry, Result};

mod coverage;

pub use coverage::CoverageSensor;

/// Register every built-in calculation sensor.
pub fn register_all(registry: &mut Registry) -> Result<()> {
    registry.register_sensor(Arc::new(CoverageSensor::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_excludes_effectful_dependencies() {
        let manifest = include_str!("../Cargo.toml");
        for forbidden in ["tokio", "reqwest", "rand", "chrono"] {
            let dependency = format!("{forbidden}.");
            assert!(
                !manifest
                    .lines()
                    .any(|line| line.trim_start().starts_with(&dependency)),
                "unclip-sensors must not depend on {forbidden}"
            );
        }
    }

    #[test]
    fn registration_is_explicit() {
        let mut registry = Registry::default();
        register_all(&mut registry).unwrap();
        assert_eq!(registry.sensors().count(), 1);
    }
}
