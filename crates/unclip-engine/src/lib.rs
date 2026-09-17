//! Stage orchestration and run planning for the semantic leveling engine.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use unclip_domain::{DomainSnapshot, MeasurementFrame};
use unclip_epistemic::{
    hash_params, Calculated, DependencyCollector, DerivedId, EmitMetadata, PluginId, Timestamp,
    Tracked,
};
use unclip_measure::{Measurement, MeasurementContext};
use unclip_observe::{Alignment, Observation, PartialRanking};
use unclip_plugin::{
    classify_sensor, EngineProfile, MeasureCtx, Registry, Result, RunPlan, SensorDecision,
};

/// Construct the runtime registry using explicit first-party registration.
pub fn builtin_registry() -> Result<Registry> {
    let mut registry = Registry::default();
    unclip_infer::register_all(&mut registry)?;
    unclip_sensors::register_all(&mut registry)?;
    Ok(registry)
}

/// Inputs already established by observation and inference stages.
pub struct MeasurementInputs<'a> {
    pub domain: &'a DomainSnapshot,
    pub frame: &'a MeasurementFrame,
    pub observations: &'a [Tracked<Observation>],
    pub alignments: &'a [Tracked<Alignment>],
    pub rankings: &'a [Tracked<PartialRanking>],
}

/// Reproducible inputs controlled by the run harness.
pub struct MeasurementRun<'a> {
    pub id: &'a str,
    pub timestamp: Timestamp,
    pub params: &'a BTreeMap<PluginId, serde_json::Value>,
}

/// Owns the plugin registry used to resolve and execute reproducible run plans.
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

    /// Execute calculation sensors in stable plugin-id order.
    ///
    /// Every invocation gets a fresh dependency collector so provenance cannot
    /// leak reads from one sensor into another.
    pub fn measure(
        &self,
        plan: &RunPlan,
        inputs: MeasurementInputs<'_>,
        run: MeasurementRun<'_>,
    ) -> Result<Vec<Calculated<Measurement>>> {
        let mut sensors = plan.sensors.iter().collect::<Vec<_>>();
        sensors.sort_by(|left, right| left.descriptor().id.cmp(&right.descriptor().id));

        let empty_params = serde_json::json!({});
        let mut measurements = Vec::new();
        for sensor in sensors {
            let descriptor = sensor.descriptor();
            let params = run.params.get(&descriptor.id).unwrap_or(&empty_params);
            let dependencies = DependencyCollector::default();
            let ctx = MeasureCtx::new(
                inputs.domain,
                inputs.frame,
                inputs.observations,
                inputs.alignments,
                inputs.rankings,
                params,
                dependencies,
            );
            let metadata = EmitMetadata {
                id: DerivedId::new(format!("{}/{}", run.id, descriptor.id)),
                producer: descriptor.id.clone(),
                algorithm: descriptor.id.0.clone(),
                version: descriptor.version.clone(),
                params: params.clone(),
                params_hash: hash_params(params),
                source: None,
                timestamp: run.timestamp.clone(),
                domain_version: None,
                frame_version: None,
                model: None,
            };
            match classify_sensor(sensor.as_ref(), &ctx, true) {
                SensorDecision::Run => {
                    measurements.extend(sensor.measure(&ctx, ctx.calculation_token(metadata))?);
                }
                SensorDecision::Record(reading) => {
                    measurements.push(ctx.calculation_token(metadata).emit(Measurement {
                        sensor: descriptor.id.clone(),
                        sensor_version: descriptor.version.clone(),
                        reading,
                        confidence: None,
                        sample_count: Some(inputs.observations.len()),
                        context: MeasurementContext::default(),
                    }));
                }
            }
        }
        Ok(measurements)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unclip_domain::{DomainId, FrameId};
    use unclip_epistemic::{DomainVersion, FrameVersion};
    use unclip_measure::Reading;
    use unclip_plugin::PluginSelection;

    #[test]
    fn empty_builtin_profile_resolves() {
        let engine = Engine::with_builtins().unwrap();
        let plan = engine.plan(&EngineProfile::default()).unwrap();
        assert!(plan.sensors.is_empty());
        assert!(plan.inferrers.is_empty());
        assert!(plan.comparators.is_empty());
    }

    #[test]
    fn measurement_stage_is_stable_sparse_and_versioned() {
        let engine = Engine::with_builtins().unwrap();
        let profile = EngineProfile {
            sensors: [
                "sensor.residual",
                "sensor.rbo",
                "sensor.permutation",
                "sensor.lehmer",
                "sensor.kendall",
                "sensor.coverage",
            ]
            .into_iter()
            .map(PluginSelection::any)
            .collect(),
            ..EngineProfile::default()
        };
        let plan = engine.plan(&profile).unwrap();
        let domain = DomainSnapshot {
            id: DomainId::new("test"),
            version: DomainVersion::new("domain-7"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("test.general"),
            version: FrameVersion::new("frame-3"),
            axes: Vec::new(),
        };
        let params = BTreeMap::new();
        let measurements = engine
            .measure(
                &plan,
                MeasurementInputs {
                    domain: &domain,
                    frame: &frame,
                    observations: &[],
                    alignments: &[],
                    rankings: &[],
                },
                MeasurementRun {
                    id: "run-1",
                    timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
                    params: &params,
                },
            )
            .unwrap();

        assert_eq!(measurements.len(), 6);
        assert_eq!(
            measurements
                .iter()
                .map(|measurement| measurement.value().sensor.0.as_str())
                .collect::<Vec<_>>(),
            vec![
                "sensor.coverage",
                "sensor.kendall",
                "sensor.lehmer",
                "sensor.permutation",
                "sensor.rbo",
                "sensor.residual",
            ]
        );
        assert!(measurements.iter().all(|measurement| matches!(
            measurement.value().reading,
            Reading::NotApplicable { .. }
        )));
        for measurement in measurements {
            assert_eq!(
                measurement.provenance().domain_version,
                Some(DomainVersion::new("domain-7"))
            );
            assert_eq!(
                measurement.provenance().frame_version,
                Some(FrameVersion::new("frame-3"))
            );
            assert!(measurement.provenance().inputs.is_empty());
        }
    }
}
