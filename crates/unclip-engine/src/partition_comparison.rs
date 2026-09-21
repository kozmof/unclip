//! Membership-based partition comparison; group names and order have no meaning.
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
fn invalid(s: &str) -> PluginError {
    PluginError::Message(s.into())
}
fn canonical(groups: &[Vec<String>]) -> Result<Vec<Vec<String>>> {
    let mut result = groups.to_vec();
    let mut seen = std::collections::BTreeSet::new();
    for group in &mut result {
        if group.is_empty() {
            return Err(invalid("partition groups must be nonempty"));
        }
        for member in group.iter() {
            if member.trim().is_empty() || !seen.insert(member.clone()) {
                return Err(invalid(
                    "partition members must be nonempty and occur exactly once",
                ));
            }
        }
        group.sort();
    }
    result.sort();
    Ok(result)
}
fn membership(groups: &[Vec<String>]) -> BTreeMap<&str, usize> {
    groups
        .iter()
        .enumerate()
        .flat_map(|(index, group)| group.iter().map(move |member| (member.as_str(), index)))
        .collect()
}
impl Comparator for PartitionRandComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }
    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let _: Parameters = serde_json::from_value(ctx.params().clone())
            .map_err(|e| PluginError::Message(e.to_string()))?;
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
            PartitionComparison::NotApplicable {
                reason: "requires explicit partitions; no membership is inferred".into(),
            }
        } else if let (Some(a), Some(b)) = (&parsed[0], &parsed[1]) {
            let left = membership(a);
            let right = membership(b);
            if !left.keys().eq(right.keys()) {
                return Err(invalid(
                    "partition comparison requires identical member sets",
                ));
            }
            if left.len() < 2 {
                PartitionComparison::Unavailable {
                    reason: "partition pair comparison requires at least two members".into(),
                    before: before.reading.clone(),
                    after: after.reading.clone(),
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
                PartitionComparison::Value {
                    rand_similarity: (together + separate) as f64 / pairs as f64,
                    pairs,
                    together_in_both: together,
                    separate_in_both: separate,
                    split_pairs: split,
                    merged_pairs: merged,
                    before: a.clone(),
                    after: b.clone(),
                }
            }
        } else {
            PartitionComparison::Unavailable {
                reason: "both partition readings must be measured".into(),
                before: before.reading.clone(),
                after: after.reading.clone(),
            }
        };
        Ok(token.emit(Delta {
            comparator: self.descriptor.id.clone(),
            value: MeasurementValue::Structured(
                serde_json::to_value(result).map_err(|e| PluginError::Message(e.to_string()))?,
            ),
        }))
    }
}
