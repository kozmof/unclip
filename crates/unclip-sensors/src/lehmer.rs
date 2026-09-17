use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{Measurement, MeasurementContext, MeasurementKind, MeasurementValue, Reading};
use unclip_plugin::{
    Applicability, Capability, EvidenceRequirement, MeasureCtx, Result, Sensor, SensorDescriptor,
};

use crate::permutation::{alignment_index, ranked_state};

const APPLICABILITY: &[Capability] = &[Capability::Alignment, Capability::RankingValue];
const EVIDENCE: &[EvidenceRequirement] = &[EvidenceRequirement::TotalOrder];
const PRODUCES: &[MeasurementKind] = &[MeasurementKind::Vector];

pub struct LehmerSensor {
    descriptor: SensorDescriptor,
}

impl Default for LehmerSensor {
    fn default() -> Self {
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.lehmer"),
                version: Version::new(0, 1, 0),
                applicability: APPLICABILITY,
                evidence: EVIDENCE,
                produces: PRODUCES,
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}

impl Sensor for LehmerSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn applies_to(&self, ctx: &MeasureCtx<'_>) -> Applicability {
        if ctx.rankings().is_empty() {
            return Applicability::NotApplicable {
                reason: "Lehmer encoding requires a ranking".into(),
            };
        }
        if ctx.alignments().is_empty() {
            return Applicability::NotApplicable {
                reason: "Lehmer encoding requires alignments".into(),
            };
        }
        if ctx.frame().axes.is_empty() {
            return Applicability::NotApplicable {
                reason: "Lehmer encoding requires frame axes".into(),
            };
        }
        Applicability::Applicable
    }

    fn measure(
        &self,
        ctx: &MeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<Measurement>>> {
        let frame_units = ctx
            .frame()
            .axes
            .iter()
            .map(|axis| axis.unit.clone())
            .collect::<BTreeSet<_>>();
        let frame_positions = ctx
            .frame()
            .axes
            .iter()
            .enumerate()
            .map(|(index, axis)| (axis.unit.clone(), index))
            .collect::<BTreeMap<_, _>>();
        let alignments = alignment_index(ctx, &frame_units);

        Ok(ctx
            .rankings()
            .iter()
            .map(|tracked| {
                let ranking = ctx.read(tracked);
                let reading = alignments
                    .get(&ranking.observation)
                    .map(|alignment| ranked_state(ranking, alignment, &frame_units))
                    .filter(|state| state.is_total(frame_units.len()))
                    .map_or(
                        Reading::InsufficientEvidence { have: 0, need: 1 },
                        |state| {
                            let permutation = state
                                .tiers
                                .iter()
                                .map(|tier| frame_positions[&tier[0]])
                                .collect::<Vec<_>>();
                            Reading::Value {
                                value: MeasurementValue::Vector(
                                    lehmer_code(&permutation)
                                        .into_iter()
                                        .map(|digit| digit as f64)
                                        .collect(),
                                ),
                            }
                        },
                    );
                token.emit(Measurement {
                    sensor: self.descriptor.id.clone(),
                    sensor_version: self.descriptor.version.clone(),
                    reading,
                    confidence: None,
                    sample_count: Some(frame_units.len()),
                    context: MeasurementContext {
                        values: BTreeMap::from([(
                            "observation".into(),
                            serde_json::json!(ranking.observation.0),
                        )]),
                    },
                })
            })
            .collect())
    }
}

fn lehmer_code(permutation: &[usize]) -> Vec<usize> {
    permutation
        .iter()
        .enumerate()
        .map(|(index, value)| {
            permutation[index + 1..]
                .iter()
                .filter(|following| *following < value)
                .count()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::lehmer_code;

    #[test]
    fn encodes_the_design_example() {
        assert_eq!(lehmer_code(&[2, 3, 4, 0, 1]), [2, 2, 2, 0, 0]);
    }

    #[test]
    fn identity_has_an_all_zero_code() {
        assert_eq!(lehmer_code(&[0, 1, 2, 3]), [0, 0, 0, 0]);
    }
}
