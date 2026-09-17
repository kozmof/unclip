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
const PRODUCES: &[MeasurementKind] = &[MeasurementKind::Structured];

pub struct ResidualSensor {
    descriptor: SensorDescriptor,
}

impl Default for ResidualSensor {
    fn default() -> Self {
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.residual"),
                version: Version::new(0, 1, 0),
                applicability: APPLICABILITY,
                evidence: EVIDENCE,
                produces: PRODUCES,
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}

impl Sensor for ResidualSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn applies_to(&self, ctx: &MeasureCtx<'_>) -> Applicability {
        if ctx.observations().is_empty() {
            return Applicability::NotApplicable {
                reason: "residuals require observations".into(),
            };
        }
        if ctx.alignments().is_empty() {
            return Applicability::NotApplicable {
                reason: "residuals require alignments".into(),
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
        Ok(vec![
            token.emit(self.measurement(
                "unmatched_units",
                support.observed_nodes,
                &support.unsupported_units,
            )),
            token.emit(self.measurement(
                "unexplained_relations",
                support.observed_relations,
                &support.unexplained_relations,
            )),
        ])
    }
}

impl ResidualSensor {
    fn measurement(&self, kind: &str, observed: usize, ids: &[String]) -> Measurement {
        let reading = if observed == 0 {
            Reading::NotApplicable {
                reason: format!("{kind} requires at least one observed item"),
            }
        } else {
            Reading::Value {
                value: MeasurementValue::Structured(serde_json::json!({
                    "count": ids.len(),
                    "ids": ids,
                })),
            }
        };
        Measurement {
            sensor: self.descriptor.id.clone(),
            sensor_version: self.descriptor.version.clone(),
            reading,
            confidence: None,
            sample_count: Some(observed),
            context: MeasurementContext {
                values: BTreeMap::from([("residual_kind".into(), serde_json::json!(kind))]),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use unclip_domain::{DomainId, DomainSnapshot, FrameId, MeasurementFrame};
    use unclip_epistemic::{
        hash_params, DependencyCollector, DerivedId, DomainVersion, EmitMetadata, FrameVersion,
        InferenceToken, SourceRef, Timestamp, Tracked,
    };
    use unclip_observe::{Alignment, Observation, ObservationId, ObservedUnit, ObservedUnitId};
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
    fn reports_unmatched_units_and_sparse_relations() {
        let domain = DomainSnapshot {
            id: DomainId::new("coffee"),
            version: DomainVersion::new("1"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("coffee.general"),
            version: FrameVersion::new("1"),
            axes: Vec::new(),
        };
        let observation = Observation {
            id: ObservationId::new("x-1"),
            source: SourceRef::new("fixture"),
            observed_at: None,
            units: vec![ObservedUnit {
                id: ObservedUnitId::new("unknown"),
                label: "unknown".into(),
                salience: None,
                uncertainty: None,
                context: BTreeMap::new(),
            }],
            relations: Vec::new(),
            context: BTreeMap::new(),
        };
        let alignment = Alignment {
            observation: observation.id.clone(),
            candidates: Vec::new(),
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
        let sensor = ResidualSensor::default();

        conformance::assert_sensor(&sensor, |plugin| {
            plugin.measure(
                &ctx,
                ctx.calculation_token(metadata("residual", "sensor.residual")),
            )
        });

        let values = sensor
            .measure(
                &ctx,
                ctx.calculation_token(metadata("residual", "sensor.residual")),
            )
            .unwrap();
        assert!(matches!(
            &values[0].value().reading,
            Reading::Value {
                value: MeasurementValue::Structured(value)
            } if value["count"] == 1 && value["ids"][0] == "x-1/unknown"
        ));
        assert!(matches!(
            &values[1].value().reading,
            Reading::NotApplicable { .. }
        ));
        assert_eq!(
            values[0].provenance().inputs,
            vec![DerivedId::new("alignment"), DerivedId::new("observation")]
        );
    }
}
