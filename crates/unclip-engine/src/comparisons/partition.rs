//! Membership-based partition comparison; group names and order have no meaning.
use crate::support::{invalid, invalid_params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{Delta, MeasurementKind, MeasurementValue, Reading};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, PluginError, Result};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum PartitionComparison {
    Value {
        rand_similarity: f64,
        pairs: usize,
        together_in_both: usize,
        separate_in_both: usize,
        split_pairs: usize,
        merged_pairs: usize,
        before: Vec<Vec<String>>,
        after: Vec<Vec<String>>,
    },
    Unavailable {
        reason: String,
        before: Reading,
        after: Reading,
    },
    NotApplicable {
        reason: String,
    },
}
/// The write side of [`PartitionComparison`], borrowing what it serializes.
///
/// Every arm borrows: the `Value` arm reports the canonicalized partitions as
/// sorted views of the readings' own member names, and the arms that report
/// whole readings borrow them because they are reached when a reading is not a
/// partition and can therefore carry a matrix or a graph. The round-trip test below pins this shape to the owning one.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum PartitionComparisonRef<'a> {
    Value {
        rand_similarity: f64,
        pairs: usize,
        together_in_both: usize,
        separate_in_both: usize,
        split_pairs: usize,
        merged_pairs: usize,
        before: Vec<Vec<&'a str>>,
        after: Vec<Vec<&'a str>>,
    },
    Unavailable {
        reason: &'a str,
        before: &'a Reading,
        after: &'a Reading,
    },
    NotApplicable {
        reason: &'a str,
    },
}
pub struct PartitionRandComparator {
    descriptor: ComparatorDescriptor,
}
impl Default for PartitionRandComparator {
    fn default() -> Self {
        Self {
            descriptor: ComparatorDescriptor {
                id: PluginId::new("compare.partition-rand"),
                version: semver::Version::new(0, 1, 0),
                supports: &[MeasurementKind::Partition],
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {}
fn canonical(groups: &[Vec<String>]) -> Result<Vec<Vec<&str>>> {
    let mut result = groups
        .iter()
        .map(|group| group.iter().map(String::as_str).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let mut seen = std::collections::BTreeSet::new();
    for group in &mut result {
        if group.is_empty() {
            return Err(invalid("partition groups must be nonempty"));
        }
        group.sort();
        for member in group.iter() {
            if member.trim().is_empty() || !seen.insert(member) {
                return Err(invalid(
                    "partition members must be nonempty and occur exactly once",
                ));
            }
        }
    }
    result.sort();
    Ok(result)
}
fn membership<'a>(groups: &[Vec<&'a str>]) -> BTreeMap<&'a str, usize> {
    groups
        .iter()
        .enumerate()
        .flat_map(|(index, group)| group.iter().map(move |member| (*member, index)))
        .collect()
}
impl Comparator for PartitionRandComparator {
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
                "partition comparison requires the same sensor, version, and measurement context",
            ));
        }
        let mut parsed = Vec::new();
        for reading in [&before.reading, &after.reading] {
            parsed.push(
                if let Reading::Value {
                    value: MeasurementValue::Partition(groups),
                } = reading
                {
                    Some(canonical(groups)?)
                } else {
                    None
                },
            );
        }
        let unsupported = |reading: &Reading| matches!(reading,Reading::Value {value} if !matches!(value,MeasurementValue::Partition(_)));
        let result = if unsupported(&before.reading) || unsupported(&after.reading) {
            PartitionComparisonRef::NotApplicable {
                reason: "requires explicit partitions; no membership is inferred",
            }
        } else if let (Some(a), Some(b)) = (parsed[0].take(), parsed[1].take()) {
            let left = membership(&a);
            let right = membership(&b);
            if !left.keys().eq(right.keys()) {
                return Err(invalid(
                    "partition comparison requires identical member sets",
                ));
            }
            if left.len() < 2 {
                PartitionComparisonRef::Unavailable {
                    reason: "partition pair comparison requires at least two members",
                    before: &before.reading,
                    after: &after.reading,
                }
            } else {
                let pairs = left
                    .len()
                    .checked_mul(left.len() - 1)
                    .and_then(|n| n.checked_div(2))
                    .ok_or_else(|| invalid("partition pair count overflow"))?;
                let members = left.keys().copied().collect::<Vec<_>>();
                let (mut together, mut separate, mut split, mut merged) = (0, 0, 0, 0);
                for i in 0..members.len() {
                    for j in i + 1..members.len() {
                        match (
                            left[members[i]] == left[members[j]],
                            right[members[i]] == right[members[j]],
                        ) {
                            (true, true) => together += 1,
                            (false, false) => separate += 1,
                            (true, false) => split += 1,
                            (false, true) => merged += 1,
                        }
                    }
                }
                let rand_similarity = (together + separate) as f64 / pairs as f64;
                PartitionComparisonRef::Value {
                    rand_similarity,
                    pairs,
                    together_in_both: together,
                    separate_in_both: separate,
                    split_pairs: split,
                    merged_pairs: merged,
                    before: a,
                    after: b,
                }
            }
        } else {
            PartitionComparisonRef::Unavailable {
                reason: "both partition readings must be measured",
                before: &before.reading,
                after: &after.reading,
            }
        };
        Ok(token.emit(Delta {
            comparator: self.descriptor.id.clone(),
            value: MeasurementValue::Structured(
                serde_json::to_value(result)
                    .map_err(|e| PluginError::Message(e.to_string()))?
                    .into(),
            ),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `PartitionComparisonRef` is what the comparator writes and
    /// `PartitionComparison` is what consumers read. They are one wire format,
    /// so this pins them to each other over all three arms.
    #[test]
    fn the_borrowed_payload_writes_what_the_owned_one_reads() {
        let graph = Reading::Value {
            value: MeasurementValue::Graph(serde_json::json!({"nodes": []}).into()),
        };
        let missing = Reading::NotMeasured;
        let groups = vec![vec!["a".to_owned()], vec!["b".to_owned()]];
        let borrowed = vec![vec!["a"], vec!["b"]];
        let cases = [
            (
                PartitionComparisonRef::Value {
                    rand_similarity: 0.5,
                    pairs: 2,
                    together_in_both: 1,
                    separate_in_both: 0,
                    split_pairs: 1,
                    merged_pairs: 0,
                    before: borrowed.clone(),
                    after: borrowed,
                },
                PartitionComparison::Value {
                    rand_similarity: 0.5,
                    pairs: 2,
                    together_in_both: 1,
                    separate_in_both: 0,
                    split_pairs: 1,
                    merged_pairs: 0,
                    before: groups.clone(),
                    after: groups,
                },
            ),
            (
                PartitionComparisonRef::Unavailable {
                    reason: "why",
                    before: &graph,
                    after: &missing,
                },
                PartitionComparison::Unavailable {
                    reason: "why".to_owned(),
                    before: graph.clone(),
                    after: missing.clone(),
                },
            ),
            (
                PartitionComparisonRef::NotApplicable { reason: "why" },
                PartitionComparison::NotApplicable {
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
            let read: PartitionComparison =
                serde_json::from_value(written).expect("a written delta reads back");
            assert_eq!(read, owned);
        }
    }
}
