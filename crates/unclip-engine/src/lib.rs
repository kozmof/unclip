//! Stage orchestration and run planning for the semantic leveling engine.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use unclip_domain::{DomainSnapshot, MeasurementFrame};
use unclip_epistemic::{
    hash_params, Calculated, DependencyCollector, DerivedId, EmitMetadata, InferenceToken,
    Inferred, PluginId, SourceRef, Timestamp, Tracked,
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

/// Inputs controlled by the harness for one inference stage.
pub struct InferenceRun<'a> {
    pub id: &'a str,
    pub source: SourceRef,
    pub timestamp: Timestamp,
    pub params: &'a BTreeMap<PluginId, serde_json::Value>,
    pub io: &'a dyn unclip_plugin::InferenceIo,
}

/// Inferred aggregates together with typed handles for later calculation stages.
#[derive(Debug, Default)]
pub struct InferenceResults {
    pub outputs: Vec<Inferred<unclip_plugin::InferenceOutput>>,
    pub observations: Vec<Tracked<Observation>>,
    pub alignments: Vec<Tracked<Alignment>>,
    pub rankings: Vec<Tracked<PartialRanking>>,
}

impl InferenceResults {
    fn push(&mut self, output: Inferred<unclip_plugin::InferenceOutput>) {
        match output.value() {
            unclip_plugin::InferenceOutput::Bundle {
                observations,
                alignments,
                rankings,
            } => {
                self.observations.extend(
                    observations
                        .iter()
                        .cloned()
                        .map(|value| Tracked::from_derived(&output, value)),
                );
                self.alignments.extend(
                    alignments
                        .iter()
                        .cloned()
                        .map(|value| Tracked::from_derived(&output, value)),
                );
                self.rankings.extend(
                    rankings
                        .iter()
                        .cloned()
                        .map(|value| Tracked::from_derived(&output, value)),
                );
            }
            unclip_plugin::InferenceOutput::Observations(values) => {
                self.observations.extend(
                    values
                        .iter()
                        .cloned()
                        .map(|value| Tracked::from_derived(&output, value)),
                );
            }
            unclip_plugin::InferenceOutput::Alignments(values) => {
                self.alignments.extend(
                    values
                        .iter()
                        .cloned()
                        .map(|value| Tracked::from_derived(&output, value)),
                );
            }
            unclip_plugin::InferenceOutput::Rankings(values) => {
                self.rankings.extend(
                    values
                        .iter()
                        .cloned()
                        .map(|value| Tracked::from_derived(&output, value)),
                );
            }
            unclip_plugin::InferenceOutput::Structured(_) => {}
        }
        self.outputs.push(output);
    }
}

#[derive(Debug)]
pub struct PipelineResults {
    pub inference: InferenceResults,
    pub explanations: Vec<Calculated<Measurement>>,
    pub residuals: Vec<Calculated<Measurement>>,
    pub measurements: Vec<Calculated<Measurement>>,
}

