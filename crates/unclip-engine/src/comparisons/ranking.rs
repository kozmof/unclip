//! Separate ranking distances with explicit supported evidence shapes.
use crate::support::{invalid, invalid_params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{Delta, MeasurementKind, MeasurementValue, RankedState, Reading};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, PluginError, Result};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RankingComparison {
    Kendall {
        distance: f64,
        discordant_pairs: usize,
        pairs: usize,
        before: RankedState,
        after: RankedState,
    },
    Rbo {
        similarity: f64,
        p: f64,
        depth: usize,
        before: RankedState,
        after: RankedState,
    },
    Unavailable {
        before: Reading,
        after: Reading,
    },
    NotApplicable {
        reason: String,
    },
}
/// The write side of [`RankingComparison`], borrowing what it serializes.
///
/// Both measured arms report the two ranked states verbatim, and a ranked
/// state owns a tier vector per unit; the `Unavailable` arm is reached when a
/// reading is not a ranking at all, so it can carry a matrix or a graph.
/// Either way the payload is built only to become JSON and is then dropped.
/// The round-trip test below pins this shape to the owning one.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum RankingComparisonRef<'a> {
    Kendall {
        distance: f64,
        discordant_pairs: usize,
        pairs: usize,
        before: &'a RankedState,
        after: &'a RankedState,
    },
    Rbo {
        similarity: f64,
        p: f64,
        depth: usize,
        before: &'a RankedState,
        after: &'a RankedState,
    },
    Unavailable {
        before: &'a Reading,
        after: &'a Reading,
    },
    NotApplicable {
        reason: &'a str,
    },
}
pub struct KendallComparator {
    descriptor: ComparatorDescriptor,
}
pub struct RboComparator {
    descriptor: ComparatorDescriptor,
}
impl Default for KendallComparator {
    fn default() -> Self {
        Self {
            descriptor: ComparatorDescriptor {
                id: PluginId::new("compare.kendall"),
                version: semver::Version::new(0, 1, 0),
                supports: &[MeasurementKind::Ranking],
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}
impl Default for RboComparator {
    fn default() -> Self {
        Self {
            descriptor: ComparatorDescriptor {
                id: PluginId::new("compare.rbo"),
                version: semver::Version::new(0, 1, 0),
                supports: &[MeasurementKind::Ranking],
                params_schema: r#"{
                    "type":"object",
                    "additionalProperties":false,
                    "required":["p"],
                    "properties":{"p":{"type":"number","exclusiveMinimum":0,"exclusiveMaximum":1}}
                }"#,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RboParams {
    p: f64,
}
fn validate(state: &RankedState) -> Result<()> {
    let mut seen = BTreeSet::new();
    for tier in &state.tiers {
        if tier.is_empty() {
            return Err(invalid("ranking contains an empty tier"));
        }
        for id in tier {
            if id.is_empty() || !seen.insert(id) {
                return Err(invalid("ranking has empty or repeated unit identities"));
            }
        }
    }
    for id in &state.unknown {
        if id.is_empty() || !seen.insert(id) {
            return Err(invalid(
                "unknown ranks overlap known ranks or repeat identities",
            ));
        }
    }
    let mut unresolved = BTreeSet::new();
    for id in &state.unresolved {
        if id.is_empty() || !unresolved.insert(id) {
            return Err(invalid("invalid unresolved ranking identities"));
        }
    }
    Ok(())
}
fn compare<'a>(ctx: &'a CompareCtx<'_>, p: Option<f64>) -> Result<RankingComparisonRef<'a>> {
    let before = ctx.before();
    let after = ctx.after();
    if before.sensor != after.sensor
        || before.sensor_version != after.sensor_version
        || before.context != after.context
    {
        return Err(invalid(
            "ranking comparison requires the same sensor, version, and measurement context",
        ));
    }
    for reading in [&before.reading, &after.reading] {
        if let Reading::Value {
            value: MeasurementValue::Ranking(state),
        } = reading
        {
            validate(state)?;
        }
    }
    let unsupported = |reading: &Reading| matches!(reading,Reading::Value {value} if !matches!(value,MeasurementValue::Ranking(_)));
    if unsupported(&before.reading) || unsupported(&after.reading) {
        return Ok(RankingComparisonRef::NotApplicable {
            reason: "requires ranking measurements",
        });
    }
    let (
        Reading::Value {
            value: MeasurementValue::Ranking(a),
        },
        Reading::Value {
            value: MeasurementValue::Ranking(b),
        },
    ) = (&before.reading, &after.reading)
    else {
        return Ok(RankingComparisonRef::Unavailable {
            before: &before.reading,
            after: &after.reading,
        });
    };
    if !a.unresolved.is_empty()
        || !b.unresolved.is_empty()
        || a.tiers.iter().chain(&b.tiers).any(|t| t.len() != 1)
    {
        return Ok(RankingComparisonRef::NotApplicable {
            reason: "this comparator requires untied rankings without unresolved identities",
        });
    }
    let left = a.tiers.iter().map(|t| &t[0]).collect::<Vec<_>>();
    let right = b.tiers.iter().map(|t| &t[0]).collect::<Vec<_>>();
    if let Some(p) = p {
        if left.len() != right.len() {
            return Ok(RankingComparisonRef::NotApplicable {
                reason: "this RBO comparator requires equal observed prefix depths",
            });
        }
        if left.is_empty() {
            return Ok(RankingComparisonRef::Unavailable {
                before: &before.reading,
                after: &after.reading,
            });
        }
        let mut left_prefix = BTreeSet::new();
        let mut right_prefix = BTreeSet::new();
        let mut similarity = 0.0;
        let mut persistence = 1.0;
        let mut agreement = 0.0;
        for (index, (a, b)) in left.iter().zip(&right).enumerate() {
            left_prefix.insert(*a);
            right_prefix.insert(*b);
            agreement = left_prefix.intersection(&right_prefix).count() as f64 / (index + 1) as f64;
            similarity += (1.0 - p) * persistence * agreement;
            persistence *= p;
        }
        similarity += persistence * agreement;
        Ok(RankingComparisonRef::Rbo {
            similarity: similarity.clamp(0.0, 1.0),
            p,
            depth: left.len(),
            before: a,
            after: b,
        })
    } else {
        if !a.unknown.is_empty()
            || !b.unknown.is_empty()
            || left.iter().copied().collect::<BTreeSet<_>>()
                != right.iter().copied().collect::<BTreeSet<_>>()
        {
            return Ok(RankingComparisonRef::NotApplicable {
                reason: "Kendall distance requires complete rankings over the same units",
            });
        }
        if left.len() < 2 {
            return Ok(RankingComparisonRef::Unavailable {
                before: &before.reading,
                after: &after.reading,
            });
        }
        let positions = right
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect::<BTreeMap<_, _>>();
        let pairs = left
            .len()
            .checked_mul(left.len() - 1)
            .and_then(|v| v.checked_div(2))
            .ok_or_else(|| invalid("ranking pair count overflow"))?;
        let mut discordant_pairs = 0;
        for i in 0..left.len() {
            for j in i + 1..left.len() {
                discordant_pairs += usize::from(positions[left[i]] > positions[left[j]]);
            }
        }
        Ok(RankingComparisonRef::Kendall {
            distance: discordant_pairs as f64 / pairs as f64,
            discordant_pairs,
            pairs,
            before: a,
            after: b,
        })
    }
}
fn emit(
    id: &PluginId,
    result: RankingComparisonRef<'_>,
    token: CalculationToken,
) -> Result<Calculated<Delta>> {
    Ok(token.emit(Delta {
        comparator: id.clone(),
        value: MeasurementValue::Structured(
            serde_json::to_value(result)
                .map_err(|e| PluginError::Message(e.to_string()))?
                .into(),
        ),
    }))
}
impl Comparator for KendallComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }
    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let _: Empty = serde::Deserialize::deserialize(ctx.params()).map_err(invalid_params)?;
        emit(&self.descriptor.id, compare(ctx, None)?, token)
    }
}
impl Comparator for RboComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }
    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let params: RboParams =
            serde::Deserialize::deserialize(ctx.params()).map_err(invalid_params)?;
        if !params.p.is_finite() || params.p <= 0.0 || params.p >= 1.0 {
            return Err(invalid(
                "RBO p must be finite and strictly between zero and one",
            ));
        }
        emit(&self.descriptor.id, compare(ctx, Some(params.p))?, token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unclip_domain::UnitId;

    /// `RankingComparisonRef` is what both comparators write and
    /// `RankingComparison` is what consumers read. They are one wire format,
    /// so this pins them to each other over all four arms.
    #[test]
    fn the_borrowed_payload_writes_what_the_owned_one_reads() {
        let a = RankedState {
            tiers: vec![vec![UnitId::new("a")], vec![UnitId::new("b")]],
            unknown: Vec::new(),
            unresolved: Vec::new(),
        };
        let b = RankedState {
            tiers: vec![vec![UnitId::new("b")], vec![UnitId::new("a")]],
            unknown: Vec::new(),
            unresolved: Vec::new(),
        };
        let matrix = Reading::Value {
            value: MeasurementValue::Matrix(vec![vec![1.0, 0.5], vec![0.5, 1.0]].into()),
        };
        let missing = Reading::NotMeasured;
        let cases = [
            (
                RankingComparisonRef::Kendall {
                    distance: 1.0,
                    discordant_pairs: 1,
                    pairs: 1,
                    before: &a,
                    after: &b,
                },
                RankingComparison::Kendall {
                    distance: 1.0,
                    discordant_pairs: 1,
                    pairs: 1,
                    before: a.clone(),
                    after: b.clone(),
                },
            ),
            (
                RankingComparisonRef::Rbo {
                    similarity: 0.25,
                    p: 0.5,
                    depth: 2,
                    before: &a,
                    after: &b,
                },
                RankingComparison::Rbo {
                    similarity: 0.25,
                    p: 0.5,
                    depth: 2,
                    before: a.clone(),
                    after: b.clone(),
                },
            ),
            (
                RankingComparisonRef::Unavailable {
                    before: &matrix,
                    after: &missing,
                },
                RankingComparison::Unavailable {
                    before: matrix.clone(),
                    after: missing.clone(),
                },
            ),
            (
                RankingComparisonRef::NotApplicable { reason: "why" },
                RankingComparison::NotApplicable {
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
            let read: RankingComparison =
                serde_json::from_value(written).expect("a written delta reads back");
            assert_eq!(read, owned);
        }
    }
}
