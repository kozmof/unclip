use std::collections::BTreeMap;

use semver::Version;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{Measurement, MeasurementContext, MeasurementKind, MeasurementValue, Reading};
use unclip_plugin::{
    Applicability, Capability, EvidenceRequirement, MeasureCtx, Result, Sensor, SensorDescriptor,
};

use crate::support::analyze;

const APPLICABILITY: &[Capability] = &[Capability::Observation, Capability::Alignment];
const EVIDENCE: &[EvidenceRequirement] = &[EvidenceRequirement::MinSamples(1)];
const PRODUCES: &[MeasurementKind] = &[MeasurementKind::Scalar];

pub struct CoverageSensor {
    descriptor: SensorDescriptor,
}

impl Default for CoverageSensor {
    fn default() -> Self {
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.coverage"),
                version: Version::new(0, 1, 0),
                applicability: APPLICABILITY,
                evidence: EVIDENCE,
                produces: PRODUCES,
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}

impl Sensor for CoverageSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn applies_to(&self, ctx: &MeasureCtx<'_>) -> Applicability {
        if ctx.observations().is_empty() {
            return Applicability::NotApplicable {
                reason: "coverage requires observations".into(),
            };
        }
        if ctx.alignments().is_empty() {
            return Applicability::NotApplicable {
                reason: "coverage requires alignments".into(),
            };
        }
        Applicability::Applicable
    }

