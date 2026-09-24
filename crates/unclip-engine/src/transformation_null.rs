//! Exact existing-domain alternative for transformation proposals.

use serde::Deserialize;
use unclip_domain::{CandidateKind, PropertyValue, UnitKind};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{MeasurementValue, Reading};
use unclip_plugin::{NullCtx, NullModel, PluginDescriptor, PluginError, Result};

pub struct ExistingTransformationNull {
    descriptor: PluginDescriptor,
}

impl Default for ExistingTransformationNull {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("null.existing-transformation"),
                version: "0.1.0".parse().expect("valid transformation-null version"),
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {}

fn invalid(message: impl Into<String>) -> PluginError {
    PluginError::Message(message.into())
}

impl NullModel for ExistingTransformationNull {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    fn evaluate(&self, ctx: &NullCtx<'_>, token: CalculationToken) -> Result<Calculated<Reading>> {
        let _: Parameters = serde_json::from_value(ctx.params().clone())
            .map_err(|error| invalid(error.to_string()))?;
        let candidate = ctx.candidate();
        if candidate.kind != CandidateKind::Transformation {
            return Ok(token.emit(Reading::NotApplicable {
                reason: "requires an exact transformation proposal".into(),
            }));
        }
        let Some(domain) = ctx.domain() else {
            return Ok(token.emit(Reading::InsufficientEvidence { have: 0, need: 1 }));
        };
        super::domain_null::validate(domain)?;
        let key = serde_json::to_string(&(&domain.id.0, &domain.version.0))
            .map_err(|error| invalid(error.to_string()))?;
        if candidate.domain_version_id != key {
            return Err(invalid(
                "transformation null baseline differs from candidate domain version",
            ));
        }
        super::transformation_application::validate(candidate, domain)?;
        let pattern = candidate
            .value
            .get("pattern")
            .expect("validated transformation pattern");
        let matches = domain
            .units
            .values()
            .filter(|unit| {
                unit.kind == UnitKind::Transformation
                    && unit.properties.get("transformation_pattern")
                        == Some(&PropertyValue::Structured(pattern.clone()))
            })
            .map(|unit| unit.id.clone())
            .collect::<Vec<_>>();
        Ok(token.emit(Reading::Value {
            value: MeasurementValue::Structured(serde_json::json!({
                "model":"existing_transformation_exact_pattern",
                "domain_version_id":key,
                "matching":"exact_unit_state_transition",
                "match_count":matches.len(),
                "matches":matches,
                "has_existing_alternative":!matches.is_empty(),
                "scope":"exact directed transformation pattern identity only; labels, causality, and semantic equivalence are excluded",
                "decision":"no automatic candidate acceptance or rejection"
            })),
        }))
    }
}
