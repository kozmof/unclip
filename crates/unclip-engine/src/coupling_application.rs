//! Validate pairwise coupling proposal evidence before temporary application.
use serde::Deserialize;
use unclip_domain::{CandidateProposal, DomainSnapshot, UnitId};
use unclip_epistemic::{DerivedId, PluginId};
use unclip_measure::{MatrixCell, MeasurementContext, PairwiseMetric};
use unclip_plugin::{PluginError, Result};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pattern {
    matching: String,
    metric: PairwiseMetric,
    units: Vec<UnitId>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    measurement: DerivedId,
    sensor: PluginId,
    sensor_version: semver::Version,
    context: MeasurementContext,
    cell: MatrixCell,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    threshold: f64,
    minimum_samples: usize,
}
fn invalid(s: impl ToString) -> PluginError {
    PluginError::Message(s.to_string())
}
pub(super) fn validate(proposal: &CandidateProposal, domain: &DomainSnapshot) -> Result<()> {
    let field = |key| {
        proposal
            .value
            .get(key)
            .cloned()
            .ok_or_else(|| invalid(format!("coupling candidate requires {key}")))
    };
    let pattern: Pattern = serde_json::from_value(field("pattern")?).map_err(invalid)?;
    if pattern.matching != "thresholded_pairwise_association"
        || pattern.units.len() != 2
        || pattern.units[0] >= pattern.units[1]
        || pattern
            .units
            .iter()
            .any(|id| !domain.units.contains_key(id))
    {
        return Err(invalid(
            "pairwise coupling requires two ordered distinct existing baseline units",
        ));
    }
    let evidence: Evidence = serde_json::from_value(field("evidence")?).map_err(invalid)?;
    let selection: Selection = serde_json::from_value(field("selection")?).map_err(invalid)?;
    if evidence.measurement.0.is_empty()
        || evidence.sensor.0.is_empty()
        || proposal.value.get("causal_claim") != Some(&serde_json::Value::Bool(false))
    {
        return Err(invalid(
            "coupling requires source identities and an explicit non-causal claim",
        ));
    }
    // Retained metadata is parsed as its declared types; no metric is inferred from sensor names.
    let _ = (evidence.sensor_version, evidence.context);
    let MatrixCell::Value {
        value,
        sample_count,
    } = evidence.cell
    else {
        return Err(invalid("coupling requires measured cell evidence"));
    };
    let valid_range = |value: f64| {
        value.is_finite()
            && match pattern.metric {
                PairwiseMetric::Spearman | PairwiseMetric::Kendall => (-1.0..=1.0).contains(&value),
                _ => value >= 0.0,
            }
    };
    if !valid_range(value)
        || !valid_range(selection.threshold)
        || selection.minimum_samples < 2
        || sample_count < selection.minimum_samples
    {
        return Err(invalid(
            "coupling metric range or sample evidence is invalid",
        ));
    }
    let qualifies = match pattern.metric {
        PairwiseMetric::RelativeRankVariance => value <= selection.threshold,
        _ => value >= selection.threshold,
    };
    if !qualifies {
        return Err(invalid(
            "coupling cell does not meet its recorded selection threshold",
        ));
    }
    Ok(())
}
