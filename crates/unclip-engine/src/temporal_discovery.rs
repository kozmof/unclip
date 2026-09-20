//! Temporal coupling hypotheses from explicit-order lagged association evidence.
use serde::Deserialize;
use std::{collections::BTreeSet, num::NonZeroUsize};
use unclip_domain::{CandidateKind, CandidateProposal, UnitId};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{MeasurementValue, ObservationSequence, Reading};
use unclip_plugin::{CandidateCtx, CandidateGenerator, PluginDescriptor, PluginError, Result};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    threshold: f64,
    minimum_samples: NonZeroUsize,
}
#[derive(Deserialize)]
struct LagEvidence {
    source: UnitId,
    target: UnitId,
    lag_steps: NonZeroUsize,
    sequence: ObservationSequence,
}

pub struct TemporalCouplingGenerator {
    descriptor: PluginDescriptor,
}
impl Default for TemporalCouplingGenerator {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("generate.temporal-coupling"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["threshold","minimum_samples"],"properties":{"threshold":{"type":"number","minimum":-1,"maximum":1},"minimum_samples":{"type":"integer","minimum":2}}}"#,
            },
        }
    }
}
fn invalid(message: impl ToString) -> PluginError {
    PluginError::Message(message.to_string())
}
impl CandidateGenerator for TemporalCouplingGenerator {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }
    fn generate(
        &self,
        ctx: &CandidateCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<CandidateProposal>>> {
        let params: Parameters = serde_json::from_value(ctx.params().clone()).map_err(invalid)?;
        if ctx.domain_version_id().is_empty()
            || params.minimum_samples.get() < 2
            || !params.threshold.is_finite()
            || !(-1.0..=1.0).contains(&params.threshold)
        {
            return Err(invalid("temporal coupling requires a domain version, at least two samples, and a threshold in [-1, 1]"));
        }
        let mut selected = ctx.measurements().iter().collect::<Vec<_>>();
        selected.sort_by_key(|input| input.id());
        let mut ids = BTreeSet::new();
        let mut measurements = Vec::new();
        for input in selected {
            if !ids.insert(input.id()) {
                return Err(invalid("duplicate discovery measurement"));
            }
            measurements.push((input.id(), ctx.read(input)));
        }
        let mut proposals = Vec::new();
        for (id, measurement) in measurements {
            if measurement.sensor.0 != "sensor.lagged-dependency" {
                continue;
            }
            let Reading::Value { value } = &measurement.reading else {
                continue;
            };
            let MeasurementValue::Scalar(coefficient) = value else {
                return Err(invalid("lagged association must be a scalar reading"));
            };
            if !coefficient.is_finite() || !(-1.0..=1.0).contains(coefficient) {
                return Err(invalid(
                    "lagged association coefficient must be finite and in [-1, 1]",
                ));
            }
            let evidence: LagEvidence =
                serde_json::from_value(serde_json::json!(measurement.context.values))
                    .map_err(invalid)?;
            if evidence.source.0.is_empty()
                || evidence.target.0.is_empty()
                || evidence
                    .sequence
                    .observations()
                    .iter()
                    .any(|entry| entry.observation.0.is_empty())
            {
                return Err(invalid("temporal evidence identities must not be empty"));
            }
            let count = measurement
                .sample_count
                .ok_or_else(|| invalid("lagged association requires a recorded sample count"))?;
            let available = evidence
                .sequence
                .observations()
                .len()
                .saturating_sub(evidence.lag_steps.get());
            if count < 2 || count > available {
                return Err(invalid(
                    "lagged sample count is inconsistent with the explicit sequence",
                ));
            }
            if count < params.minimum_samples.get() || *coefficient < params.threshold {
                continue;
            }
            proposals.push(token.emit(CandidateProposal {domain_version_id:ctx.domain_version_id().into(),kind:CandidateKind::DynamicCoupling,
                value:serde_json::json!({"pattern":{"matching":"lagged_directional_association","source":evidence.source,"target":evidence.target,"lag_steps":evidence.lag_steps,"sequence":evidence.sequence},"evidence":{"measurement":id,"sensor":measurement.sensor,"sensor_version":measurement.sensor_version,"coefficient":coefficient,"sample_count":count,"context":measurement.context},"selection":{"threshold":params.threshold,"minimum_samples":params.minimum_samples},"causal_claim":false}).as_object().expect("object").clone()}));
        }
        Ok(proposals)
    }
}
