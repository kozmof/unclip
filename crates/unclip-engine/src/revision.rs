//! Ordered minimal-revision tests without scalarizing experiment evidence.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use unclip_domain::{CandidateKind, CandidateProposal};
use unclip_epistemic::{
    hash_params, Calculated, DependencyCollector, DerivedId, EmitMetadata, ExperimentToken,
    Experimental, PluginId, Timestamp, Tracked,
};
use unclip_measure::{MeasurementValue, Reading};
use unclip_plugin::{PluginError, Result};

use crate::{ConstraintStatus, CounterfactualEvidence, CounterfactualSnapshot};

/// A step in the required smallest-to-largest domain-revision order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionStep {
    DeltaW,
    DeltaE,
    DynamicCoupling,
    Structural,
    DeltaV,
}

/// The explicit result of testing one ladder step against held-out evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionTestOutcome {
    Sufficient,
    Insufficient,
}

/// Experimental record of one tested minimal-revision step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionAttempt {
    pub step: RevisionStep,
    #[serde(default)]
    pub prior: Option<DerivedId>,
    pub candidate: DerivedId,
    pub counterfactual: DerivedId,
    pub experiment: DerivedId,
    pub baseline: DerivedId,
    pub frame: DerivedId,
    pub split: DerivedId,
    pub outcome: RevisionTestOutcome,
    pub reason: String,
}

