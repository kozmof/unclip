//! Validation of anonymous transformations derived from repeated state changes.

use crate::support::invalid;
use serde::{Deserialize, Serialize};
use unclip_domain::{CandidateProposal, DomainSnapshot, UnitId};
use unclip_epistemic::DerivedId;
use unclip_plugin::Result;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransformationPattern {
    pub matching: String,
    pub before: Vec<UnitId>,
    pub after: Vec<UnitId>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatePair {
    before: DerivedId,
    after: DerivedId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransformationEvidence {
    pattern: TransformationPattern,
    transitions: Vec<StatePair>,
    measurements: Vec<DerivedId>,
}

fn ordered_unique<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

pub(crate) fn validate(
    proposal: &CandidateProposal,
    domain: &DomainSnapshot,
) -> Result<TransformationPattern> {
    let evidence: TransformationEvidence =
        serde::Deserialize::deserialize(serde::de::value::MapDeserializer::new(
            proposal
                .value
                .iter()
                .map(|(key, value)| (key.as_str(), value)),
        ))
        .map_err(invalid)?;
    let pattern = &evidence.pattern;
    if pattern.matching != "exact_unit_state_transition"
        || pattern.before.is_empty()
        || pattern.after.is_empty()
        || !ordered_unique(&pattern.before)
        || !ordered_unique(&pattern.after)
        || pattern
            .before
            .iter()
            .chain(&pattern.after)
            .any(|unit| unit.0.trim().is_empty() || !domain.units.contains_key(unit))
        || pattern.before == pattern.after
    {
        return Err(invalid(
            "transformation application requires distinct nonempty ordered before and after sets of existing units",
        ));
    }
    if evidence.transitions.len() < 2
        || !ordered_unique(&evidence.transitions)
        || evidence.transitions.iter().any(|pair| {
            pair.before.0.trim().is_empty()
                || pair.after.0.trim().is_empty()
                || pair.before == pair.after
        })
        || evidence.measurements.is_empty()
        || !ordered_unique(&evidence.measurements)
        || evidence
            .measurements
            .iter()
            .any(|id| id.0.trim().is_empty())
    {
        return Err(invalid(
            "transformation evidence requires at least two ordered unique directed state pairs and ordered unique measurements",
        ));
    }
    Ok(evidence.pattern)
}
