//! Temporary candidate application; no repository writes or domain promotion.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use unclip_domain::{
    CandidateKind, CandidateProposal, DomainSnapshot, PropertyValue, RelationId, Unit, UnitId,
    UnitKind,
};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, DomainVersion,
    EmitMetadata, PluginId, Timestamp, Tracked,
};
use unclip_plugin::{PluginError, Result};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CounterfactualSnapshot {
    pub baseline_domain_version_id: String,
    pub candidate: DerivedId,
    pub added_units: Vec<UnitId>,
    pub property_changes: Vec<PropertyChange>,
    pub domain: DomainSnapshot,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PropertyTarget {
    Unit { id: UnitId },
    Relation { id: RelationId },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyChange {
    pub target: PropertyTarget,
    pub property: String,
    pub before: PropertyValue,
    pub after: PropertyValue,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WeightPattern {
    matching: String,
    target: PropertyTarget,
    property: String,
    proposed_value: serde_json::Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AtomicPattern {
    matching: String,
    observed_label: String,
}
fn invalid(s: impl ToString) -> PluginError {
    PluginError::Message(s.to_string())
}
impl super::Engine {
    /// Apply a supported proposal to a clone. The caller supplies a unique run ID.
    /// This prepares a counterfactual; it does not accept or persist the candidate.
    pub fn apply_candidate(
        &self,
        baseline: &Tracked<DomainSnapshot>,
        candidate: &Tracked<CandidateProposal>,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<CounterfactualSnapshot>> {
        if run_id.trim().is_empty()
            || baseline.id().0.is_empty()
            || candidate.id().0.is_empty()
            || baseline.id() == candidate.id()
        {
            return Err(invalid(
                "candidate application requires distinct nonempty input IDs and a run ID",
            ));
        }
        let output_id = DerivedId::new(format!("{run_id}/counterfactual"));
        if &output_id == baseline.id() || &output_id == candidate.id() {
            return Err(invalid(
                "counterfactual output identity collides with input evidence",
            ));
        }
        let dependencies = DependencyCollector::default();
        let domain = dependencies.read(baseline);
        let proposal = dependencies.read(candidate);
        super::domain_null::validate(domain)?;
        let baseline_key =
            serde_json::to_string(&(&domain.id.0, &domain.version.0)).map_err(invalid)?;
        if proposal.domain_version_id != baseline_key {
            return Err(invalid(
                "candidate application baseline differs from proposal domain version",
            ));
        }
        let pattern_value = proposal
            .value
            .get("pattern")
            .ok_or_else(|| invalid("candidate requires a pattern"))?;
        let version = DomainVersion::new(format!("counterfactual:{run_id}"));
        if version == domain.version {
            return Err(invalid(
                "temporary domain version must differ from baseline",
            ));
        }
        let mut temporary = domain.clone();
        temporary.version = version.clone();
        let mut added_units = Vec::new();
        let mut property_changes = Vec::new();
        match proposal.kind {
            CandidateKind::AtomicMeaning => {
                let pattern: AtomicPattern = serde_json::from_value(pattern_value.clone()).map_err(invalid)?;
                if pattern.matching != "exact_observed_label" || pattern.observed_label.trim().is_empty() { return Err(invalid("atomic application requires a nonempty exact observed-label pattern")); }
                let unit_id = UnitId::new(format!("candidate:{}", candidate.id().0));
                if domain.units.contains_key(&unit_id) { return Err(invalid("candidate unit identity already exists in the baseline")); }
                temporary.units.insert(unit_id.clone(), Unit {
                    id: unit_id.clone(), kind: UnitKind::AtomicMeaning, label: None,
                    properties: BTreeMap::from([
                        ("candidate_id".into(), PropertyValue::Text(candidate.id().0.clone())),
                        ("candidate_pattern".into(), PropertyValue::Structured(pattern_value.clone())),
                        ("candidate_evidence".into(), PropertyValue::Structured(serde_json::Value::Object(proposal.value.clone()))),
                    ]),
                });
                added_units.push(unit_id);
            }
            CandidateKind::WeightRevision => {
                let pattern: WeightPattern = serde_json::from_value(pattern_value.clone()).map_err(invalid)?;
                if pattern.matching != "numeric_property_revision" || pattern.property.trim().is_empty() { return Err(invalid("weight application requires an explicit numeric property revision")); }
                if pattern.proposed_value.as_u64().is_some_and(|v| v > 9_007_199_254_740_992) { return Err(invalid("proposed integer weight exceeds exact numeric range")); }
                let proposed: PropertyValue = serde_json::from_value(pattern.proposed_value).map_err(invalid)?;
                super::weight_null::numeric(&proposed)?;
                let properties = match &pattern.target {
                    PropertyTarget::Unit { id } => &mut temporary.units.get_mut(id).ok_or_else(|| invalid("weight target unit does not exist"))?.properties,
                    PropertyTarget::Relation { id } => &mut temporary.relations.get_mut(id).ok_or_else(|| invalid("weight target relation does not exist"))?.properties,
                };
                let previous = properties.get(&pattern.property).ok_or_else(|| invalid("weight property does not exist in baseline"))?.clone();
                let before = super::weight_null::numeric(&previous)?;
                let after = super::weight_null::numeric(&proposed)?;
                if !(after - before).is_finite() { return Err(invalid("weight change exceeds finite numeric range")); }
                properties.insert(pattern.property.clone(), proposed.clone());
                property_changes.push(PropertyChange { target: pattern.target, property: pattern.property, before: previous, after: proposed });
            }
            _ => return Err(invalid("candidate application supports atomic observed-label and numeric-property weight proposals only")),
        }
        let params = serde_json::json!({"baseline_domain_version_id":baseline_key,"candidate":candidate.id(),"temporary_version":version,"added_units":added_units,"property_changes":property_changes,"application_kind":proposal.kind});
        let token = CalculationToken::from_harness(
            EmitMetadata {
                id: output_id,
                producer: PluginId::new("experiment.apply-candidate"),
                algorithm: "temporary_candidate_application".into(),
                version: semver::Version::new(0, 2, 0),
                params_hash: hash_params(&params),
                params,
                source: None,
                timestamp,
                domain_version: Some(version),
                frame_version: None,
                model: None,
            },
            dependencies,
        );
        Ok(token.emit(CounterfactualSnapshot {
            baseline_domain_version_id: baseline_key,
            candidate: candidate.id().clone(),
            added_units,
            property_changes,
            domain: temporary,
        }))
    }
}