    fn measure(
        &self,
        ctx: &MeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<Measurement>>> {
        let support = analyze(ctx);
        let observed_nodes = support.observed_nodes;
        let supported_nodes = support.supported_nodes();
        let observed_relations = support.observed_relations;
        let supported_relations = support.supported_relations();

        let node = ratio(supported_nodes, observed_nodes);
        let relation = ratio(supported_relations, observed_relations);
        let structural = ratio(
            supported_nodes + supported_relations,
            observed_nodes + observed_relations,
        );

        Ok(vec![
            token.emit(self.measurement("node_coverage", node, observed_nodes)),
            token.emit(self.measurement("relation_coverage", relation, observed_relations)),
            token.emit(self.measurement(
                "structural_coverage",
                structural,
                observed_nodes + observed_relations,
            )),
        ])
    }
}

impl CoverageSensor {
    fn measurement(&self, metric: &str, value: Option<f64>, sample_count: usize) -> Measurement {
        let reading = value.map_or_else(
            || Reading::NotApplicable {
                reason: format!("{metric} requires at least one observed item"),
            },
            |value| Reading::Value {
                value: MeasurementValue::Scalar(value),
            },
        );
        Measurement {
            sensor: self.descriptor.id.clone(),
            sensor_version: self.descriptor.version.clone(),
            reading,
            confidence: None,
            sample_count: Some(sample_count),
            context: MeasurementContext {
                values: BTreeMap::from([("metric".into(), serde_json::json!(metric))]),
            },
        }
    }
}

fn ratio(supported: usize, observed: usize) -> Option<f64> {
    (observed > 0).then(|| supported as f64 / observed as f64)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use unclip_domain::{
        DomainId, DomainSnapshot, FrameId, MeasurementFrame, Relation, RelationId, Unit, UnitId,
        UnitKind,
    };
    use unclip_epistemic::{
        hash_params, DependencyCollector, DerivedId, DomainVersion, EmitMetadata, FrameVersion,
        InferenceToken, ParameterHash, SourceRef, Timestamp, Tracked,
    };
    use unclip_observe::{
        Alignment, AlignmentCandidate, Observation, ObservationId, ObservedRelation,
        ObservedRelationId, ObservedUnit, ObservedUnitId,
    };
    use unclip_plugin::conformance;

    use super::*;

    fn metadata(id: &str, producer: &str) -> EmitMetadata {
        let params = serde_json::json!({});
        EmitMetadata {
            id: DerivedId::new(id),
            producer: PluginId::new(producer),
            algorithm: producer.into(),
            version: Version::new(0, 1, 0),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
            domain_version: None,
            frame_version: None,
            model: None,
        }
    }

    #[test]
    fn reports_node_relation_and_structural_coverage() {
        let sensory = UnitId::new("sensory");
        let social = UnitId::new("social");
        let domain = DomainSnapshot {
            id: DomainId::new("coffee"),
            version: DomainVersion::new("1"),
            units: BTreeMap::from([
                (
                    sensory.clone(),
                    Unit {
                        id: sensory.clone(),
                        kind: UnitKind::AtomicMeaning,
                        label: Some("sensory".into()),
                        properties: BTreeMap::new(),
                    },
                ),
                (
                    social.clone(),
                    Unit {
                        id: social.clone(),
                        kind: UnitKind::AtomicMeaning,
                        label: Some("social".into()),
                        properties: BTreeMap::new(),
                    },
                ),
            ]),
            relations: BTreeMap::from([(
                RelationId::new("sensory-social"),
                Relation {
                    id: RelationId::new("sensory-social"),
                    source: sensory.clone(),
                    target: social.clone(),
                    kind: "affects".into(),
                    properties: BTreeMap::new(),
                },
            )]),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("coffee.general"),
            version: FrameVersion::new("1"),
            axes: Vec::new(),
        };
        let appearance = ObservedUnitId::new("appearance");
        let sharing = ObservedUnitId::new("sharing");
        let unknown = ObservedUnitId::new("unknown");
        let observation = Observation {
            id: ObservationId::new("x-1"),
            source: SourceRef::new("fixture"),
            observed_at: None,
            units: vec![
                ObservedUnit {
                    id: appearance.clone(),
                    label: "appearance".into(),
                    salience: None,
                    uncertainty: None,
                    context: BTreeMap::new(),
                },
                ObservedUnit {
                    id: sharing.clone(),
                    label: "sharing".into(),
                    salience: None,
                    uncertainty: None,
                    context: BTreeMap::new(),
                },
                ObservedUnit {
                    id: unknown,
                    label: "unknown".into(),
                    salience: None,
                    uncertainty: None,
                    context: BTreeMap::new(),
                },
            ],
            relations: vec![ObservedRelation {
                id: ObservedRelationId::new("r-1"),
                source: appearance.clone(),
                target: sharing.clone(),
                kind: "affects".into(),
                uncertainty: None,
            }],
            context: BTreeMap::new(),
        };
        let alignment = Alignment {
            observation: observation.id.clone(),
            candidates: vec![
                AlignmentCandidate {
                    observed: appearance,
                    domain: sensory,
                    confidence: 0.9,
                    evidence: vec![],
                },
                AlignmentCandidate {
                    observed: sharing,
                    domain: social,
                    confidence: 0.8,
                    evidence: vec![],
                },
            ],
        };
        let inferred_observation = InferenceToken::from_harness(
            metadata("observation", "infer.fixture"),
            Default::default(),
        )
        .emit(observation);
        let inferred_alignment = InferenceToken::from_harness(
            metadata("alignment", "infer.fixture"),
            Default::default(),
        )
        .emit(alignment);
        let observations = vec![Tracked::from(&inferred_observation)];
        let alignments = vec![Tracked::from(&inferred_alignment)];
        let params = serde_json::json!({});
        let ctx = MeasureCtx::new(
            &domain,
            &frame,
            &observations,
            &alignments,
            &[],
            &params,
            DependencyCollector::default(),
        );
        let sensor = CoverageSensor::default();

        conformance::assert_sensor(&sensor, |plugin| {
            plugin.measure(
                &ctx,
                ctx.calculation_token(metadata("coverage", "sensor.coverage")),
            )
        });

        let values = sensor
            .measure(
                &ctx,
                ctx.calculation_token(metadata("coverage", "sensor.coverage")),
            )
            .unwrap();
        let scalars = values
            .iter()
            .map(|derived| match &derived.value().reading {
                Reading::Value {
                    value: MeasurementValue::Scalar(value),
                } => *value,
                other => panic!("unexpected reading: {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(scalars, vec![2.0 / 3.0, 1.0, 3.0 / 4.0]);
        assert_eq!(
            values[0].provenance().inputs,
            vec![DerivedId::new("alignment"), DerivedId::new("observation")]
        );
        assert_eq!(
            values[0].provenance().params_hash,
            ParameterHash::new("fnv1a64:08f44b07b5901a25")
        );
    }
}
