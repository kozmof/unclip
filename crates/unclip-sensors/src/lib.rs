//! Built-in deterministic calculation sensors.
//!
//! This crate intentionally has no async runtime, HTTP client, RNG, or clock
//! dependency. Sensors receive established inputs and operation tokens from the
//! engine.

#![forbid(unsafe_code)]

use std::sync::Arc;

use unclip_plugin::{Registry, Result};

mod conditioned;
mod coverage;
mod kendall;
mod lehmer;
mod multi_observation;
mod permutation;
mod rbo;
mod residual;
mod support;
mod temporal;

pub use conditioned::{SelectedPairSensor, SelectedPairStatistic};
pub use coverage::CoverageSensor;
pub use kendall::KendallSensor;
pub use lehmer::LehmerSensor;
pub use multi_observation::MultiObservationSensor;
pub use permutation::PermutationSensor;
pub use rbo::RboSensor;
pub use residual::ResidualSensor;
pub use temporal::{TemporalSensor, TemporalStatistic};

/// Register every built-in calculation sensor.
pub fn register_all(registry: &mut Registry) -> Result<()> {
    registry.register_sensor(Arc::new(CoverageSensor::default()))?;
    registry.register_sensor(Arc::new(ResidualSensor::default()))?;
    registry.register_sensor(Arc::new(PermutationSensor::default()))?;
    registry.register_sensor(Arc::new(LehmerSensor::default()))?;
    registry.register_sensor(Arc::new(KendallSensor::default()))?;
    registry.register_sensor(Arc::new(RboSensor::default()))?;
    registry.register_sensor(Arc::new(MultiObservationSensor::trajectories()))?;
    for metric in [
        unclip_measure::PairwiseMetric::Spearman,
        unclip_measure::PairwiseMetric::Kendall,
        unclip_measure::PairwiseMetric::RelativeRankVariance,
        unclip_measure::PairwiseMetric::MutualInformation,
    ] {
        registry.register_sensor(Arc::new(MultiObservationSensor::matrix(metric)))?;
    }
    for statistic in [
        SelectedPairStatistic::CoForeground,
        SelectedPairStatistic::ConditionalMutualInformation,
        SelectedPairStatistic::PartialCorrelation,
    ] {
        registry.register_sensor(Arc::new(SelectedPairSensor::new(statistic)))?;
    }
    for statistic in [
        TemporalStatistic::LaggedDependency,
        TemporalStatistic::DynamicTimeWarping,
        TemporalStatistic::ChangePoints,
    ] {
        registry.register_sensor(Arc::new(TemporalSensor::new(statistic)))?;
    }
    Ok(())
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
        assert_eq!(registry.sensors().count(), 17);
    }
}
