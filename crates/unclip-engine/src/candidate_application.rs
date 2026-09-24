//! Temporary candidate application; no repository writes or domain promotion.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use unclip_domain::{
    CandidateKind, CandidateProposal, DomainSnapshot, PropertyValue, Relation, RelationId, Unit,
    UnitId, UnitKind,
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
    pub added_relations: Vec<RelationId>,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationBindings {
    pub source: UnitId,
    pub target: UnitId,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelationPattern {
    matching: String,
    source_label: String,
    target_label: String,
    relation_kind: String,
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
struct CommunityPattern {
    matching: String,
    members: Vec<UnitId>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunityEvidence {
    structure: DerivedId,
    community_index: usize,
    result: unclip_measure::CommunityDetection,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunitySelection {
    metric: unclip_measure::PairwiseMetric,
    minimum_samples: usize,
    minimum_members: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LatentPattern {
    matching: String,
    units: Vec<UnitId>,
    eigenvalue: f64,
    loadings: Vec<f64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LatentEvidence {
    structure: DerivedId,
    eigenpair_index: usize,
    result: unclip_measure::SpectralDecomposition,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LatentSelection {
    metric: unclip_measure::PairwiseMetric,
    minimum_samples: usize,
    minimum_absolute_eigenvalue: f64,
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
        self.apply_candidate_with_relation_bindings(baseline, candidate, None, run_id, timestamp)
    }
    /// Directed relation proposals require explicitly bound existing endpoint IDs.
    pub fn apply_candidate_with_relation_bindings(
        &self,
        baseline: &Tracked<DomainSnapshot>,
        candidate: &Tracked<CandidateProposal>,
        bindings: Option<&RelationBindings>,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<CounterfactualSnapshot>> {
        self.apply_candidate_internal(baseline, candidate, bindings, None, run_id, timestamp)
    }

    pub(crate) fn apply_candidate_for_revision(
        &self,
        baseline: &Tracked<DomainSnapshot>,
        candidate: &Tracked<CandidateProposal>,
        revision_step: super::RevisionStep,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<CounterfactualSnapshot>> {
        self.apply_candidate_internal(
            baseline,
            candidate,
            None,
            Some(revision_step),
            run_id,
            timestamp,
        )
    }

    fn apply_candidate_internal(
        &self,
        baseline: &Tracked<DomainSnapshot>,
        candidate: &Tracked<CandidateProposal>,
        bindings: Option<&RelationBindings>,
        revision_step: Option<super::RevisionStep>,
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
        if bindings.is_some() && proposal.kind != CandidateKind::Relation {
            return Err(invalid(
                "relation bindings are only valid for relation candidates",
            ));
        }
        let mut added_units = Vec::new();
        let mut added_relations = Vec::new();
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
            CandidateKind::CompositeMeaning => {
                let pattern: CommunityPattern = serde_json::from_value(pattern_value.clone()).map_err(invalid)?;
                if pattern.matching != "empirical_community" || pattern.members.len() < 2 || pattern.members.windows(2).any(|pair| pair[0] >= pair[1]) { return Err(invalid("community application requires at least two ordered unique members")); }
                if pattern.members.iter().any(|id| !domain.units.contains_key(id)) { return Err(invalid("community member does not exist in baseline")); }
                let evidence: CommunityEvidence = serde_json::from_value(proposal.value.get("evidence").ok_or_else(|| invalid("community candidate requires evidence"))?.clone()).map_err(invalid)?;
                let selection: CommunitySelection = serde_json::from_value(proposal.value.get("selection").ok_or_else(|| invalid("community candidate requires selection parameters"))?.clone()).map_err(invalid)?;
                super::structure_discovery::validate_community(&evidence.result)?;
                if evidence.structure.0.is_empty() || evidence.result.communities.get(evidence.community_index) != Some(&pattern.members) || selection.minimum_samples < 2 || selection.minimum_members < 2 || evidence.result.metric != selection.metric || evidence.result.minimum_samples.get() < selection.minimum_samples || pattern.members.len() < selection.minimum_members { return Err(invalid("community candidate pattern and selection conflict with recorded evidence")); }
                let unit_id = UnitId::new(format!("candidate:{}", candidate.id().0));
                if domain.units.contains_key(&unit_id) { return Err(invalid("candidate unit identity already exists in the baseline")); }
                temporary.units.insert(unit_id.clone(), Unit {
                    id: unit_id.clone(), kind: UnitKind::CompositeMeaning, label: None,
                    properties: BTreeMap::from([
                        ("candidate_id".into(), PropertyValue::Text(candidate.id().0.clone())),
                        ("candidate_pattern".into(), PropertyValue::Structured(pattern_value.clone())),
                        ("candidate_evidence".into(), PropertyValue::Structured(serde_json::Value::Object(proposal.value.clone()))),
                        ("members".into(), PropertyValue::Structured(serde_json::to_value(&pattern.members).map_err(invalid)?)),
                    ]),
                });
                added_units.push(unit_id);
            }
            CandidateKind::LatentAxis => {
                let pattern: LatentPattern = serde_json::from_value(pattern_value.clone()).map_err(invalid)?;
                if pattern.matching != "empirical_spectral_axis" || pattern.units.len() < 2 || pattern.units.iter().any(|id| !domain.units.contains_key(id)) { return Err(invalid("latent application requires at least two existing baseline units")); }
                let evidence: LatentEvidence = serde_json::from_value(proposal.value.get("evidence").ok_or_else(|| invalid("latent candidate requires evidence"))?.clone()).map_err(invalid)?;
                let selection: LatentSelection = serde_json::from_value(proposal.value.get("selection").ok_or_else(|| invalid("latent candidate requires selection parameters"))?.clone()).map_err(invalid)?;
                super::structure_discovery::validate_spectral(&evidence.result)?;
                let pair = evidence.result.eigenpairs.get(evidence.eigenpair_index).ok_or_else(|| invalid("latent eigenpair index is out of range"))?;
                if evidence.structure.0.is_empty() || pattern.units != evidence.result.units || pattern.eigenvalue != pair.eigenvalue || pattern.loadings != pair.loadings || selection.metric != evidence.result.metric || selection.minimum_samples < 2 || evidence.result.minimum_cell_samples < selection.minimum_samples || !selection.minimum_absolute_eigenvalue.is_finite() || selection.minimum_absolute_eigenvalue <= 0.0 || pattern.eigenvalue.abs() < selection.minimum_absolute_eigenvalue { return Err(invalid("latent pattern and selection conflict with recorded spectral evidence")); }
                let unit_id = UnitId::new(format!("candidate:{}", candidate.id().0));
                if domain.units.contains_key(&unit_id) { return Err(invalid("candidate unit identity already exists in the baseline")); }
                temporary.units.insert(unit_id.clone(), Unit {
                    id: unit_id.clone(), kind: UnitKind::LatentAxis, label: None,
                    properties: BTreeMap::from([
                        ("candidate_id".into(), PropertyValue::Text(candidate.id().0.clone())),
                        ("candidate_pattern".into(), PropertyValue::Structured(pattern_value.clone())),
                        ("candidate_evidence".into(), PropertyValue::Structured(serde_json::Value::Object(proposal.value.clone()))),
                        ("units".into(), PropertyValue::Structured(serde_json::to_value(&pattern.units).map_err(invalid)?)),
                        ("eigenvalue".into(), PropertyValue::Number(pattern.eigenvalue)),
                        ("loadings".into(), PropertyValue::Structured(serde_json::to_value(&pattern.loadings).map_err(invalid)?)),
                    ]),
                });
                added_units.push(unit_id);
            }
            CandidateKind::DynamicCoupling => {
                let coupling_units = super::coupling_application::validate(proposal, domain)?;
                let unit_id = UnitId::new(format!("candidate:{}", candidate.id().0));
                if domain.units.contains_key(&unit_id) { return Err(invalid("candidate unit identity already exists in the baseline")); }
                temporary.units.insert(unit_id.clone(), Unit {
                    id: unit_id.clone(), kind: UnitKind::DynamicCoupling, label: None,
                    properties: BTreeMap::from([
                        ("candidate_id".into(), PropertyValue::Text(candidate.id().0.clone())),
                        ("candidate_pattern".into(), PropertyValue::Structured(pattern_value.clone())),
                        ("candidate_evidence".into(), PropertyValue::Structured(serde_json::Value::Object(proposal.value.clone()))),
                        ("units".into(), PropertyValue::Structured(serde_json::to_value(&coupling_units).map_err(invalid)?)),
                        ("causal_claim".into(), PropertyValue::Boolean(false)),
                    ]),
                });
                added_units.push(unit_id);
            }
            CandidateKind::GraphMotif => {
                super::motif_application::validate(proposal)?;
                let unit_id = UnitId::new(format!("candidate:{}", candidate.id().0));
                if domain.units.contains_key(&unit_id) { return Err(invalid("candidate unit identity already exists in the baseline")); }
                temporary.units.insert(unit_id.clone(), Unit {
                    id: unit_id.clone(), kind: UnitKind::GraphMotif, label: None,
                    properties: BTreeMap::from([
                        ("candidate_id".into(), PropertyValue::Text(candidate.id().0.clone())),
                        ("candidate_pattern".into(), PropertyValue::Structured(pattern_value.clone())),
                        ("candidate_evidence".into(), PropertyValue::Structured(serde_json::Value::Object(proposal.value.clone()))),
                        ("graph_pattern".into(), PropertyValue::Structured(pattern_value.clone())),
                    ]),
                });
                added_units.push(unit_id);
            }
            CandidateKind::Relation => {
                let pattern: RelationPattern = serde_json::from_value(pattern_value.clone()).map_err(invalid)?;
                if pattern.matching != "exact_directed_observed_relation" || pattern.source_label.trim().is_empty() || pattern.target_label.trim().is_empty() || pattern.relation_kind.trim().is_empty() { return Err(invalid("relation application requires an exact directed observed-label pattern")); }
                let endpoints = bindings.ok_or_else(|| invalid("relation application requires explicit endpoint bindings"))?;
                let source = domain.units.get(&endpoints.source).ok_or_else(|| invalid("bound source unit does not exist"))?;
                let target = domain.units.get(&endpoints.target).ok_or_else(|| invalid("bound target unit does not exist"))?;
                if source.label.as_deref() != Some(&pattern.source_label) || target.label.as_deref() != Some(&pattern.target_label) { return Err(invalid("bound endpoint labels differ from candidate evidence")); }
                if domain.relations.values().any(|relation| relation.source == endpoints.source && relation.target == endpoints.target && relation.kind == pattern.relation_kind) { return Err(invalid("directed relation already exists in baseline")); }
                let id = RelationId::new(format!("candidate:{}", candidate.id().0));
                if domain.relations.contains_key(&id) { return Err(invalid("candidate relation identity already exists in baseline")); }
                temporary.relations.insert(id.clone(), Relation {
                    id: id.clone(), source: endpoints.source.clone(), target: endpoints.target.clone(), kind: pattern.relation_kind,
                    properties: BTreeMap::from([
                        ("candidate_id".into(), PropertyValue::Text(candidate.id().0.clone())),
                        ("candidate_pattern".into(), PropertyValue::Structured(pattern_value.clone())),
                        ("candidate_evidence".into(), PropertyValue::Structured(serde_json::Value::Object(proposal.value.clone()))),
                    ]),
                });
                added_relations.push(id);
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
            _ => return Err(invalid("candidate application supports atomic, recurring-motif, empirical-community, latent-axis, pairwise or temporal coupling, explicitly bound relation, and numeric-property weight proposals only")),
        }
        let mut params = serde_json::json!({"baseline_domain_version_id":baseline_key,"candidate":candidate.id(),"temporary_version":version,"added_units":added_units,"added_relations":added_relations,"relation_bindings":bindings,"property_changes":property_changes,"application_kind":proposal.kind});
        if let Some(step) = revision_step {
            params["revision_step"] = serde_json::to_value(step).map_err(invalid)?;
        }
        let token = CalculationToken::from_harness(
            EmitMetadata {
                id: output_id,
                producer: PluginId::new("experiment.apply-candidate"),
                algorithm: "temporary_candidate_application".into(),
                version: semver::Version::new(0, 8, 0),
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
            added_relations,
            property_changes,
            domain: temporary,
        }))
    }
}
