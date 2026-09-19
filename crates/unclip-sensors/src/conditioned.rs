//! Explicit pair selection for foreground and conditioned rank statistics.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use semver::Version;
use serde::Deserialize;
use unclip_domain::UnitId;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    co_foreground_frequency, conditional_mutual_information, construct_rank_trajectories,
    partial_correlation, Measurement, MeasurementContext, MeasurementKind, MeasurementValue,
    RankPosition, RankTrajectory, Reading,
};
use unclip_plugin::{
    Applicability, Capability, EvidenceRequirement, MeasureCtx, PluginError, Result, Sensor,
    SensorDescriptor,
};

use crate::multi_observation::batch_states;

#[derive(Debug, Clone, Copy)]
pub enum SelectedPairStatistic {
    CoForeground,
    ConditionalMutualInformation,
    PartialCorrelation,
}

pub struct SelectedPairSensor {
    descriptor: SensorDescriptor,
    statistic: SelectedPairStatistic,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForegroundParams {
    left: UnitId,
    right: UnitId,
    foreground_rank: NonZeroUsize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConditionalParams {
    left: UnitId,
    right: UnitId,
    #[serde(default)]
    conditioning_variables: Vec<UnitId>,
}

impl SelectedPairSensor {
    pub fn new(statistic: SelectedPairStatistic) -> Self {
        let (id, evidence, schema): (_, &'static [EvidenceRequirement], _) = match statistic {
            SelectedPairStatistic::CoForeground => (
                "sensor.co-foreground",
                &[EvidenceRequirement::MinSamples(1)],
                r#"{"type":"object","additionalProperties":false,"required":["left","right","foreground_rank"],"properties":{"left":{"type":"string","minLength":1},"right":{"type":"string","minLength":1},"foreground_rank":{"type":"integer","minimum":1}}}"#,
            ),
            SelectedPairStatistic::ConditionalMutualInformation => (
                "sensor.conditional-mutual-information",
                &[
                    EvidenceRequirement::MinSamples(2),
                    EvidenceRequirement::MinConditioningVariables(1),
                ],
                CONDITIONAL_SCHEMA,
            ),
            SelectedPairStatistic::PartialCorrelation => (
                "sensor.partial-correlation",
                &[
                    EvidenceRequirement::MinSamples(4),
                    EvidenceRequirement::MinConditioningVariables(1),
                ],
                CONDITIONAL_SCHEMA,
            ),
        };
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new(id),
                version: Version::new(0, 1, 0),
                applicability: &[
                    Capability::MultiObservation,
                    Capability::RankingValue,
                    Capability::Alignment,
                ],
                evidence,
                produces: &[MeasurementKind::Scalar],
                params_schema: schema,
            },
            statistic,
        }
    }
}

const CONDITIONAL_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["left","right"],"properties":{"left":{"type":"string","minLength":1},"right":{"type":"string","minLength":1},"conditioning_variables":{"type":"array","items":{"type":"string","minLength":1},"maxItems":1,"uniqueItems":true,"default":[]}}}"#;

fn invalid(message: impl Into<String>) -> PluginError {
    PluginError::Message(message.into())
}

fn find<'a>(ranks: &'a [RankTrajectory], unit: &UnitId) -> Result<&'a RankTrajectory> {
    ranks.iter().find(|rank| rank.unit == *unit).ok_or_else(|| {
        invalid(format!(
            "selected unit {} is not in the measurement frame",
            unit.0
        ))
    })
}

