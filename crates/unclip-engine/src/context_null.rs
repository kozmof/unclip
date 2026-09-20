//! Conditional co-presence diagnostics within explicitly recorded metadata strata.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use unclip_domain::CandidateKind;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{MeasurementValue, Reading};
use unclip_plugin::{NullCtx, NullModel, PluginDescriptor, PluginError, Result};

pub struct ContextualCooccurrenceNull {
    descriptor: PluginDescriptor,
}
impl Default for ContextualCooccurrenceNull {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("null.contextual-cooccurrence"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["strata","minimum_observations"],"properties":{"minimum_observations":{"type":"integer","minimum":2},"strata":{"type":"array","minItems":1,"uniqueItems":true,"items":{"oneOf":[{"type":"object","additionalProperties":false,"required":["field"],"properties":{"field":{"const":"source"}}},{"type":"object","additionalProperties":false,"required":["field","key"],"properties":{"field":{"const":"context"},"key":{"type":"string","minLength":1}}}]}}}}"#,
            },
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "field", rename_all = "snake_case", deny_unknown_fields)]
enum Stratum {
    Source,
    Context { key: String },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    strata: Vec<Stratum>,
    minimum_observations: usize,
}
#[derive(Default)]
struct Group {
    values: Vec<Value>,
    observations: BTreeSet<String>,
    left: usize,
    right: usize,
    overlap: usize,
}
fn invalid(s: &str) -> PluginError {
    PluginError::Message(s.into())
}
impl NullModel for ContextualCooccurrenceNull {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }
    fn evaluate(&self, ctx: &NullCtx<'_>, token: CalculationToken) -> Result<Calculated<Reading>> {
        let params: Parameters = serde_json::from_value(ctx.params().clone())
            .map_err(|e| PluginError::Message(e.to_string()))?;
        let unique = params.strata.iter().collect::<BTreeSet<_>>();
        if params.minimum_observations < 2
            || params.strata.is_empty()
            || unique.len() != params.strata.len()
            || params
                .strata
                .iter()
                .any(|s| matches!(s,Stratum::Context {key} if key.trim().is_empty()))
        {
            return Err(invalid("context null requires unique explicit strata and at least two observations per stratum"));
        }
        let candidate = ctx.candidate();
        let pattern = candidate.value.get("pattern");
        if candidate.kind != CandidateKind::Relation
            || pattern
                .and_then(|p| p.get("matching"))
                .and_then(Value::as_str)
                != Some("exact_directed_observed_relation")
        {
            return Ok(token.emit(Reading::NotApplicable {
                reason: "requires an exact observed-label relation proposal".into(),
            }));
        }
        let pattern = pattern.unwrap();
        let left = pattern
            .get("source_label")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| invalid("missing source label"))?;
        let right = pattern
            .get("target_label")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| invalid("missing target label"))?;
        if left == right {
            return Ok(token.emit(Reading::NotApplicable {
                reason: "same-label endpoints need a multiplicity null model".into(),
            }));
        }
        let mut inputs = ctx.observations().iter().collect::<Vec<_>>();
        inputs.sort_by_key(|input| input.id());
        let mut derived_ids = BTreeSet::new();
        let mut observations = BTreeSet::new();
        let mut groups = BTreeMap::<Vec<String>, Group>::new();
        let mut excluded = Vec::new();
        for input in inputs {
            let observation = ctx.read(input);
            if !derived_ids.insert(input.id())
                || observation.id.0.is_empty()
                || !observations.insert(&observation.id)
            {
                return Err(invalid("duplicate or empty null observation identity"));
            }
            let mut values = Vec::new();
            let mut missing = Vec::new();
            for selector in &params.strata {
                let value = match selector {
                    Stratum::Source => Some(Value::String(observation.source.0.clone())),
                    Stratum::Context { key } => observation.context.get(key).cloned(),
                };
                match value {
                    None | Some(Value::Null) => missing.push(selector),
                    Some(Value::String(s)) if s.trim().is_empty() => missing.push(selector),
                    Some(value @ (Value::String(_) | Value::Bool(_) | Value::Number(_))) => values.push(value),
                    _ => return Err(invalid("stratum metadata must be a scalar category; arrays and objects require an explicit derived category")),
                }
            }
            if !missing.is_empty() {
                excluded.push(json!({"observation":observation.id,"missing":missing}));
                continue;
            }
            let key = values
                .iter()
                .map(|v| serde_json::to_string(v).expect("finite JSON scalar"))
                .collect::<Vec<_>>();
            let group = groups.entry(key).or_insert_with(|| Group {
                values,
                ..Default::default()
            });
            group.observations.insert(observation.id.0.clone());
            let a = observation.units.iter().any(|u| u.label == left);
            let b = observation.units.iter().any(|u| u.label == right);
            group.left += usize::from(a);
            group.right += usize::from(b);
            group.overlap += usize::from(a && b);
        }
        let mut results = Vec::new();
        let mut assessed = 0;
        for group in groups.into_values() {
            let n = group.observations.len();
            let reading = if n < params.minimum_observations {
                Reading::InsufficientEvidence {
                    have: n,
                    need: params.minimum_observations,
                }
            } else {
                assessed += 1;
                Reading::Value {
                    value: MeasurementValue::Structured(
                        json!({"expected_overlap":group.left as f64*group.right as f64/n as f64,"upper_tail_probability":super::null_models::overlap_tail(n,group.left,group.right,group.overlap)}),
                    ),
                }
            };
            results.push(json!({"values":group.values,"observations":group.observations,"sample_count":n,"source_count":group.left,"target_count":group.right,"observed_overlap":group.overlap,"reading":reading}));
        }
        Ok(token.emit(Reading::Value {value:MeasurementValue::Structured(json!({
            "model":"fixed_margin_within_recorded_strata","source_label":left,"target_label":right,"strata":params.strata,
            "selected_observations":observations.len(),"assessed_strata":assessed,"results":results,"excluded_missing_metadata":excluded,
            "assumption":"observations are exchangeable within each recorded stratum",
            "scope":"endpoint co-presence conditional on recorded categories only; no inferred metadata, automatic time bins, or proof of bias removal",
            "selection_adjusted":false,"multiple_testing_adjusted":false,"causal_claim":false
        }))}))
    }
}
