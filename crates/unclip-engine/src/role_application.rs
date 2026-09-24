//! Validation of anonymous semantic roles derived from exact graph signatures.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use unclip_domain::{CandidateProposal, DomainSnapshot, UnitId};
use unclip_epistemic::DerivedId;
use unclip_plugin::{PluginError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RolePattern {
    pub matching: String,
    pub members: Vec<UnitId>,
    pub incoming: Vec<String>,
    pub outgoing: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleEvidence {
    pattern: RolePattern,
    structures: Vec<DerivedId>,
    measurements: Vec<DerivedId>,
}

fn invalid(message: impl Into<String>) -> PluginError {
    PluginError::Message(message.into())
}

fn ordered_unique<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

pub(super) fn validate(
    proposal: &CandidateProposal,
    domain: &DomainSnapshot,
) -> Result<RolePattern> {
    let evidence: RoleEvidence =
        serde_json::from_value(serde_json::Value::Object(proposal.value.clone()))
            .map_err(|error| invalid(error.to_string()))?;
    let pattern = &evidence.pattern;
    if pattern.matching != "exact_relation_kind_signature"
        || pattern.members.len() < 2
        || !ordered_unique(&pattern.members)
        || pattern
            .members
            .iter()
            .any(|member| member.0.trim().is_empty())
        || !ordered_unique(&pattern.incoming)
        || !ordered_unique(&pattern.outgoing)
        || pattern
            .incoming
            .iter()
            .chain(&pattern.outgoing)
            .any(|kind| kind.trim().is_empty())
        || pattern.incoming.is_empty() && pattern.outgoing.is_empty()
    {
        return Err(invalid(
            "semantic-role application requires at least two ordered members with one exact ordered relation-kind signature",
        ));
    }
    if evidence.structures.len() < 2
        || !ordered_unique(&evidence.structures)
        || evidence.structures.iter().any(|id| id.0.trim().is_empty())
        || evidence.measurements.is_empty()
        || !ordered_unique(&evidence.measurements)
        || evidence
            .measurements
            .iter()
            .any(|id| id.0.trim().is_empty())
    {
        return Err(invalid(
            "semantic-role evidence requires ordered unique calculated structures and measurements",
        ));
    }
    for member in &pattern.members {
        if !domain.units.contains_key(member) {
            return Err(invalid("semantic-role member does not exist in baseline"));
        }
        let incoming = domain
            .relations
            .values()
            .filter(|relation| &relation.target == member)
            .map(|relation| relation.kind.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let outgoing = domain
            .relations
            .values()
            .filter(|relation| &relation.source == member)
            .map(|relation| relation.kind.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if incoming != pattern.incoming || outgoing != pattern.outgoing {
            return Err(invalid(
                "semantic-role members do not share the exact recorded relation-kind signature",
            ));
        }
    }
    Ok(evidence.pattern)
}
