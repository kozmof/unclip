use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{Measurement, MeasurementContext, MeasurementKind, MeasurementValue, Reading};
use unclip_plugin::{
    Applicability, Capability, EvidenceRequirement, MeasureCtx, PluginError, Result, Sensor,
    SensorDescriptor,
};

use crate::permutation::{alignment_index, ranked_state};

const APPLICABILITY: &[Capability] = &[Capability::Alignment, Capability::RankingValue];
const EVIDENCE: &[EvidenceRequirement] = &[EvidenceRequirement::TotalOrder];
const PRODUCES: &[MeasurementKind] = &[MeasurementKind::Scalar];

pub struct KendallSensor {
    descriptor: SensorDescriptor,
}

impl Default for KendallSensor {
    fn default() -> Self {
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.kendall"),
                version: Version::new(0, 1, 0),
                applicability: APPLICABILITY,
                evidence: EVIDENCE,
                produces: PRODUCES,
                params_schema: r#"{"type":"object","properties":{"weighted":{"type":"boolean","default":false}},"additionalProperties":false}"#,
            },
        }
    }
}

impl Sensor for KendallSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn applies_to(&self, ctx: &MeasureCtx<'_>) -> Applicability {
        if ctx.rankings().is_empty() {
            return Applicability::NotApplicable {
                reason: "Kendall distance requires a ranking".into(),
            };
        }
        if ctx.alignments().is_empty() {
            return Applicability::NotApplicable {
                reason: "Kendall distance requires alignments".into(),
            };
        }
        if ctx.frame().axes.is_empty() {
            return Applicability::NotApplicable {
                reason: "Kendall distance requires frame axes".into(),
            };
        }
        Applicability::Applicable
    }

    fn measure(
        &self,
        ctx: &MeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<Measurement>>> {
        let weighted = weighted_param(ctx)?;
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
                                value: MeasurementValue::Scalar(kendall_distance(
                                    &permutation,
                                    weighted,
                                )),
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
                        values: BTreeMap::from([
                            (
                                "observation".into(),
                                serde_json::json!(ranking.observation.0),
                            ),
                            ("weighted".into(), serde_json::json!(weighted)),
                        ]),
                    },
                })
            })
            .collect())
    }
}

fn weighted_param(ctx: &MeasureCtx<'_>) -> Result<bool> {
    let params = ctx.params().as_object().ok_or_else(|| {
        PluginError::Message("sensor.kendall parameters must be an object".into())
    })?;
    if let Some(name) = params.keys().find(|name| name.as_str() != "weighted") {
        return Err(PluginError::Message(format!(
            "sensor.kendall does not accept parameter {name}"
        )));
    }
    params.get("weighted").map_or(Ok(false), |value| {
        value
            .as_bool()
            .ok_or_else(|| PluginError::Message("sensor.kendall weighted must be a boolean".into()))
    })
}

fn kendall_distance(permutation: &[usize], weighted: bool) -> f64 {
    let mut discordant = 0.0;
    let mut possible = 0.0;
    for left in 0..permutation.len() {
        for right in left + 1..permutation.len() {
            let weight = if weighted {
                reciprocal_rank(permutation[left]) + reciprocal_rank(permutation[right])
            } else {
                1.0
            };
            possible += weight;
            if permutation[left] > permutation[right] {
                discordant += weight;
            }
        }
    }
    if possible == 0.0 {
        0.0
    } else {
        discordant / possible
    }
}

fn reciprocal_rank(position: usize) -> f64 {
    1.0 / (position + 1) as f64
}

#[cfg(test)]
mod tests {
    use super::kendall_distance;

    #[test]
    fn identity_and_reverse_bound_the_distance() {
        assert_eq!(kendall_distance(&[0, 1, 2, 3], false), 0.0);
        assert_eq!(kendall_distance(&[3, 2, 1, 0], false), 1.0);
        assert_eq!(kendall_distance(&[3, 2, 1, 0], true), 1.0);
    }

    #[test]
    fn weighting_prioritizes_the_top_of_the_frame() {
        let top_swap = kendall_distance(&[1, 0, 2, 3], true);
        let bottom_swap = kendall_distance(&[0, 1, 3, 2], true);
        assert!(top_swap > bottom_swap);
        assert_eq!(
            kendall_distance(&[1, 0, 2, 3], false),
            kendall_distance(&[0, 1, 3, 2], false)
        );
    }
}
