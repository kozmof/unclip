//! Exact-identity distances for simple directed typed graphs.
use crate::support::{invalid, invalid_params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{Delta, MeasurementKind, MeasurementValue, Reading};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, Result};
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
/// The write side of [`GraphComparison`], borrowing what it serializes.
///
/// Both parsed graphs stay in the caller's hands, so the `Value` arm reports
/// the two graphs and the four difference lists without copying a node or edge
/// out of either one; the `Unavailable` arm borrows the readings for the same
/// reason the other comparators do. The round-trip test below pins this shape
/// to the owning one.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum GraphComparisonRef<'a> {
    Value {
        node_distance: usize,
        edge_distance: usize,
        nodes_added: Vec<&'a String>,
        nodes_removed: Vec<&'a String>,
        edges_added: Vec<&'a DirectedGraphEdge>,
        edges_removed: Vec<&'a DirectedGraphEdge>,
        before: &'a NamedDirectedGraph,
        after: &'a NamedDirectedGraph,
    },
    Unavailable {
        before: &'a Reading,
        after: &'a Reading,
    },
    NotApplicable {
        reason: &'a str,
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
fn graph(value: &serde_json::Value) -> Result<NamedDirectedGraph> {
    let mut graph: NamedDirectedGraph = serde::Deserialize::deserialize(value).map_err(invalid)?;
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
        let _: Parameters =
            serde::Deserialize::deserialize(ctx.params()).map_err(invalid_params)?;
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
        // The parsed graphs outlive the payload so it can borrow both of them.
        let (parsed_before, parsed_after) = (parsed[0].take(), parsed[1].take());
        let unsupported = |reading: &Reading| matches!(reading,Reading::Value {value} if !matches!(value,MeasurementValue::Graph(_)));
        let result = if unsupported(&before.reading) || unsupported(&after.reading) {
            GraphComparisonRef::NotApplicable {
                reason: "requires explicit named directed graphs",
            }
        } else if let (Some(a), Some(b)) = (&parsed_before, &parsed_after) {
            let an = a.nodes.iter().collect::<BTreeSet<_>>();
            let bn = b.nodes.iter().collect::<BTreeSet<_>>();
            let ae = a.edges.iter().collect::<BTreeSet<_>>();
            let be = b.edges.iter().collect::<BTreeSet<_>>();
            let nodes_added = bn.difference(&an).copied().collect::<Vec<_>>();
            let nodes_removed = an.difference(&bn).copied().collect::<Vec<_>>();
            let edges_added = be.difference(&ae).copied().collect::<Vec<_>>();
            let edges_removed = ae.difference(&be).copied().collect::<Vec<_>>();
            GraphComparisonRef::Value {
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
                before: a,
                after: b,
            }
        } else {
            GraphComparisonRef::Unavailable {
                before: &before.reading,
                after: &after.reading,
            }
        };
        Ok(token.emit(Delta {
            comparator: self.descriptor.id.clone(),
            value: MeasurementValue::Structured(
                serde_json::to_value(result).map_err(invalid)?.into(),
            ),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `GraphComparisonRef` is what the comparator writes and
    /// `GraphComparison` is what consumers read. They are one wire format, so
    /// this pins them to each other over all three arms.
    #[test]
    fn the_borrowed_payload_writes_what_the_owned_one_reads() {
        let edge = DirectedGraphEdge {
            source: "a".to_owned(),
            target: "b".to_owned(),
            kind: "k".to_owned(),
        };
        let a = NamedDirectedGraph {
            nodes: vec!["a".to_owned(), "b".to_owned()],
            edges: vec![edge.clone()],
        };
        let b = NamedDirectedGraph {
            nodes: vec!["a".to_owned()],
            edges: Vec::new(),
        };
        let node = "b".to_owned();
        let missing = Reading::NotMeasured;
        let scalar = Reading::Value {
            value: MeasurementValue::Scalar(1.0),
        };
        let cases = [
            (
                GraphComparisonRef::Value {
                    node_distance: 1,
                    edge_distance: 1,
                    nodes_added: Vec::new(),
                    nodes_removed: vec![&node],
                    edges_added: Vec::new(),
                    edges_removed: vec![&edge],
                    before: &a,
                    after: &b,
                },
                GraphComparison::Value {
                    node_distance: 1,
                    edge_distance: 1,
                    nodes_added: Vec::new(),
                    nodes_removed: vec![node.clone()],
                    edges_added: Vec::new(),
                    edges_removed: vec![edge.clone()],
                    before: a.clone(),
                    after: b.clone(),
                },
            ),
            (
                GraphComparisonRef::Unavailable {
                    before: &scalar,
                    after: &missing,
                },
                GraphComparison::Unavailable {
                    before: scalar.clone(),
                    after: missing.clone(),
                },
            ),
            (
                GraphComparisonRef::NotApplicable { reason: "why" },
                GraphComparison::NotApplicable {
                    reason: "why".to_owned(),
                },
            ),
        ];
        for (borrowed, owned) in cases {
            let written = serde_json::to_value(&borrowed).expect("borrowed payload serializes");
            assert_eq!(
                written,
                serde_json::to_value(&owned).expect("owned payload serializes")
            );
            let read: GraphComparison =
                serde_json::from_value(written).expect("a written delta reads back");
            assert_eq!(read, owned);
        }
    }
}
