//! Existing-domain alternatives are exact structural matches, not semantic verdicts.
use serde::Deserialize;
use unclip_domain::{CandidateKind, DomainSnapshot, UnitKind};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{MeasurementValue, Reading};
use unclip_plugin::{NullCtx, NullModel, PluginDescriptor, PluginError, Result};

macro_rules! model {
    ($name:ident, $id:literal, $kind:ident) => {
        pub struct $name {
            descriptor: PluginDescriptor,
        }
        impl Default for $name {
            fn default() -> Self {
                Self {
                    descriptor: PluginDescriptor {
                        id: PluginId::new($id),
                        version: semver::Version::new(0, 1, 0),
                        params_schema: r#"{"type":"object","additionalProperties":false}"#,
                    },
                }
            }
        }
        impl NullModel for $name {
            fn descriptor(&self) -> &PluginDescriptor {
                &self.descriptor
            }
            fn evaluate(
                &self,
                ctx: &NullCtx<'_>,
                token: CalculationToken,
            ) -> Result<Calculated<Reading>> {
                evaluate(ctx, token, CandidateKind::$kind)
            }
        }
    };
}
model!(ExistingUnitNull, "null.existing-unit", AtomicMeaning);
model!(ExistingRelationNull, "null.existing-relation", Relation);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {}
fn invalid(message: &str) -> PluginError {
    PluginError::Message(message.into())
}
fn label<'a>(pattern: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    pattern
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| invalid("missing or empty exact-match pattern field"))
}
fn validate(domain: &DomainSnapshot) -> Result<()> {
    if domain.id.0.is_empty() || domain.version.0.is_empty() {
        return Err(invalid("empty baseline domain identity"));
    }
    for (id, unit) in &domain.units {
        if id != &unit.id || id.0.is_empty() {
            return Err(invalid("inconsistent baseline unit identity"));
        }
    }
    for (id, relation) in &domain.relations {
        if id != &relation.id
            || id.0.is_empty()
            || relation.kind.trim().is_empty()
            || !domain.units.contains_key(&relation.source)
            || !domain.units.contains_key(&relation.target)
        {
            return Err(invalid("invalid baseline relation identity or endpoints"));
        }
    }
    Ok(())
}
fn evaluate(
    ctx: &NullCtx<'_>,
    token: CalculationToken,
    kind: CandidateKind,
) -> Result<Calculated<Reading>> {
    let _: Parameters = serde_json::from_value(ctx.params().clone())
        .map_err(|e| PluginError::Message(e.to_string()))?;
    let candidate = ctx.candidate();
    let matching = if kind == CandidateKind::AtomicMeaning {
        "exact_observed_label"
    } else {
        "exact_directed_observed_relation"
    };
    let pattern = candidate.value.get("pattern");
    if candidate.kind != kind
        || pattern
            .and_then(|p| p.get("matching"))
            .and_then(|v| v.as_str())
            != Some(matching)
    {
        return Ok(token.emit(Reading::NotApplicable {
            reason: format!("requires {matching} candidate evidence"),
        }));
    }
    let pattern = pattern.unwrap();
    // Validate supported candidate shapes even when the baseline is unavailable.
    let labels = if kind == CandidateKind::AtomicMeaning {
        vec![label(pattern, "observed_label")?]
    } else {
        vec![
            label(pattern, "source_label")?,
            label(pattern, "target_label")?,
            label(pattern, "relation_kind")?,
        ]
    };
    let Some(domain) = ctx.domain() else {
        return Ok(token.emit(Reading::InsufficientEvidence { have: 0, need: 1 }));
    };
    validate(domain)?;
    let key = serde_json::to_string(&(&domain.id.0, &domain.version.0))
        .map_err(|e| PluginError::Message(e.to_string()))?;
    if candidate.domain_version_id != key {
        return Err(invalid(
            "null baseline differs from candidate domain version",
        ));
    }
    let mut matches = Vec::new();
    if kind == CandidateKind::AtomicMeaning {
        for unit in domain.units.values() {
            if unit.kind == UnitKind::AtomicMeaning && unit.label.as_deref() == Some(labels[0]) {
                matches
                    .push(serde_json::json!({"unit":unit.id,"kind":unit.kind,"label":unit.label}));
            }
        }
    } else {
        for relation in domain.relations.values() {
            let source = &domain.units[&relation.source];
            let target = &domain.units[&relation.target];
            if relation.kind == labels[2]
                && source.label.as_deref() == Some(labels[0])
                && target.label.as_deref() == Some(labels[1])
            {
                matches.push(serde_json::json!({"relation":relation.id,"source":relation.source,"target":relation.target,"kind":relation.kind}));
            }
        }
    }
    Ok(token.emit(Reading::Value {value:MeasurementValue::Structured(serde_json::json!({
        "model":"existing_domain_exact_match","domain_version_id":key,"matching":matching,
        "match_count":matches.len(),"matches":matches,"has_existing_alternative":!matches.is_empty(),
        "scope":"case-sensitive label and kind matching only; does not establish semantic equivalence or explanatory adequacy",
        "decision":"no automatic candidate acceptance or rejection"
    }))}))
}