fn invalid(message: impl Into<String>) -> PluginError {
    PluginError::Message(message.into())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AtomicRevisionEvidence {
    pattern: AtomicRevisionPattern,
    observation_count: usize,
    observations: Vec<unclip_observe::ObservationId>,
    examples: Vec<AtomicRevisionExample>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AtomicRevisionPattern {
    matching: String,
    observed_label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AtomicRevisionExample {
    observation: unclip_observe::ObservationId,
    unit: unclip_observe::ObservedUnitId,
    measurements: Vec<DerivedId>,
}

fn validate_atomic_revision(proposal: &CandidateProposal) -> Result<&serde_json::Value> {
    let evidence: AtomicRevisionEvidence =
        serde_json::from_value(serde_json::Value::Object(proposal.value.clone()))
            .map_err(|error| invalid(error.to_string()))?;
    if evidence.pattern.matching != "exact_observed_label"
        || evidence.pattern.observed_label.trim().is_empty()
        || evidence.observation_count < 2
        || evidence.observation_count != evidence.observations.len()
        || evidence
            .observations
            .iter()
            .any(|observation| observation.0.trim().is_empty())
        || evidence
            .observations
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || evidence.examples.len() < evidence.observation_count
    {
        return Err(invalid(
            "Delta V requires a nonempty exact-label pattern supported by at least two ordered distinct observations",
        ));
    }
    let selected = evidence.observations.iter().collect::<BTreeSet<_>>();
    let mut covered = BTreeSet::new();
    let mut examples = BTreeSet::new();
    for example in &evidence.examples {
        if !selected.contains(&example.observation)
            || example.unit.0.trim().is_empty()
            || example.measurements.is_empty()
            || example
                .measurements
                .iter()
                .any(|measurement| measurement.0.trim().is_empty())
            || example
                .measurements
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || !examples.insert((&example.observation, &example.unit))
        {
            return Err(invalid(
                "Delta V residual examples must be unique, selected, and retain ordered measurement evidence",
            ));
        }
        covered.insert(&example.observation);
    }
    if covered != selected {
        return Err(invalid(
            "Delta V residual examples must cover every supporting observation",
        ));
    }
    Ok(proposal
        .value
        .get("pattern")
        .expect("validated atomic revision pattern"))
}

fn has_measured_null(evidence: &CounterfactualEvidence, plugin: &str, model: &str) -> bool {
    evidence.null_results.iter().any(|result| {
        result.model == PluginId::new(plugin)
            && matches!(
                &result.reading,
                Reading::Value {
                    value: MeasurementValue::Structured(value)
                } if value.get("model").and_then(serde_json::Value::as_str) == Some(model)
            )
    })
}

fn validate_experiment<'a>(
    step: &str,
    candidate: &'a Tracked<CandidateProposal>,
    counterfactual: &Calculated<CounterfactualSnapshot>,
    experiment: &Experimental<CounterfactualEvidence>,
    outcome: RevisionTestOutcome,
    reason: &str,
    run_id: &str,
) -> Result<&'a CandidateProposal> {
    crate::require_calculated_evidence(candidate, "revision candidate")?;
    if run_id.trim().is_empty() || reason.is_empty() {
        return Err(invalid(format!(
            "{step} tests require a run identity and an explicit reason"
        )));
    }
    if candidate.id().0.trim().is_empty()
        || counterfactual.id().0.trim().is_empty()
        || experiment.id().0.trim().is_empty()
    {
        return Err(invalid(format!(
            "{step} evidence identities must be nonempty"
        )));
    }
    let proposal = DependencyCollector::default().read(candidate);
    let snapshot = counterfactual.value();
    let evidence = experiment.value();
    if snapshot.candidate != *candidate.id()
        || evidence.candidate != *candidate.id()
        || evidence.counterfactual != *counterfactual.id()
        || snapshot.baseline_domain_version_id != proposal.domain_version_id
    {
        return Err(invalid(format!(
            "{step} candidate, counterfactual, experiment, and baseline must match"
        )));
    }
    if !counterfactual.provenance().inputs.contains(candidate.id())
        || !counterfactual
            .provenance()
            .inputs
            .contains(&evidence.baseline)
        || !experiment.provenance().inputs.contains(candidate.id())
        || !experiment.provenance().inputs.contains(counterfactual.id())
    {
        return Err(invalid(format!(
            "{step} evidence must track the candidate and counterfactual dependencies"
        )));
    }
    let experiment_inputs = &experiment.provenance().inputs;
    for required in [
        &evidence.baseline,
        &evidence.frame,
        &evidence.split,
        &evidence.comparison,
    ]
    .into_iter()
    .chain(evidence.before.iter())
    .chain(evidence.after.iter())
    .chain(evidence.null_results.iter().map(|result| &result.id))
    {
        if required.0.trim().is_empty() || !experiment_inputs.contains(required) {
            return Err(invalid(format!(
                "{step} experiment must track its baseline, frame, split, measurements, comparison, and null evidence"
            )));
        }
    }
    if evidence.before.is_empty()
        || evidence.after.is_empty()
        || evidence.delta_profile.deltas.is_empty()
    {
        return Err(invalid(format!(
            "{step} requires completed before/after measurements and typed comparison deltas"
        )));
    }
    for delta in &evidence.delta_profile.deltas {
        if delta.id.0.trim().is_empty()
            || !experiment_inputs.contains(&delta.id)
            || !evidence.before.contains(&delta.pair.before)
            || !evidence.after.contains(&delta.pair.after)
            || !evidence.delta_profile.pairs.contains(&delta.pair)
        {
            return Err(invalid(format!(
                "{step} comparison deltas must track explicitly selected before and after measurements"
            )));
        }
    }
    if outcome == RevisionTestOutcome::Sufficient
        && evidence
            .constraints
            .iter()
            .any(|constraint| constraint.status != ConstraintStatus::Satisfied)
    {
        return Err(invalid(format!(
            "{step} cannot be sufficient while an explicit constraint is unsatisfied"
        )));
    }
    Ok(proposal)
}

fn validate_prior(
    prior: &Experimental<RevisionAttempt>,
    expected: RevisionStep,
    experiment: &Experimental<CounterfactualEvidence>,
) -> Result<()> {
    let value = prior.value();
    if value.step != expected || value.outcome != RevisionTestOutcome::Insufficient {
        return Err(invalid(format!(
            "the prior {expected:?} step must be recorded as insufficient"
        )));
    }
    if value.reason.trim().is_empty()
        || !prior.provenance().inputs.contains(&value.candidate)
        || !prior.provenance().inputs.contains(&value.counterfactual)
        || !prior.provenance().inputs.contains(&value.experiment)
    {
        return Err(invalid(
            "the prior revision attempt must retain its reason and evidence dependencies",
        ));
    }
    if value.baseline != experiment.value().baseline
        || value.frame != experiment.value().frame
        || value.split != experiment.value().split
        || prior.provenance().domain_version != experiment.provenance().domain_version
        || prior.provenance().frame_version != experiment.provenance().frame_version
    {
        return Err(invalid(
            "ordered revision attempts must use the same domain and frame context",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_attempt(
    candidate: &Tracked<CandidateProposal>,
    counterfactual: &Calculated<CounterfactualSnapshot>,
    experiment: &Experimental<CounterfactualEvidence>,
    prior: Option<&Experimental<RevisionAttempt>>,
    step: RevisionStep,
    outcome: RevisionTestOutcome,
    reason: &str,
    run_id: &str,
    timestamp: Timestamp,
) -> Result<Experimental<RevisionAttempt>> {
    let (slug, algorithm, version) = match step {
        RevisionStep::DeltaW => (
            "delta-w",
            "minimal_revision_delta_w",
            semver::Version::new(0, 1, 0),
        ),
        RevisionStep::DeltaE => (
            "delta-e",
            "minimal_revision_delta_e",
            semver::Version::new(0, 1, 0),
        ),
        RevisionStep::DynamicCoupling => (
            "dynamic-coupling",
            "minimal_revision_dynamic_coupling",
            semver::Version::new(0, 1, 0),
        ),
        RevisionStep::Structural => (
            "structural",
            "minimal_revision_structural",
            semver::Version::new(0, 1, 0),
        ),
        RevisionStep::DeltaV => (
            "delta-v",
            "minimal_revision_delta_v",
            semver::Version::new(0, 1, 0),
        ),
    };
    let output_id = DerivedId::new(format!("{run_id}/revision/{slug}"));
    let mut inputs = vec![candidate.id(), counterfactual.id(), experiment.id()];
    if let Some(prior) = prior {
        inputs.push(prior.id());
    }
    if inputs.contains(&&output_id) {
        return Err(invalid(format!(
            "{step:?} output identity collides with its evidence"
        )));
    }
    let dependencies = DependencyCollector::default();
    dependencies.read(candidate);
    dependencies.read(&Tracked::from(counterfactual));
    dependencies.read(&Tracked::from(experiment));
    if let Some(prior) = prior {
        dependencies.read(&Tracked::from(prior));
    }
    let prior_id = prior.map(|attempt| attempt.id().clone());
    let params = serde_json::json!({
        "step": step,
        "prior": prior_id,
        "outcome": outcome,
        "reason": reason,
    });
    let token = ExperimentToken::from_harness(
        EmitMetadata {
            id: output_id,
            producer: PluginId::new("experiment.revision-ladder"),
            algorithm: algorithm.into(),
            version,
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp,
            domain_version: experiment.provenance().domain_version.clone(),
            frame_version: experiment.provenance().frame_version.clone(),
            model: None,
        },
        dependencies,
    );
    Ok(token.emit(RevisionAttempt {
        step,
        prior: prior_id,
        candidate: candidate.id().clone(),
        counterfactual: counterfactual.id().clone(),
        experiment: experiment.id().clone(),
        baseline: experiment.value().baseline.clone(),
        frame: experiment.value().frame.clone(),
        split: experiment.value().split.clone(),
        outcome,
        reason: reason.into(),
    }))
}

impl crate::Engine {
    /// Create the Delta V counterfactual only after the structural rung failed.
    ///
    /// This gate runs before candidate application, so a sufficient structural
    /// revision cannot create an unnecessary atomic unit, even temporarily.
    pub fn apply_delta_v_candidate(
        &self,
        prior: &Experimental<RevisionAttempt>,
        baseline: &Tracked<unclip_domain::DomainSnapshot>,
        candidate: &Tracked<CandidateProposal>,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<CounterfactualSnapshot>> {
        let value = prior.value();
        let inputs = DependencyCollector::default();
        let domain = inputs.read(baseline);
        let proposal = inputs.read(candidate);
        if value.step != RevisionStep::Structural
            || value.outcome != RevisionTestOutcome::Insufficient
        {
            return Err(invalid(
                "Delta V application requires an insufficient structural attempt",
            ));
        }
        if value.reason.trim().is_empty()
            || !prior.provenance().inputs.contains(&value.candidate)
            || !prior.provenance().inputs.contains(&value.counterfactual)
            || !prior.provenance().inputs.contains(&value.experiment)
        {
            return Err(invalid(
                "the structural attempt must retain its reason and evidence dependencies",
            ));
        }
        if value.baseline != *baseline.id()
            || prior.provenance().domain_version.as_ref() != Some(&domain.version)
        {
            return Err(invalid(
                "Delta V application must use the structural attempt baseline",
            ));
        }
        if proposal.kind != CandidateKind::AtomicMeaning {
            return Err(invalid(
                "Delta V application requires an atomic-meaning candidate",
            ));
        }
        self.apply_candidate_for_revision(
            baseline,
            candidate,
            RevisionStep::DeltaV,
            run_id,
            timestamp,
        )
    }

    /// Record the first minimal-revision step after a held-out counterfactual test.
    ///
    /// The caller supplies the explicit sufficiency verdict and reason. Typed
    /// deltas, null evidence, Pareto relations, and constraints stay separate;
    /// this method never turns them into a score.
    #[allow(clippy::too_many_arguments)]
    pub fn record_delta_w_test(
        &self,
        candidate: &Tracked<CandidateProposal>,
        counterfactual: &Calculated<CounterfactualSnapshot>,
        experiment: &Experimental<CounterfactualEvidence>,
        outcome: RevisionTestOutcome,
        reason: &str,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Experimental<RevisionAttempt>> {
        let reason = reason.trim();
        let proposal = validate_experiment(
            "Delta W",
            candidate,
            counterfactual,
            experiment,
            outcome,
            reason,
            run_id,
        )?;
        if proposal.kind != CandidateKind::WeightRevision {
            return Err(invalid(
                "Delta W can test only an explicit numeric-property weight revision",
            ));
        }
        let snapshot = counterfactual.value();
        if !snapshot.added_units.is_empty()
            || !snapshot.added_relations.is_empty()
            || snapshot.property_changes.len() != 1
        {
            return Err(invalid(
                "Delta W must change exactly one existing numeric property",
            ));
        }
        if !has_measured_null(
            experiment.value(),
            "null.weight-change",
            "retain_existing_numeric_property",
        ) {
            return Err(invalid(
                "Delta W requires a measured null.weight-change result",
            ));
        }
        emit_attempt(
            candidate,
            counterfactual,
            experiment,
            None,
            RevisionStep::DeltaW,
            outcome,
            reason,
            run_id,
            timestamp,
        )
    }

    /// Record a relation revision test after Delta W was explicitly insufficient.
    ///
    /// The relation counterfactual must add exactly one explicitly bound edge and
    /// must compete with the existing-relation null. A sufficient earlier weight
    /// revision stops the ladder before this method can emit an attempt.
    #[allow(clippy::too_many_arguments)]
    pub fn record_delta_e_test(
        &self,
        prior: &Experimental<RevisionAttempt>,
        candidate: &Tracked<CandidateProposal>,
        counterfactual: &Calculated<CounterfactualSnapshot>,
        experiment: &Experimental<CounterfactualEvidence>,
        outcome: RevisionTestOutcome,
        reason: &str,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Experimental<RevisionAttempt>> {
        let reason = reason.trim();
        let proposal = validate_experiment(
            "Delta E",
            candidate,
            counterfactual,
            experiment,
            outcome,
            reason,
            run_id,
        )?;
        validate_prior(prior, RevisionStep::DeltaW, experiment)?;
        if proposal.kind != CandidateKind::Relation {
            return Err(invalid(
                "Delta E can test only an explicit relation revision",
            ));
        }
        let snapshot = counterfactual.value();
        if !snapshot.added_units.is_empty()
            || snapshot.added_relations.len() != 1
            || !snapshot.property_changes.is_empty()
        {
            return Err(invalid(
                "Delta E must add exactly one relation without changing units or properties",
            ));
        }
        let relation = snapshot
            .domain
            .relations
            .get(&snapshot.added_relations[0])
            .ok_or_else(|| invalid("Delta E added relation is absent from the counterfactual"))?;
        let bindings = counterfactual
            .provenance()
            .params
            .get("relation_bindings")
            .ok_or_else(|| invalid("Delta E requires explicit relation bindings"))?;
        if bindings.get("source").and_then(serde_json::Value::as_str) != Some(&relation.source.0)
            || bindings.get("target").and_then(serde_json::Value::as_str)
                != Some(&relation.target.0)
        {
            return Err(invalid(
                "Delta E relation endpoints must match the explicit bindings",
            ));
        }
        if !has_measured_null(
            experiment.value(),
            "null.existing-relation",
            "existing_domain_exact_match",
        ) {
            return Err(invalid(
                "Delta E requires a measured null.existing-relation result",
            ));
        }
        emit_attempt(
            candidate,
            counterfactual,
            experiment,
            Some(prior),
            RevisionStep::DeltaE,
            outcome,
            reason,
            run_id,
            timestamp,
        )
    }

    /// Record a dynamic-coupling test after Delta E was explicitly insufficient.
    ///
    /// The counterfactual must add exactly one anonymous, explicitly non-causal
    /// coupling and must compete with the zero-association baseline diagnostic.
    #[allow(clippy::too_many_arguments)]
    pub fn record_dynamic_coupling_test(
        &self,
        prior: &Experimental<RevisionAttempt>,
        candidate: &Tracked<CandidateProposal>,
        counterfactual: &Calculated<CounterfactualSnapshot>,
        experiment: &Experimental<CounterfactualEvidence>,
        outcome: RevisionTestOutcome,
        reason: &str,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Experimental<RevisionAttempt>> {
        let reason = reason.trim();
        let proposal = validate_experiment(
            "dynamic coupling",
            candidate,
            counterfactual,
            experiment,
            outcome,
            reason,
            run_id,
        )?;
        validate_prior(prior, RevisionStep::DeltaE, experiment)?;
        if proposal.kind != CandidateKind::DynamicCoupling {
            return Err(invalid(
                "dynamic coupling can test only an explicit coupling revision",
            ));
        }
        let matching = proposal
            .value
            .get("pattern")
            .and_then(|pattern| pattern.get("matching"))
            .and_then(serde_json::Value::as_str);
        if !matches!(
            matching,
            Some("thresholded_pairwise_association" | "lagged_directional_association")
        ) {
            return Err(invalid("dynamic coupling pattern is not supported"));
        }
        let snapshot = counterfactual.value();
        if snapshot.added_units.len() != 1
            || !snapshot.added_relations.is_empty()
            || !snapshot.property_changes.is_empty()
        {
            return Err(invalid(
                "dynamic coupling must add exactly one unit without changing relations or properties",
            ));
        }
        let unit = snapshot
            .domain
            .units
            .get(&snapshot.added_units[0])
            .ok_or_else(|| invalid("dynamic coupling unit is absent from the counterfactual"))?;
        if unit.kind != unclip_domain::UnitKind::DynamicCoupling
            || unit.label.is_some()
            || unit.properties.get("causal_claim")
                != Some(&unclip_domain::PropertyValue::Boolean(false))
            || unit.properties.get("candidate_evidence")
                != Some(&unclip_domain::PropertyValue::Structured(
                    serde_json::Value::Object(proposal.value.clone()),
                ))
        {
            return Err(invalid(
                "dynamic coupling must remain anonymous, non-causal, and retain its candidate evidence",
            ));
        }
        if !has_measured_null(
            experiment.value(),
            "null.coupling-zero",
            "zero_association_baseline",
        ) {
            return Err(invalid(
                "dynamic coupling requires a measured null.coupling-zero result",
            ));
        }
        emit_attempt(
            candidate,
            counterfactual,
            experiment,
            Some(prior),
            RevisionStep::DynamicCoupling,
            outcome,
            reason,
            run_id,
            timestamp,
        )
    }

    /// Record a graph-motif structural test after dynamic coupling was insufficient.
    ///
    /// Semantic-role and transformation candidates remain unavailable until they
    /// have explicit calculated evidence and application schemas.
    #[allow(clippy::too_many_arguments)]
    pub fn record_structural_test(
        &self,
        prior: &Experimental<RevisionAttempt>,
        candidate: &Tracked<CandidateProposal>,
        counterfactual: &Calculated<CounterfactualSnapshot>,
        experiment: &Experimental<CounterfactualEvidence>,
        outcome: RevisionTestOutcome,
        reason: &str,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Experimental<RevisionAttempt>> {
        let reason = reason.trim();
        let proposal = validate_experiment(
            "structural revision",
            candidate,
            counterfactual,
            experiment,
            outcome,
            reason,
            run_id,
        )?;
        validate_prior(prior, RevisionStep::DynamicCoupling, experiment)?;
        if proposal.kind != CandidateKind::GraphMotif {
            return Err(invalid(
                "structural revision currently supports only calculated graph-motif evidence",
            ));
        }
        let snapshot = counterfactual.value();
        if snapshot.added_units.len() != 1
            || !snapshot.added_relations.is_empty()
            || !snapshot.property_changes.is_empty()
        {
            return Err(invalid(
                "graph-motif revision must add exactly one unit without changing relations or properties",
            ));
        }
        let unit = snapshot
            .domain
            .units
            .get(&snapshot.added_units[0])
            .ok_or_else(|| invalid("graph-motif unit is absent from the counterfactual"))?;
        let pattern = proposal
            .value
            .get("pattern")
            .ok_or_else(|| invalid("graph-motif candidate requires a pattern"))?;
        if unit.kind != unclip_domain::UnitKind::GraphMotif
            || unit.label.is_some()
            || unit.properties.get("graph_pattern")
                != Some(&unclip_domain::PropertyValue::Structured(pattern.clone()))
            || unit.properties.get("candidate_evidence")
                != Some(&unclip_domain::PropertyValue::Structured(
                    serde_json::Value::Object(proposal.value.clone()),
                ))
        {
            return Err(invalid(
                "graph motif must remain anonymous and retain its exact pattern and candidate evidence",
            ));
        }
        if !has_measured_null(
            experiment.value(),
            "null.existing-motif",
            "existing_motif_exact_pattern",
        ) {
            return Err(invalid(
                "graph-motif revision requires a measured null.existing-motif result",
            ));
        }
        emit_attempt(
            candidate,
            counterfactual,
            experiment,
            Some(prior),
            RevisionStep::Structural,
            outcome,
            reason,
            run_id,
            timestamp,
        )
    }

    /// Record an atomic membership revision after structural change was insufficient.
    ///
    /// The new unit stays anonymous and retains the exact calculated residual
    /// evidence. Semantic naming remains a later interpretation operation.
    #[allow(clippy::too_many_arguments)]
    pub fn record_delta_v_test(
        &self,
        prior: &Experimental<RevisionAttempt>,
        candidate: &Tracked<CandidateProposal>,
        counterfactual: &Calculated<CounterfactualSnapshot>,
        experiment: &Experimental<CounterfactualEvidence>,
        outcome: RevisionTestOutcome,
        reason: &str,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Experimental<RevisionAttempt>> {
        let reason = reason.trim();
        let proposal = validate_experiment(
            "Delta V",
            candidate,
            counterfactual,
            experiment,
            outcome,
            reason,
            run_id,
        )?;
        validate_prior(prior, RevisionStep::Structural, experiment)?;
        if counterfactual
            .provenance()
            .params
            .get("revision_step")
            .and_then(serde_json::Value::as_str)
            != Some("delta_v")
        {
            return Err(invalid(
                "Delta V counterfactual must be created through the ordered application gate",
            ));
        }
        if proposal.kind != CandidateKind::AtomicMeaning {
            return Err(invalid(
                "Delta V currently supports only calculated atomic-meaning residual evidence",
            ));
        }
        let pattern = validate_atomic_revision(proposal)?;
        let snapshot = counterfactual.value();
        if snapshot.added_units.len() != 1
            || !snapshot.added_relations.is_empty()
            || !snapshot.property_changes.is_empty()
        {
            return Err(invalid(
                "Delta V must add exactly one unit without changing relations or properties",
            ));
        }
        let unit = snapshot
            .domain
            .units
            .get(&snapshot.added_units[0])
            .ok_or_else(|| invalid("Delta V unit is absent from the counterfactual"))?;
        if unit.kind != unclip_domain::UnitKind::AtomicMeaning
            || unit.label.is_some()
            || unit.properties.get("candidate_id")
                != Some(&unclip_domain::PropertyValue::Text(
                    candidate.id().0.clone(),
                ))
            || unit.properties.get("candidate_pattern")
                != Some(&unclip_domain::PropertyValue::Structured(pattern.clone()))
            || unit.properties.get("candidate_evidence")
                != Some(&unclip_domain::PropertyValue::Structured(
                    serde_json::Value::Object(proposal.value.clone()),
                ))
        {
            return Err(invalid(
                "Delta V unit must remain anonymous and retain its exact candidate identity, pattern, and evidence",
            ));
        }
        if !has_measured_null(
            experiment.value(),
            "null.existing-unit",
            "existing_domain_exact_match",
        ) {
            return Err(invalid(
                "Delta V requires a measured null.existing-unit result",
            ));
        }
        emit_attempt(
            candidate,
            counterfactual,
            experiment,
            Some(prior),
            RevisionStep::DeltaV,
            outcome,
            reason,
            run_id,
            timestamp,
        )
    }
}
