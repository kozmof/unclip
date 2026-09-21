//! Validation of recurring observed two-edge paths before temporary application.
use serde::Deserialize;
use std::collections::BTreeSet;
use unclip_domain::CandidateProposal;
use unclip_epistemic::DerivedId;
use unclip_plugin::{PluginError, Result};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    position: usize,
    observed_label: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edge {
    source: usize,
    target: usize,
    kind: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pattern {
    matching: String,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgeEvidence {
    relation: String,
    uncertainty: Option<f64>,
    measurements: Vec<DerivedId>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Example {
    observation: String,
    units: Vec<String>,
    edges: Vec<EdgeEvidence>,
}
fn invalid(s: impl ToString) -> PluginError {
    PluginError::Message(s.to_string())
}
pub(super) fn validate(proposal: &CandidateProposal) -> Result<()> {
    let field = |key| {
        proposal
            .value
            .get(key)
            .cloned()
            .ok_or_else(|| invalid(format!("motif candidate requires {key}")))
    };
    let pattern: Pattern = serde_json::from_value(field("pattern")?).map_err(invalid)?;
    if pattern.matching != "exact_directed_two_edge_path"
        || pattern.nodes.len() != 3
        || pattern.edges.len() != 2
        || pattern
            .nodes
            .iter()
            .enumerate()
            .any(|(i, n)| n.position != i || n.observed_label.trim().is_empty())
        || pattern
            .edges
            .iter()
            .enumerate()
            .any(|(i, e)| e.source != i || e.target != i + 1 || e.kind.trim().is_empty())
    {
        return Err(invalid(
            "motif application requires an ordered three-node directed two-edge path",
        ));
    }
    let observations: Vec<String> =
        serde_json::from_value(field("observations")?).map_err(invalid)?;
    let count: usize = serde_json::from_value(field("observation_count")?).map_err(invalid)?;
    let unique = observations.iter().collect::<BTreeSet<_>>();
    if count < 2
        || count != observations.len()
        || unique.len() != count
        || observations.iter().any(|id| id.is_empty())
    {
        return Err(invalid(
            "motif support requires at least two distinct nonempty observation identities",
        ));
    }
    let examples: Vec<Example> = serde_json::from_value(field("examples")?).map_err(invalid)?;
    let mut supported = BTreeSet::new();
    for example in &examples {
        if !unique.contains(&example.observation)
            || example.units.len() != 3
            || example.units.iter().collect::<BTreeSet<_>>().len() != 3
            || example.units.iter().any(|id| id.is_empty())
            || example.edges.len() != 2
            || example.edges[0].relation == example.edges[1].relation
        {
            return Err(invalid("motif examples must identify supported observations, three distinct units, and two distinct edges"));
        }
        supported.insert(&example.observation);
        for edge in &example.edges {
            if edge.relation.is_empty()
                || edge
                    .uncertainty
                    .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
                || edge.measurements.is_empty()
                || edge.measurements.iter().any(|id| id.0.is_empty())
                || edge.measurements.iter().collect::<BTreeSet<_>>().len()
                    != edge.measurements.len()
            {
                return Err(invalid("motif edges require valid uncertainty and unique source measurement identities"));
            }
        }
    }
    if supported != unique {
        return Err(invalid(
            "every motif support observation requires an example",
        ));
    }
    Ok(())
}