fn calculation_stage(plugin: &PluginId) -> u8 {
    match plugin.0.as_str() {
        "sensor.coverage" => 0,
        "sensor.residual" => 1,
        _ => 2,
    }
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

    /// Execute all configured inferrers before any calculation sensor runs.
    pub async fn infer(
        &self,
        plan: &RunPlan,
        domain: &DomainSnapshot,
        run: InferenceRun<'_>,
    ) -> Result<InferenceResults> {
        let empty_params = serde_json::json!({});
        let mut results = InferenceResults::default();
        for inferrer in &plan.inferrers {
            let descriptor = inferrer.descriptor();
            let params = run.params.get(&descriptor.id).unwrap_or(&empty_params);
            let ctx = unclip_plugin::InferCtx {
                source: run.source.clone(),
                domain,
                params,
                io: run.io,
            };
            let metadata = EmitMetadata {
                id: DerivedId::new(format!("{}/{}", run.id, descriptor.id)),
                producer: descriptor.id.clone(),
                algorithm: descriptor.id.0.clone(),
                version: descriptor.version.clone(),
                params: params.clone(),
                params_hash: hash_params(params),
                source: Some(run.source.clone()),
                timestamp: run.timestamp.clone(),
                domain_version: Some(domain.version.clone()),
                frame_version: None,
                model: None,
            };
            let output = inferrer
                .infer(
                    &ctx,
                    InferenceToken::from_harness(metadata, DependencyCollector::default()),
                )
                .await?;
            results.push(output);
        }
        Ok(results)
    }

    /// Execute inference, explanation, residual, and measurement stages in order.
    pub async fn execute(
        &self,
        plan: &RunPlan,
        domain: &DomainSnapshot,
        frame: &MeasurementFrame,
        run: InferenceRun<'_>,
    ) -> Result<PipelineResults> {
        let measurement_run = MeasurementRun {
            id: run.id,
            timestamp: run.timestamp.clone(),
            params: run.params,
        };
        let inference = self.infer(plan, domain, run).await?;
        let calculated = self.measure(
            plan,
            MeasurementInputs {
                domain,
                frame,
                observations: &inference.observations,
                alignments: &inference.alignments,
                rankings: &inference.rankings,
            },
            measurement_run,
        )?;

        let mut explanations = Vec::new();
        let mut residuals = Vec::new();
        let mut measurements = Vec::new();
        for value in calculated {
            match calculation_stage(&value.value().sensor) {
                0 => explanations.push(value),
                1 => residuals.push(value),
                _ => measurements.push(value),
            }
        }
        Ok(PipelineResults {
            inference,
            explanations,
            residuals,
            measurements,
        })
    }

    /// Execute calculation sensors in stable stage and plugin-id order.
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
        sensors.sort_by(|left, right| {
            let left = left.descriptor();
            let right = right.descriptor();
            calculation_stage(&left.id)
                .cmp(&calculation_stage(&right.id))
                .then_with(|| left.id.cmp(&right.id))
        });

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
                "sensor.residual",
                "sensor.kendall",
                "sensor.lehmer",
                "sensor.permutation",
                "sensor.rbo",
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
    struct OrdinaryTextIo;

    #[async_trait::async_trait]
    impl unclip_plugin::InferenceIo for OrdinaryTextIo {
        async fn request(
            &self,
            _source: &SourceRef,
            params: &serde_json::Value,
        ) -> unclip_plugin::Result<serde_json::Value> {
            if params.get("min_confidence").is_some() {
                return Ok(serde_json::json!({
                    "text": "alpha beta",
                    "observation_id": "ordinary",
                    "patterns": [
                        {
                            "pattern": "alpha",
                            "target": {"kind": "o2o", "name": "unit", "value": "u1"}
                        },
                        {
                            "pattern": "beta",
                            "target": {"kind": "o2o", "name": "unit", "value": "u2"}
                        }
                    ]
                }));
            }
            Ok(serde_json::json!({
                "observation": {
                    "id": "ordinary",
                    "source": "notes/ordinary.txt",
                    "observed_at": null,
                    "units": [
                        {
                            "id": "hit-0-5",
                            "label": "alpha",
                            "salience": null,
                            "uncertainty": null,
                            "context": {}
                        },
                        {
                            "id": "hit-6-10",
                            "label": "beta",
                            "salience": null,
                            "uncertainty": null,
                            "context": {}
                        }
                    ],
                    "relations": [],
                    "context": {}
                },
                "evidence": [
                    {"pattern": "alpha", "salience": 0.9, "uncertainty": 0.1}
                ]
            }))
        }
    }

    #[tokio::test]
    async fn ordinary_text_is_ranked_before_state_sensors_run() {
        use unclip_domain::{FrameAxis, Relation, Unit, UnitId, UnitKind};
        use unclip_measure::MeasurementValue;

        let engine = Engine::with_builtins().unwrap();
        let profile = EngineProfile {
            inferrers: vec![
                PluginSelection::any("infer.pattern"),
                PluginSelection::any("infer.rank-pattern"),
            ],
            sensors: vec![
                PluginSelection::any("sensor.coverage"),
                PluginSelection::any("sensor.residual"),
                PluginSelection::any("sensor.permutation"),
            ],
            ..EngineProfile::default()
        };
        let plan = engine.plan(&profile).unwrap();
        let u1 = UnitId::new("u1");
        let u2 = UnitId::new("u2");
        let domain = DomainSnapshot {
            id: DomainId::new("ordinary"),
            version: DomainVersion::new("domain-1"),
            units: [
                (
                    u1.clone(),
                    Unit {
                        id: u1.clone(),
                        kind: UnitKind::AtomicMeaning,
                        label: None,
                        properties: BTreeMap::new(),
                    },
                ),
                (
                    u2.clone(),
                    Unit {
                        id: u2.clone(),
                        kind: UnitKind::AtomicMeaning,
                        label: None,
                        properties: BTreeMap::new(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
            relations: BTreeMap::<unclip_domain::RelationId, Relation>::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("ordinary.general"),
            version: FrameVersion::new("frame-1"),
            axes: vec![
                FrameAxis {
                    unit: u1.clone(),
                    label: None,
                },
                FrameAxis {
                    unit: u2.clone(),
                    label: None,
                },
            ],
        };
        let params = BTreeMap::from([
            (
                PluginId::new("infer.pattern"),
                serde_json::json!({"min_confidence": 0.7}),
            ),
            (
                PluginId::new("infer.rank-pattern"),
                serde_json::json!({"ties": "preserve", "unknown_tail": "preserve"}),
            ),
        ]);
        let results = engine
            .execute(
                &plan,
                &domain,
                &frame,
                InferenceRun {
                    id: "run-text",
                    source: SourceRef::new("notes/ordinary.txt"),
                    timestamp: Timestamp::new("2026-09-18T00:00:00Z"),
                    params: &params,
                    io: &OrdinaryTextIo,
                },
            )
            .await
            .unwrap();

        assert_eq!(results.inference.outputs.len(), 2);
        assert_eq!(results.inference.observations.len(), 2);
        assert_eq!(results.inference.alignments.len(), 1);
        assert_eq!(results.inference.rankings.len(), 1);
        assert_eq!(
            results.inference.rankings[0].id(),
            &DerivedId::new("run-text/infer.rank-pattern")
        );
        assert!(!results.explanations.is_empty());
        assert!(results
            .explanations
            .iter()
            .all(|value| value.value().sensor == PluginId::new("sensor.coverage")));
        assert!(!results.residuals.is_empty());
        assert!(results
            .residuals
            .iter()
            .all(|value| value.value().sensor == PluginId::new("sensor.residual")));
        assert_eq!(results.measurements.len(), 1);
        assert!(matches!(
            &results.measurements[0].value().reading,
            Reading::Value {
                value: MeasurementValue::Ranking(state)
            } if state.tiers == vec![vec![u1]]
                && state.unknown == vec![u2]
                && state.unresolved.is_empty()
        ));
        assert_eq!(
            results.measurements[0].provenance().inputs,
            vec![
                DerivedId::new("run-text/infer.pattern"),
                DerivedId::new("run-text/infer.rank-pattern")
            ]
        );
    }
}
