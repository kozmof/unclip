//! Temporal sensors requiring an explicit, validated observation sequence.

use std::{collections::BTreeMap, num::NonZeroUsize};

use semver::Version;
use serde::Deserialize;
use unclip_domain::UnitId;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    construct_rank_trajectories, detect_change_points, dynamic_time_warping, lagged_dependency,
    Measurement, MeasurementContext, MeasurementKind, MeasurementValue, ObservationSequence,
    RankPosition, RankTrajectory, Reading,
};
use unclip_plugin::{
    Applicability, Capability, EvidenceRequirement, MeasureCtx, PluginError, Result, Sensor,
    SensorDescriptor,
};

use crate::multi_observation::batch_states;

#[derive(Debug, Clone, Copy)]
pub enum TemporalStatistic {
    LaggedDependency,
    DynamicTimeWarping,
    ChangePoints,
}

pub struct TemporalSensor {
    descriptor: SensorDescriptor,
    statistic: TemporalStatistic,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LagParams {
    source: UnitId,
    target: UnitId,
    lag: NonZeroUsize,
    sequence: Option<ObservationSequence>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DtwParams {
    left: UnitId,
    right: UnitId,
    sequence: Option<ObservationSequence>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeParams {
    unit: UnitId,
    window: NonZeroUsize,
    minimum_shift: f64,
    sequence: Option<ObservationSequence>,
}

enum Parameters {
    Lag(LagParams),
    Dtw(DtwParams),
    Change(ChangeParams),
}
impl Parameters {
    fn sequence(&self) -> Option<&ObservationSequence> {
        match self {
            Self::Lag(p) => p.sequence.as_ref(),
            Self::Dtw(p) => p.sequence.as_ref(),
            Self::Change(p) => p.sequence.as_ref(),
        }
    }
}

impl TemporalSensor {
    pub fn new(statistic: TemporalStatistic) -> Self {
        let (id, samples, kinds, schema): (_, _, &'static [MeasurementKind], _) = match statistic {
            TemporalStatistic::LaggedDependency => (
                "sensor.lagged-dependency",
                3,
                &[MeasurementKind::Scalar],
                LAG_SCHEMA,
            ),
            TemporalStatistic::DynamicTimeWarping => {
                ("sensor.dtw", 1, &[MeasurementKind::Scalar], DTW_SCHEMA)
            }
            TemporalStatistic::ChangePoints => (
                "sensor.change-points",
                2,
                &[MeasurementKind::Events],
                CHANGE_SCHEMA,
            ),
        };
        let evidence: &'static [EvidenceRequirement] = match samples {
            1 => &[
                EvidenceRequirement::ExplicitOrder,
                EvidenceRequirement::MinSamples(1),
            ],
            2 => &[
                EvidenceRequirement::ExplicitOrder,
                EvidenceRequirement::MinSamples(2),
            ],
            _ => &[
                EvidenceRequirement::ExplicitOrder,
                EvidenceRequirement::MinSamples(3),
            ],
        };
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new(id),
                version: Version::new(0, 1, 0),
                applicability: &[
                    Capability::MultiObservation,
                    Capability::RankingValue,
                    Capability::Alignment,
                    Capability::Ordered,
                ],
                evidence,
                produces: kinds,
                params_schema: schema,
            },
            statistic,
        }
    }
}

const LAG_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["source","target","lag"],"properties":{"source":{"type":"string","minLength":1},"target":{"type":"string","minLength":1},"lag":{"type":"integer","minimum":1},"sequence":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["observation","position"],"properties":{"observation":{"type":"string"},"position":{"type":"integer"}}}}}}"#;
const DTW_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["left","right"],"properties":{"left":{"type":"string","minLength":1},"right":{"type":"string","minLength":1},"sequence":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["observation","position"],"properties":{"observation":{"type":"string"},"position":{"type":"integer"}}}}}}"#;
const CHANGE_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["unit","window","minimum_shift"],"properties":{"unit":{"type":"string","minLength":1},"window":{"type":"integer","minimum":1},"minimum_shift":{"type":"number","exclusiveMinimum":0},"sequence":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["observation","position"],"properties":{"observation":{"type":"string"},"position":{"type":"integer"}}}}}}"#;

fn invalid(error: impl std::fmt::Display) -> PluginError {
    PluginError::Message(error.to_string())
}
fn rank<'a>(ranks: &'a [RankTrajectory], unit: &UnitId) -> Result<&'a RankTrajectory> {
    ranks.iter().find(|r| r.unit == *unit).ok_or_else(|| {
        invalid(format!(
            "selected unit {} is not in the measurement frame",
            unit.0
        ))
    })
}
fn known(position: RankPosition) -> bool {
    matches!(position, RankPosition::Ranked { .. })
}

