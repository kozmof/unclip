//! Exact-identity distances for simple directed typed graphs.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{Delta, MeasurementKind, MeasurementValue, Reading};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, PluginError, Result};
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectedGraphEdge {
    pub source: String,
    pub target: String,
    pub kind: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedDirectedGraph {
    pub nodes: Vec<String>,
    pub edges: Vec<DirectedGraphEdge>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphComparison {
    Value {
        node_distance: usize,
        edge_distance: usize,
        nodes_added: Vec<String>,
        nodes_removed: Vec<String>,
        edges_added: Vec<DirectedGraphEdge>,
        edges_removed: Vec<DirectedGraphEdge>,
        before: NamedDirectedGraph,
        after: NamedDirectedGraph,
    },
    Unavailable {
        before: Reading,
        after: Reading,
    },
    NotApplicable {
        reason: String,
    },
}
pub struct GraphIdentityComparator {
    descriptor: ComparatorDescriptor,
}
impl Default for GraphIdentityComparator {
    fn default() -> Self {
        Self {
            descriptor: ComparatorDescriptor {
                id: PluginId::new("compare.graph-identity"),
                version: semver::Version::new(0, 1, 0),
                supports: &[MeasurementKind::Graph],
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {}
fn invalid(s: impl ToString) -> PluginError {
    PluginError::Message(s.to_string())
}
fn graph(value: &serde_json::Value) -> Result<NamedDirectedGraph> {
    let mut graph: NamedDirectedGraph = serde_json::from_value(value.clone()).map_err(invalid)?;
    let nodes = graph.nodes.iter().collect::<BTreeSet<_>>();
    let edges = graph.edges.iter().collect::<BTreeSet<_>>();
    if nodes.len() != graph.nodes.len() || nodes.iter().any(|n| n.trim().is_empty()) {
        return Err(invalid("graph nodes must have unique nonempty identities"));
    }
    if edges.len() != graph.edges.len()
        || edges.iter().any(|e| {
            e.kind.trim().is_empty() || !nodes.contains(&e.source) || !nodes.contains(&e.target)
        })
    {
        return Err(invalid(
            "graph edges must have unique source/target/kind triples and existing endpoints",
        ));
    }
    graph.nodes.sort();
    graph.edges.sort();
    Ok(graph)
}
impl Comparator for GraphIdentityComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }
    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let _: Parameters = serde_json::from_value(ctx.params().clone()).map_err(invalid)?;
        let before = ctx.before();
        let after = ctx.after();
        if before.sensor != after.sensor
            || before.sensor_version != after.sensor_version
            || before.context != after.context
        {
            return Err(invalid(
                "graph comparison requires the same sensor, version, and measurement context",
            ));
        }
        let mut parsed = Vec::new();
        for reading in [&before.reading, &after.reading] {
            parsed.push(
                if let Reading::Value {
                    value: MeasurementValue::Graph(value),
                } = reading
                {
                    Some(graph(value)?)
                } else {
                    None
                },
            );
        }
        let unsupported = |reading: &Reading| matches!(reading,Reading::Value {value} if !matches!(value,MeasurementValue::Graph(_)));
        let result = if unsupported(&before.reading) || unsupported(&after.reading) {
            GraphComparison::NotApplicable {
                reason: "requires explicit named directed graphs".into(),
            }
        } else if let (Some(a), Some(b)) = (&parsed[0], &parsed[1]) {
            let an = a.nodes.iter().cloned().collect::<BTreeSet<_>>();
            let bn = b.nodes.iter().cloned().collect::<BTreeSet<_>>();
            let ae = a.edges.iter().cloned().collect::<BTreeSet<_>>();
            let be = b.edges.iter().cloned().collect::<BTreeSet<_>>();
            let nodes_added = bn.difference(&an).cloned().collect::<Vec<_>>();
            let nodes_removed = an.difference(&bn).cloned().collect::<Vec<_>>();
            let edges_added = be.difference(&ae).cloned().collect::<Vec<_>>();
            let edges_removed = ae.difference(&be).cloned().collect::<Vec<_>>();
            GraphComparison::Value {
                node_distance: nodes_added
                    .len()
                    .checked_add(nodes_removed.len())
                    .ok_or_else(|| invalid("node distance overflow"))?,
                edge_distance: edges_added
                    .len()
                    .checked_add(edges_removed.len())
                    .ok_or_else(|| invalid("edge distance overflow"))?,
                nodes_added,
                nodes_removed,
                edges_added,
                edges_removed,
                before: a.clone(),
                after: b.clone(),
            }
        } else {
            GraphComparison::Unavailable {
                before: before.reading.clone(),
                after: after.reading.clone(),
            }
        };
        Ok(token.emit(Delta {
            comparator: self.descriptor.id.clone(),
            value: MeasurementValue::Structured(serde_json::to_value(result).map_err(invalid)?),
        }))
    }
}
