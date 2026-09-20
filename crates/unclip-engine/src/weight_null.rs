//! Baseline-retention null for explicit numeric property revisions.
use serde::Deserialize;
use unclip_domain::{CandidateKind, PropertyValue, RelationId, UnitId};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{MeasurementValue, Reading};
use unclip_plugin::{NullCtx, NullModel, PluginDescriptor, PluginError, Result};

pub struct WeightChangeNull {
    descriptor: PluginDescriptor,
}
impl Default for WeightChangeNull {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("null.weight-change"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["absolute_tolerance"],"properties":{"absolute_tolerance":{"type":"number","minimum":0}}}"#,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    absolute_tolerance: f64,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Target {
    Unit { id: UnitId },
    Relation { id: RelationId },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pattern {
    matching: String,
    target: Target,
    property: String,
    proposed_value: serde_json::Value,
}
fn invalid(message: &str) -> PluginError {
    PluginError::Message(message.into())
}
fn numeric(value: &PropertyValue) -> Result<f64> {
    let value = match value {
        PropertyValue::Integer(value) if value.unsigned_abs() <= 9_007_199_254_740_992 => {
            *value as f64
        }
        PropertyValue::Number(value) => *value,
        _ => {
            return Err(invalid(
                "weight must be numeric and integers must be exactly representable",
            ))
        }
    };
    if !value.is_finite() {
        return Err(invalid("weight must be finite"));
    }
    Ok(value)
}
impl NullModel for WeightChangeNull {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }
    fn evaluate(&self, ctx: &NullCtx<'_>, token: CalculationToken) -> Result<Calculated<Reading>> {
        let params: Parameters = serde_json::from_value(ctx.params().clone())
            .map_err(|e| PluginError::Message(e.to_string()))?;
        if !params.absolute_tolerance.is_finite() || params.absolute_tolerance < 0.0 {
            return Err(invalid("absolute tolerance must be finite and nonnegative"));
        }
        let candidate = ctx.candidate();
        let raw = candidate.value.get("pattern");
        if candidate.kind != CandidateKind::WeightRevision
            || raw.and_then(|p| p.get("matching")).and_then(|v| v.as_str())
                != Some("numeric_property_revision")
        {
            return Ok(token.emit(Reading::NotApplicable {
                reason: "requires an explicit numeric-property weight revision".into(),
            }));
        }
        let pattern: Pattern = serde_json::from_value(raw.unwrap().clone())
            .map_err(|e| PluginError::Message(e.to_string()))?;
        if pattern.property.trim().is_empty() {
            return Err(invalid("weight property must be explicit and nonempty"));
        }
        // Inspect integer JSON tokens before conversion, including unsigned values.
        if pattern
            .proposed_value
            .as_u64()
            .is_some_and(|v| v > 9_007_199_254_740_992)
        {
            return Err(invalid(
                "proposed integer weight exceeds exact numeric range",
            ));
        }
        let proposed: PropertyValue = serde_json::from_value(pattern.proposed_value.clone())
            .map_err(|e| PluginError::Message(e.to_string()))?;
        let proposed = numeric(&proposed)?;
        let Some(domain) = ctx.domain() else {
            return Ok(token.emit(Reading::InsufficientEvidence { have: 0, need: 1 }));
        };
        super::domain_null::validate(domain)?;
        let key = serde_json::to_string(&(&domain.id.0, &domain.version.0))
            .map_err(|e| PluginError::Message(e.to_string()))?;
        if candidate.domain_version_id != key {
            return Err(invalid(
                "null baseline differs from candidate domain version",
            ));
        }
        let properties = match &pattern.target {
            Target::Unit { id } => {
                &domain
                    .units
                    .get(id)
                    .ok_or_else(|| invalid("weight target unit does not exist"))?
                    .properties
            }
            Target::Relation { id } => {
                &domain
                    .relations
                    .get(id)
                    .ok_or_else(|| invalid("weight target relation does not exist"))?
                    .properties
            }
        };
        let Some(baseline) = properties.get(&pattern.property) else {
            return Ok(token.emit(Reading::InsufficientEvidence { have: 0, need: 1 }));
        };
        let baseline = numeric(baseline)?;
        let difference = proposed - baseline;
        if !difference.is_finite() {
            return Err(invalid("weight difference exceeds finite numeric range"));
        }
        Ok(token.emit(Reading::Value {value:MeasurementValue::Structured(serde_json::json!({
            "model":"retain_existing_numeric_property","domain_version_id":key,"matching":pattern.matching,
            "target":raw.unwrap()["target"],"property":pattern.property,"baseline_value":baseline,"proposed_value":proposed,
            "difference":difference,"absolute_tolerance":params.absolute_tolerance,"within_tolerance":difference.abs()<=params.absolute_tolerance,
            "scope":"numeric distance from retained baseline only; explanatory improvement requires held-out experiments",
            "decision":"no automatic candidate acceptance or rejection"
        }))}))
    }
}