impl Sensor for TemporalSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }
    fn applies_to(&self, ctx: &MeasureCtx<'_>) -> Applicability {
        if ctx.frame().axes.is_empty() {
            Applicability::NotApplicable {
                reason: "temporal rank sensors require frame axes".into(),
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
        let params = match self.statistic {
            TemporalStatistic::LaggedDependency => {
                Parameters::Lag(serde_json::from_value(ctx.params().clone()).map_err(invalid)?)
            }
            TemporalStatistic::DynamicTimeWarping => {
                Parameters::Dtw(serde_json::from_value(ctx.params().clone()).map_err(invalid)?)
            }
            TemporalStatistic::ChangePoints => {
                Parameters::Change(serde_json::from_value(ctx.params().clone()).map_err(invalid)?)
            }
        };
        let mut context = BTreeMap::new();
        let Some(sequence) = params.sequence() else {
            context.insert(
                "missing_evidence".into(),
                serde_json::json!("explicit observation sequence"),
            );
            return Ok(vec![token.emit(self.measurement(
                Reading::InsufficientEvidence { have: 0, need: 1 },
                0,
                context,
            ))]);
        };
        let (units, states) = batch_states(ctx)?;
        let mut by_id = states.into_iter().collect::<BTreeMap<_, _>>();
        if sequence.observations().len() != by_id.len() {
            return Err(invalid(
                "sequence must cover every selected observation exactly once",
            ));
        }
        let mut states = Vec::new();
        for entry in sequence.observations() {
            let state = by_id.remove(&entry.observation).ok_or_else(|| {
                invalid(format!(
                    "sequence observation {} is not selected",
                    entry.observation.0
                ))
            })?;
            states.push((entry.observation.clone(), state));
        }
        let ranks = construct_rank_trajectories(&units, &states);
        context.insert("sequence".into(), serde_json::json!(sequence));
        let (reading, count) = match &params {
            Parameters::Lag(p) => {
                let x = rank(&ranks, &p.source)?;
                let y = rank(&ranks, &p.target)?;
                let count = x
                    .samples
                    .iter()
                    .zip(y.samples.iter().skip(p.lag.get()))
                    .filter(|(x, y)| known(x.position) && known(y.position))
                    .count();
                context.insert("source".into(), serde_json::json!(p.source));
                context.insert("target".into(), serde_json::json!(p.target));
                context.insert("lag_steps".into(), serde_json::json!(p.lag));
                context.insert(
                    "evidence".into(),
                    serde_json::json!("directional association, not causality"),
                );
                let reading = match lagged_dependency(sequence, x, y, p.lag).map_err(invalid)? {
                    Some(value) => Reading::Value {
                        value: MeasurementValue::Scalar(value.coefficient),
                    },
                    None if count < 2 => Reading::InsufficientEvidence {
                        have: count,
                        need: 2,
                    },
                    None => {
                        context.insert(
                            "missing_evidence".into(),
                            serde_json::json!("nonzero lagged variance"),
                        );
                        Reading::InsufficientEvidence { have: 0, need: 1 }
                    }
                };
                (reading, count)
            }
            Parameters::Dtw(p) => {
                let x = rank(&ranks, &p.left)?;
                let y = rank(&ranks, &p.right)?;
                let count = x
                    .samples
                    .iter()
                    .zip(&y.samples)
                    .filter(|(x, y)| known(x.position) && known(y.position))
                    .count();
                context.insert("pair".into(), serde_json::json!([p.left, p.right]));
                context.insert(
                    "distance".into(),
                    serde_json::json!("unnormalized absolute rank cost"),
                );
                let reading =
                    match dynamic_time_warping(sequence, x, sequence, y).map_err(invalid)? {
                        Some(value) => Reading::Value {
                            value: MeasurementValue::Scalar(value),
                        },
                        None => Reading::InsufficientEvidence {
                            have: count,
                            need: states.len().max(1),
                        },
                    };
                (reading, count)
            }
            Parameters::Change(p) => {
                let x = rank(&ranks, &p.unit)?;
                let need = p
                    .window
                    .get()
                    .checked_mul(2)
                    .ok_or_else(|| invalid("window is too large"))?;
                let count = x
                    .samples
                    .iter()
                    .filter(|sample| known(sample.position))
                    .count();
                context.insert("unit".into(), serde_json::json!(p.unit));
                context.insert("window".into(), serde_json::json!(p.window));
                context.insert("minimum_shift".into(), serde_json::json!(p.minimum_shift));
                let reading = match detect_change_points(sequence, x, p.window, p.minimum_shift)
                    .map_err(invalid)?
                {
                    Some(result) => {
                        context.insert(
                            "evaluated_boundaries".into(),
                            serde_json::json!(result.evaluated_boundaries),
                        );
                        Reading::Value {
                            value: MeasurementValue::Events(
                                result
                                    .events
                                    .into_iter()
                                    .map(serde_json::to_value)
                                    .collect::<std::result::Result<_, _>>()
                                    .map_err(invalid)?,
                            ),
                        }
                    }
                    None => {
                        let (mut longest, mut current) = (0, 0);
                        for sample in &x.samples {
                            current = if known(sample.position) {
                                current + 1
                            } else {
                                0
                            };
                            longest = longest.max(current);
                        }
                        context.insert(
                            "missing_evidence".into(),
                            serde_json::json!("two adjacent complete windows"),
                        );
                        Reading::InsufficientEvidence {
                            have: longest,
                            need,
                        }
                    }
                };
                (reading, count)
            }
        };
        Ok(vec![token.emit(self.measurement(reading, count, context))])
    }
}
impl TemporalSensor {
    fn measurement(
        &self,
        reading: Reading,
        count: usize,
        values: BTreeMap<String, serde_json::Value>,
    ) -> Measurement {
        Measurement {
            sensor: self.descriptor.id.clone(),
            sensor_version: self.descriptor.version.clone(),
            reading,
            confidence: None,
            sample_count: Some(count),
            context: MeasurementContext { values },
        }
    }
}
