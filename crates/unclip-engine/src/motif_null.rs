//! Exact existing-domain alternative for recurring graph-motif proposals.

use serde::Deserialize;
use unclip_domain::{CandidateKind, PropertyValue, UnitKind};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{MeasurementValue, Reading};
use unclip_plugin::{NullCtx, NullModel, PluginDescriptor, PluginError, Result};

pub struct ExistingMotifNull {
    descriptor: PluginDescriptor,
}

impl Default for ExistingMotifNull {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("null.existing-motif"),
                version: semver::Version::new(0, 1, 0),
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

impl NullModel for ExistingMotifNull {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    fn evaluate(&self, ctx: &NullCtx<'_>, token: CalculationToken) -> Result<Calculated<Reading>> {
        let _: Parameters = serde_json::from_value(ctx.params().clone())
            .map_err(|error| invalid(error.to_string()))?;
        let candidate = ctx.candidate();
        if candidate.kind != CandidateKind::GraphMotif
            || candidate
                .value
                .get("pattern")
                .and_then(|pattern| pattern.get("matching"))
                .and_then(serde_json::Value::as_str)
                != Some("exact_directed_two_edge_path")
        {
            return Ok(token.emit(Reading::NotApplicable {
                reason: "requires a recurring graph-motif proposal".into(),
            }));
        }
        super::motif_application::validate(candidate)?;
        let Some(domain) = ctx.domain() else {
            return Ok(token.emit(Reading::InsufficientEvidence { have: 0, need: 1 }));
        };
        super::domain_null::validate(domain)?;
        let key = serde_json::to_string(&(&domain.id.0, &domain.version.0))
            .map_err(|error| invalid(error.to_string()))?;
        if candidate.domain_version_id != key {
            return Err(invalid(
                "motif null baseline differs from candidate domain version",
            ));
        }
        let pattern = candidate
            .value
            .get("pattern")
            .expect("validated motif pattern");
        let mut matches = Vec::new();
        for unit in domain.units.values() {
            if unit.kind == UnitKind::GraphMotif
                && unit.properties.get("graph_pattern")
                    == Some(&PropertyValue::Structured(pattern.clone()))
            {
                matches.push(unit.id.clone());
            }
        }
        Ok(token.emit(Reading::Value {
            value: MeasurementValue::Structured(serde_json::json!({
                "model": "existing_motif_exact_pattern",
                "domain_version_id": key,
                "matching": "exact_directed_two_edge_path",
                "match_count": matches.len(),
                "matches": matches,
                "has_existing_alternative": !matches.is_empty(),
                "scope": "exact graph-motif pattern identity only; this does not establish semantic equivalence or explanatory adequacy",
                "decision": "no automatic candidate acceptance or rejection"
            })),
        }))
    }
}
