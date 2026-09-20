//! Pairwise coupling hypotheses from explicitly selected matrix metrics.
use serde::Deserialize;
use std::{collections::BTreeSet, num::NonZeroUsize};
use unclip_domain::{CandidateKind, CandidateProposal};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{MatrixCell, MeasurementValue, PairwiseMetric, Reading};
use unclip_plugin::{CandidateCtx, CandidateGenerator, PluginDescriptor, PluginError, Result};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    metric: PairwiseMetric,
    threshold: f64,
    minimum_samples: NonZeroUsize,
}

pub struct PairwiseCouplingGenerator {
    descriptor: PluginDescriptor,
}
impl Default for PairwiseCouplingGenerator {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("generate.pairwise-coupling"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["metric","threshold","minimum_samples"],"properties":{"metric":{"enum":["relative_rank_variance","spearman","kendall","mutual_information"]},"threshold":{"type":"number"},"minimum_samples":{"type":"integer","minimum":2}}}"#,
            },
        }
    }
}
fn invalid(message: impl ToString) -> PluginError {
    PluginError::Message(message.to_string())
}
impl CandidateGenerator for PairwiseCouplingGenerator {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }
    fn generate(
        &self,
        ctx: &CandidateCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<CandidateProposal>>> {
        let params: Parameters = serde_json::from_value(ctx.params().clone()).map_err(invalid)?;
        let valid_threshold = match params.metric {
            PairwiseMetric::Spearman | PairwiseMetric::Kendall => {
                (-1.0..=1.0).contains(&params.threshold)
            }
            PairwiseMetric::RelativeRankVariance | PairwiseMetric::MutualInformation => {
                params.threshold >= 0.0
            }
        };
        if ctx.domain_version_id().is_empty()
            || params.minimum_samples.get() < 2
            || !params.threshold.is_finite()
            || !valid_threshold
        {
            return Err(invalid("pairwise coupling requires a domain version, at least two samples, and a threshold in the metric's range"));
        }
        let mut ids = BTreeSet::new();
        let mut selected = ctx.measurements().iter().collect::<Vec<_>>();
        selected.sort_by_key(|input| input.id());
        // Read all selected measurements before emission: candidate IDs and
        // selection decisions depend on this complete, explicitly supplied set.
        let mut measurements = Vec::new();
        for input in selected {
            if !ids.insert(input.id()) {
                return Err(invalid("duplicate discovery measurement"));
            }
            measurements.push((input.id(), ctx.read(input)));
        }
        let mut proposals = Vec::new();
        for (id, measurement) in measurements {
            let Reading::Value {
                value: MeasurementValue::PairwiseMatrix(matrix),
            } = &measurement.reading
            else {
                continue;
            };
            if matrix.metric() != params.metric {
                continue;
            }
            for (left, row) in matrix.cells().iter().enumerate() {
                for (right, cell) in row.iter().enumerate().skip(left + 1) {
                    let MatrixCell::Value {
                        value,
                        sample_count,
                    } = cell
                    else {
                        continue;
                    };
                    if *sample_count < params.minimum_samples.get() {
                        continue;
                    }
                    let qualifies = match params.metric {
                        PairwiseMetric::RelativeRankVariance => *value <= params.threshold,
                        _ => *value >= params.threshold,
                    };
                    if !qualifies {
                        continue;
                    }
                    proposals.push(token.emit(CandidateProposal {domain_version_id:ctx.domain_version_id().into(),kind:CandidateKind::DynamicCoupling,
                        value:serde_json::json!({"pattern":{"matching":"thresholded_pairwise_association","metric":matrix.metric(),"units":[matrix.units()[left],matrix.units()[right]]},"evidence":{"measurement":id,"sensor":measurement.sensor,"sensor_version":measurement.sensor_version,"context":measurement.context,"cell":cell},"selection":{"threshold":params.threshold,"minimum_samples":params.minimum_samples},"causal_claim":false}).as_object().expect("object").clone()}));
                }
            }
        }
        Ok(proposals)
    }
}
