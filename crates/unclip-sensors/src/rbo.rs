use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{Measurement, MeasurementContext, MeasurementKind, MeasurementValue, Reading};
use unclip_plugin::{
    Applicability, Capability, MeasureCtx, PluginError, Result, Sensor, SensorDescriptor,
};

use crate::permutation::{alignment_index, ranked_state};

const APPLICABILITY: &[Capability] = &[Capability::Alignment, Capability::RankingValue];
const PRODUCES: &[MeasurementKind] = &[MeasurementKind::Scalar];
const DEFAULT_P: f64 = 0.9;

pub struct RboSensor {
    descriptor: SensorDescriptor,
}

impl Default for RboSensor {
    fn default() -> Self {
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.rbo"),
                version: Version::new(0, 1, 0),
                applicability: APPLICABILITY,
                evidence: &[],
                produces: PRODUCES,
                params_schema: r#"{"type":"object","properties":{"p":{"type":"number","exclusiveMinimum":0.0,"exclusiveMaximum":1.0,"default":0.9}},"additionalProperties":false}"#,
            },
        }
    }
}

impl Sensor for RboSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn applies_to(&self, ctx: &MeasureCtx<'_>) -> Applicability {
        if ctx.rankings().is_empty() {
            return Applicability::NotApplicable {
                reason: "RBO requires a ranking".into(),
            };
        }
        if ctx.alignments().is_empty() {
            return Applicability::NotApplicable {
                reason: "RBO requires alignments".into(),
            };
        }
        if ctx.frame().axes.is_empty() {
            return Applicability::NotApplicable {
                reason: "RBO requires frame axes".into(),
            };
        }
        Applicability::Applicable
    }

    fn measure(
        &self,
        ctx: &MeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<Measurement>>> {
        let p = persistence_param(ctx.params())?;
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
                let state = alignments
                    .get(&ranking.observation)
                    .map(|alignment| ranked_state(ranking, alignment, &frame_units));
                let ranked_tiers = state.as_ref().map(|state| {
                    state
                        .tiers
                        .iter()
                        .map(|tier| {
                            tier.iter()
                                .map(|unit| frame_positions[unit])
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>()
                });
                let known = ranked_tiers
                    .as_ref()
                    .map_or(0, |tiers| tiers.iter().map(Vec::len).sum());
                let reading = ranked_tiers.filter(|tiers| !tiers.is_empty()).map_or(
                    Reading::InsufficientEvidence { have: 0, need: 1 },
                    |tiers| Reading::Value {
                        value: MeasurementValue::Scalar(rank_biased_overlap(&tiers, p)),
                    },
                );
                token.emit(Measurement {
                    sensor: self.descriptor.id.clone(),
                    sensor_version: self.descriptor.version.clone(),
                    reading,
                    confidence: None,
                    sample_count: Some(known),
                    context: MeasurementContext {
                        values: BTreeMap::from([
                            (
                                "observation".into(),
                                serde_json::json!(ranking.observation.0),
                            ),
                            ("p".into(), serde_json::json!(p)),
                        ]),
                    },
                })
            })
            .collect())
    }
}

fn persistence_param(params: &serde_json::Value) -> Result<f64> {
    let params = params
        .as_object()
        .ok_or_else(|| PluginError::Message("sensor.rbo parameters must be an object".into()))?;
    if let Some(name) = params.keys().find(|name| name.as_str() != "p") {
        return Err(PluginError::Message(format!(
            "sensor.rbo does not accept parameter {name}"
        )));
    }
    let p = params
        .get("p")
        .map_or(Some(DEFAULT_P), serde_json::Value::as_f64)
        .ok_or_else(|| PluginError::Message("sensor.rbo p must be a number".into()))?;
    if !(p.is_finite() && 0.0 < p && p < 1.0) {
        return Err(PluginError::Message(
            "sensor.rbo p must be finite and strictly between 0 and 1".into(),
        ));
    }
    Ok(p)
}

fn rank_biased_overlap(tiers: &[Vec<usize>], p: f64) -> f64 {
    let depth = tiers.iter().map(Vec::len).sum::<usize>();
    if depth == 0 {
        return 0.0;
    }
    let mut weighted_agreement = 0.0;
    let mut last_agreement = 0.0;
    for current_depth in 1..=depth {
        last_agreement = agreement_at_depth(tiers, current_depth);
        weighted_agreement += (1.0 - p) * p.powi((current_depth - 1) as i32) * last_agreement;
    }
    weighted_agreement + p.powi(depth as i32) * last_agreement
}

fn agreement_at_depth(tiers: &[Vec<usize>], depth: usize) -> f64 {
    let mut overlap = 0.0;
    let mut first_position = 1;
    for tier in tiers {
        let last_position = first_position + tier.len() - 1;
        let membership = if depth < first_position {
            0.0
        } else if depth >= last_position {
            1.0
        } else {
            (depth - first_position + 1) as f64 / tier.len() as f64
        };
        overlap += tier.iter().filter(|position| **position < depth).count() as f64 * membership;
        first_position = last_position + 1;
    }
    overlap / depth as f64
}

#[cfg(test)]
mod tests {
    use super::{persistence_param, rank_biased_overlap};

    #[test]
    fn identical_complete_and_partial_prefixes_have_full_overlap() {
        assert_eq!(rank_biased_overlap(&[vec![0], vec![1], vec![2]], 0.9), 1.0);
        assert_eq!(rank_biased_overlap(&[vec![0], vec![1]], 0.9), 1.0);
    }

    #[test]
    fn ties_are_fractional_and_order_independent() {
        let tied = rank_biased_overlap(&[vec![0, 1], vec![2]], 0.9);
        let tied_reordered = rank_biased_overlap(&[vec![1, 0], vec![2]], 0.9);
        assert_eq!(tied, tied_reordered);
        assert!(tied > 0.0 && tied < 1.0);
    }

    #[test]
    fn validates_persistence() {
        assert_eq!(persistence_param(&serde_json::json!({})).unwrap(), 0.9);
        assert_eq!(
            persistence_param(&serde_json::json!({"p": 0.8})).unwrap(),
            0.8
        );
        for invalid in [
            serde_json::json!({"p": 0.0}),
            serde_json::json!({"p": 1.0}),
            serde_json::json!({"p": "high"}),
            serde_json::json!({"unknown": true}),
        ] {
            assert!(persistence_param(&invalid).is_err());
        }
    }
}
