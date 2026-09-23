//! Zero-association baseline diagnostic for supported dynamic-coupling proposals.

use serde::Deserialize;
use unclip_domain::CandidateKind;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{MatrixCell, MeasurementValue, PairwiseMetric, Reading};
use unclip_plugin::{NullCtx, NullModel, PluginDescriptor, PluginError, Result};

pub struct CouplingZeroNull {
    descriptor: PluginDescriptor,
}

impl Default for CouplingZeroNull {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("null.coupling-zero"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["absolute_tolerance"],"properties":{"absolute_tolerance":{"type":"number","minimum":0}}}"#,
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    absolute_tolerance: f64,
}

fn invalid(message: impl Into<String>) -> PluginError {
    PluginError::Message(message.into())
}

impl NullModel for CouplingZeroNull {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    fn evaluate(&self, ctx: &NullCtx<'_>, token: CalculationToken) -> Result<Calculated<Reading>> {
        let params: Parameters = serde_json::from_value(ctx.params().clone())
            .map_err(|error| invalid(error.to_string()))?;
        if !params.absolute_tolerance.is_finite() || params.absolute_tolerance < 0.0 {
            return Err(invalid(
                "coupling zero tolerance must be finite and nonnegative",
            ));
        }
        let candidate = ctx.candidate();
        let matching = candidate
            .value
            .get("pattern")
            .and_then(|pattern| pattern.get("matching"))
            .and_then(serde_json::Value::as_str);
        if candidate.kind != CandidateKind::DynamicCoupling
            || !matches!(
                matching,
                Some("thresholded_pairwise_association" | "lagged_directional_association")
            )
        {
            return Ok(token.emit(Reading::NotApplicable {
                reason: "requires a supported dynamic-coupling proposal".into(),
            }));
        }
        let Some(domain) = ctx.domain() else {
            return Ok(token.emit(Reading::InsufficientEvidence { have: 0, need: 1 }));
        };
        super::coupling_application::validate(candidate, domain)?;

        let (observed, metric) = match matching.expect("validated matching") {
            "thresholded_pairwise_association" => {
                let metric: PairwiseMetric =
                    serde_json::from_value(candidate.value["pattern"]["metric"].clone())
                        .map_err(|error| invalid(error.to_string()))?;
                if metric == PairwiseMetric::RelativeRankVariance {
                    return Ok(token.emit(Reading::NotApplicable {
                        reason:
                            "relative-rank variance has no universal zero-independence baseline"
                                .into(),
                    }));
                }
                let cell: MatrixCell =
                    serde_json::from_value(candidate.value["evidence"]["cell"].clone())
                        .map_err(|error| invalid(error.to_string()))?;
                let MatrixCell::Value { value, .. } = cell else {
                    return Err(invalid(
                        "coupling zero null requires measured cell evidence",
                    ));
                };
                (
                    value,
                    serde_json::to_value(metric).expect("metric serialization"),
                )
            }
            "lagged_directional_association" => {
                let observed = candidate.value["evidence"]["coefficient"]
                    .as_f64()
                    .ok_or_else(|| invalid("lagged coupling requires a numeric coefficient"))?;
                (
                    observed,
                    serde_json::Value::String("lagged_coefficient".into()),
                )
            }
            _ => unreachable!("matching checked above"),
        };
        let distance = observed.abs();
        if !distance.is_finite() {
            return Err(invalid("coupling distance from zero must be finite"));
        }
        Ok(token.emit(Reading::Value {
            value: MeasurementValue::Structured(serde_json::json!({
                "model": "zero_association_baseline",
                "matching": matching,
                "metric": metric,
                "observed_value": observed,
                "baseline_value": 0.0,
                "absolute_distance": distance,
                "absolute_tolerance": params.absolute_tolerance,
                "within_tolerance": distance <= params.absolute_tolerance,
                "scope": "distance from zero association only; this is not a significance, exchangeability, or causal test",
                "causal_claim": false,
                "decision": "no automatic candidate acceptance or rejection"
            })),
        }))
    }
}
