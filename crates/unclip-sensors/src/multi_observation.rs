//! Batch rank calculations over tracked observation, alignment, and ranking inputs.

use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    construct_rank_trajectories, construct_relative_rank_trajectories, pairwise_matrix,
    Measurement, MeasurementContext, MeasurementKind, MeasurementValue, PairwiseMetric,
    RankedState, Reading,
};
use unclip_plugin::{
    Applicability, Capability, EvidenceRequirement, MeasureCtx, PluginError, Result, Sensor,
    SensorDescriptor,
};

use crate::permutation::{alignment_index, ranked_state};

/// One independent batch statistic; selection never combines metrics into a score.
pub struct MultiObservationSensor {
    descriptor: SensorDescriptor,
    metric: Option<PairwiseMetric>,
}

impl MultiObservationSensor {
    pub fn trajectories() -> Self {
        Self::new(
            "sensor.trajectories",
            None,
            &[EvidenceRequirement::MinSamples(1)],
            &[MeasurementKind::Structured],
        )
    }

    pub fn matrix(metric: PairwiseMetric) -> Self {
        let id = match metric {
            PairwiseMetric::Spearman => "sensor.spearman",
            PairwiseMetric::Kendall => "sensor.kendall-association",
            PairwiseMetric::RelativeRankVariance => "sensor.relative-rank-variance",
            PairwiseMetric::MutualInformation => "sensor.mutual-information",
        };
        Self::new(
            id,
            Some(metric),
            &[EvidenceRequirement::MinSamples(2)],
            &[MeasurementKind::Matrix],
        )
    }

    fn new(
        id: &str,
        metric: Option<PairwiseMetric>,
        evidence: &'static [EvidenceRequirement],
        produces: &'static [MeasurementKind],
    ) -> Self {
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
                produces,
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
            metric,
        }
    }
}

impl Sensor for MultiObservationSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn applies_to(&self, ctx: &MeasureCtx<'_>) -> Applicability {
        if ctx.frame().axes.is_empty() {
            Applicability::NotApplicable {
                reason: "rank trajectories require frame axes".into(),
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
        if !ctx
            .params()
            .as_object()
            .is_some_and(|params| params.is_empty())
        {
            return Err(PluginError::Message(
                "batch rank sensors accept only an empty parameter object".into(),
            ));
        }
        let (frame_units, states) = batch_states(ctx)?;
        let ranks = construct_rank_trajectories(&frame_units, &states);
        let reading = if let Some(metric) = self.metric {
            let matrix = pairwise_matrix(&ranks, metric)
                .map_err(|error| PluginError::Message(error.to_string()))?;
            Reading::Value {
                value: MeasurementValue::PairwiseMatrix(matrix),
            }
        } else {
            Reading::Value {
                value: MeasurementValue::Structured(serde_json::json!({
                    "rank_trajectories": ranks,
                    "relative_rank_trajectories": construct_relative_rank_trajectories(&frame_units, &states),
                })),
            }
        };
        Ok(vec![token.emit(Measurement {
            sensor: self.descriptor.id.clone(),
            sensor_version: self.descriptor.version.clone(),
            reading,
            confidence: None,
            sample_count: Some(states.len()),
            context: MeasurementContext {
                values: BTreeMap::from([(
                    "observations".into(),
                    serde_json::json!(states.iter().map(|(id, _)| id).collect::<Vec<_>>()),
                )]),
            },
        })])
    }
}

type BatchStates = (
    Vec<unclip_domain::UnitId>,
    Vec<(unclip_observe::ObservationId, RankedState)>,
);

pub(crate) fn batch_states(ctx: &MeasureCtx<'_>) -> Result<BatchStates> {
    let frame_units = ctx
        .frame()
        .axes
        .iter()
        .map(|axis| axis.unit.clone())
        .collect::<BTreeSet<_>>();
    let alignments = alignment_index(ctx, &frame_units);
    let mut rankings = BTreeMap::new();
    for tracked in ctx.rankings() {
        let ranking = ctx.read(tracked);
        if rankings
            .insert(ranking.observation.clone(), ranking)
            .is_some()
        {
            return Err(PluginError::Message(format!(
                "multiple rankings for observation {} require explicit selection",
                ranking.observation.0
            )));
        }
    }
    let mut observations = BTreeSet::new();
    for tracked in ctx.observations() {
        let observation = ctx.read(tracked);
        if !observations.insert(observation.id.clone()) {
            return Err(PluginError::Message(format!(
                "duplicate observation {}",
                observation.id.0
            )));
        }
    }
    if rankings
        .keys()
        .chain(alignments.keys())
        .any(|id| !observations.contains(id))
    {
        return Err(PluginError::Message(
            "rankings and alignments must belong to the selected observations".into(),
        ));
    }
    // Non-temporal batches use stable observation-ID order. No temporal
    // ordering is inferred from IDs or timestamps.
    let states = observations
        .iter()
        .map(|id| {
            let state = match (rankings.get(id), alignments.get(id)) {
                (Some(ranking), Some(alignment)) => ranked_state(ranking, alignment, &frame_units),
                _ => RankedState {
                    tiers: vec![],
                    unknown: vec![],
                    unresolved: vec![],
                },
            };
            (id.clone(), state)
        })
        .collect::<Vec<_>>();
    let frame_units = frame_units.into_iter().collect::<Vec<_>>();
    Ok((frame_units, states))
}