impl Sensor for SelectedPairSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn applies_to(&self, ctx: &MeasureCtx<'_>) -> Applicability {
        if ctx.frame().axes.is_empty() {
            Applicability::NotApplicable {
                reason: "selected-pair statistics require frame axes".into(),
            }
        } else {
            Applicability::Applicable
        }
    }

    fn measure(
        &self,
        ctx: &MeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<Measurement>>> {
        let (left, right, condition, foreground) = match self.statistic {
            SelectedPairStatistic::CoForeground => {
                let params: ForegroundParams = serde_json::from_value(ctx.params().clone())
                    .map_err(|e| invalid(e.to_string()))?;
                (
                    params.left,
                    params.right,
                    None,
                    Some(params.foreground_rank),
                )
            }
            _ => {
                let params: ConditionalParams = serde_json::from_value(ctx.params().clone())
                    .map_err(|e| invalid(e.to_string()))?;
                if params.conditioning_variables.len() > 1 {
                    return Err(invalid(
                        "this sensor supports exactly one conditioning variable",
                    ));
                }
                (
                    params.left,
                    params.right,
                    params.conditioning_variables.into_iter().next(),
                    None,
                )
            }
        };
        if left == right
            || condition
                .as_ref()
                .is_some_and(|unit| unit == &left || unit == &right)
        {
            return Err(invalid(
                "selected pair and conditioning units must be distinct",
            ));
        }
        let (units, states) = batch_states(ctx)?;
        let ranks = construct_rank_trajectories(&units, &states);
        let x = find(&ranks, &left)?;
        let y = find(&ranks, &right)?;
        let z = condition
            .as_ref()
            .map(|unit| find(&ranks, unit))
            .transpose()?;
        let sample_count = x
            .samples
            .iter()
            .zip(&y.samples)
            .enumerate()
            .filter(|(index, (x, y))| {
                matches!(x.position, RankPosition::Ranked { .. })
                    && matches!(y.position, RankPosition::Ranked { .. })
                    && z.is_none_or(|z| {
                        matches!(z.samples[*index].position, RankPosition::Ranked { .. })
                    })
            })
            .count();
        let mut context = BTreeMap::from([
            (
                "observations".into(),
                serde_json::json!(states.iter().map(|(id, _)| id).collect::<Vec<_>>()),
            ),
            ("pair".into(), serde_json::json!([left, right])),
        ]);
        if let Some(unit) = condition {
            context.insert("conditioning_variables".into(), serde_json::json!([unit]));
        }
        if let Some(cutoff) = foreground {
            context.insert("foreground_rank".into(), serde_json::json!(cutoff));
        }
        let (value, need, missing_condition) = match self.statistic {
            SelectedPairStatistic::CoForeground => (
                co_foreground_frequency(
                    x,
                    y,
                    foreground.ok_or_else(|| invalid("missing foreground rank"))?,
                )
                .map_err(|e| invalid(format!("{e:?}")))?
                .map(|stat| stat.value),
                1,
                false,
            ),
            SelectedPairStatistic::ConditionalMutualInformation => (
                match z {
                    Some(z) => conditional_mutual_information(x, y, z)
                        .map_err(|e| invalid(format!("{e:?}")))?
                        .map(|stat| stat.value),
                    None => None,
                },
                2,
                z.is_none(),
            ),
            SelectedPairStatistic::PartialCorrelation => (
                match z {
                    Some(z) => partial_correlation(x, y, z)
                        .map_err(|e| invalid(format!("{e:?}")))?
                        .map(|stat| stat.coefficient),
                    None => None,
                },
                4,
                z.is_none(),
            ),
        };
        let reading = match value {
            Some(value) => Reading::Value {
                value: MeasurementValue::Scalar(value),
            },
            None if missing_condition => {
                context.insert(
                    "missing_evidence".into(),
                    serde_json::json!("conditioning variable"),
                );
                Reading::InsufficientEvidence { have: 0, need: 1 }
            }
            None if sample_count < need => Reading::InsufficientEvidence {
                have: sample_count,
                need,
            },
            None => {
                context.insert(
                    "missing_evidence".into(),
                    serde_json::json!("nonzero conditioning and residual variance"),
                );
                Reading::InsufficientEvidence { have: 0, need: 1 }
            }
        };
        Ok(vec![token.emit(Measurement {
            sensor: self.descriptor.id.clone(),
            sensor_version: self.descriptor.version.clone(),
            reading,
            confidence: None,
            sample_count: Some(if missing_condition { 0 } else { sample_count }),
            context: MeasurementContext { values: context },
        })])
    }
}
