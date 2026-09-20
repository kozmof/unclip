//! Conditional endpoint-order null preserving observed ties and unknown ranks.
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use unclip_domain::CandidateKind;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{MeasurementValue, Reading};
use unclip_plugin::{NullCtx, NullModel, PluginDescriptor, PluginError, Result};

pub struct RankingConstraintNull {
    descriptor: PluginDescriptor,
}
impl Default for RankingConstraintNull {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("null.ranking-constraints"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["minimum_observations"],"properties":{"minimum_observations":{"type":"integer","minimum":2}}}"#,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    minimum_observations: usize,
}
fn invalid(s: &str) -> PluginError {
    PluginError::Message(s.into())
}
// Binomial(n, 1/2) masses relative to the mode, avoiding factorials.
fn order_tail(n: usize, observed: usize) -> f64 {
    let mode = n / 2;
    let mut total = 1.0;
    let mut tail = if mode >= observed { 1.0 } else { 0.0 };
    let mut weight = 1.0;
    for k in mode..n {
        weight *= (n - k) as f64 / (k + 1) as f64;
        total += weight;
        if k + 1 >= observed {
            tail += weight;
        }
    }
    weight = 1.0;
    for k in (1..=mode).rev() {
        weight *= k as f64 / (n - k + 1) as f64;
        total += weight;
        if k > observed {
            tail += weight;
        }
    }
    (tail / total).clamp(0.0, 1.0)
}
impl NullModel for RankingConstraintNull {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }
    fn evaluate(&self, ctx: &NullCtx<'_>, token: CalculationToken) -> Result<Calculated<Reading>> {
        let params: Parameters = serde_json::from_value(ctx.params().clone())
            .map_err(|e| PluginError::Message(e.to_string()))?;
        if params.minimum_observations < 2 {
            return Err(invalid(
                "ranking null requires at least two comparable observations",
            ));
        }
        let candidate = ctx.candidate();
        let pattern = candidate.value.get("pattern");
        if candidate.kind != CandidateKind::Relation
            || pattern
                .and_then(|p| p.get("matching"))
                .and_then(|v| v.as_str())
                != Some("exact_directed_observed_relation")
        {
            return Ok(token.emit(Reading::NotApplicable {
                reason: "requires an exact observed-label relation proposal".into(),
            }));
        }
        let pattern = pattern.unwrap();
        let left = pattern
            .get("source_label")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| invalid("missing source label"))?;
        let right = pattern
            .get("target_label")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| invalid("missing target label"))?;
        if left == right {
            return Ok(token.emit(Reading::NotApplicable {
                reason: "same-label endpoints have no unique relative order".into(),
            }));
        }
        let mut observations = BTreeMap::new();
        let mut derived_ids = BTreeSet::new();
        for input in ctx.observations() {
            let observation = ctx.read(input);
            if !derived_ids.insert(input.id())
                || observation.id.0.is_empty()
                || observations.insert(&observation.id, observation).is_some()
            {
                return Err(invalid("duplicate or empty null observation identity"));
            }
        }
        let mut rankings = BTreeMap::new();
        let mut ranking_ids = BTreeSet::new();
        for input in ctx.rankings() {
            let ranking = ctx.read(input);
            if !ranking_ids.insert(input.id())
                || rankings.insert(&ranking.observation, ranking).is_some()
            {
                return Err(invalid("duplicate ranking identity or observation"));
            }
            if !observations.contains_key(&ranking.observation) {
                return Err(invalid("ranking has no selected source observation"));
            }
        }
        let (mut before, mut after, mut ties, mut skipped) = (0, 0, 0, 0);
        let mut evidence = Vec::new();
        for (id, observation) in observations {
            let mut units = BTreeMap::new();
            for unit in &observation.units {
                if unit.id.0.is_empty() || units.insert(&unit.id, unit).is_some() {
                    return Err(invalid("duplicate or empty observed unit identity"));
                }
            }
            let mut positions = BTreeMap::new();
            if let Some(ranking) = rankings.get(id) {
                let mut seen = BTreeSet::new();
                for (index, tier) in ranking.tiers.iter().enumerate() {
                    if tier.units.is_empty() {
                        return Err(invalid("empty ranking tier"));
                    }
                    for unit in &tier.units {
                        if !units.contains_key(unit) || !seen.insert(unit) {
                            return Err(invalid("unknown or repeated ranked unit"));
                        }
                        positions.insert(unit, index);
                    }
                }
                for unit in &ranking.unknown {
                    if !units.contains_key(unit) || !seen.insert(unit) {
                        return Err(invalid("unknown or repeated unranked unit"));
                    }
                }
            }
            let left_units = observation
                .units
                .iter()
                .filter(|u| u.label == left)
                .collect::<Vec<_>>();
            let right_units = observation
                .units
                .iter()
                .filter(|u| u.label == right)
                .collect::<Vec<_>>();
            if left_units.len() > 1 || right_units.len() > 1 {
                return Err(invalid(
                    "endpoint labels must identify unique observed units",
                ));
            }
            let pair = left_units
                .first()
                .and_then(|u| positions.get(&u.id))
                .zip(right_units.first().and_then(|u| positions.get(&u.id)));
            let status = match pair {
                Some((a, b)) if a < b => {
                    before += 1;
                    "source_before_target"
                }
                Some((a, b)) if a > b => {
                    after += 1;
                    "source_after_target"
                }
                Some(_) => {
                    ties += 1;
                    "tied"
                }
                None => {
                    skipped += 1;
                    "missing_endpoint_or_rank"
                }
            };
            evidence.push(serde_json::json!({"observation":id,"status":status}));
        }
        let n = before + after;
        if n < params.minimum_observations {
            return Ok(token.emit(Reading::InsufficientEvidence {
                have: n,
                need: params.minimum_observations,
            }));
        }
        Ok(token.emit(Reading::Value {value:MeasurementValue::Structured(serde_json::json!({
            "model":"conditional_exchangeable_endpoint_order","source_label":left,"target_label":right,
            "sample_count":n,"source_before_target":before,"source_after_target":after,"ties":ties,"skipped":skipped,
            "expected_source_before_target":n as f64/2.0,"upper_tail_probability":order_tail(n,before),"evidence":evidence,
            "constraints":"condition on endpoint availability and pair tie status; retain all ranking tiers and unknown membership",
            "assumption":"untied endpoint order is independently exchangeable within each observation",
            "scope":"relative endpoint rank only; does not explain relation kind, metric coupling, or temporal dependence",
            "selection_adjusted":false,"causal_claim":false
        }))}))
    }
}
#[cfg(test)]
mod tests {
    use super::order_tail;
    #[test]
    fn matches_all_small_endpoint_swaps() {
        for n in 0usize..=10 {
            for observed in 0..=n {
                let expected = (0u32..(1 << n))
                    .filter(|bits| bits.count_ones() as usize >= observed)
                    .count() as f64
                    / (1 << n) as f64;
                assert!((order_tail(n, observed) - expected).abs() < 1e-12);
            }
        }
        assert!((order_tail(10000, 5000) - 0.5).abs() < 0.02);
    }
}
