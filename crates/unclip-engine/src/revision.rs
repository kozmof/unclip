//! Ordered minimal-revision tests without scalarizing experiment evidence.

use serde::{Deserialize, Serialize};
use unclip_domain::{CandidateKind, CandidateProposal};
use unclip_epistemic::{
    hash_params, DependencyCollector, DerivedId, EmitMetadata, ExperimentToken, Experimental,
    PluginId, Timestamp, Tracked,
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
    pub candidate: DerivedId,
    pub counterfactual: DerivedId,
    pub experiment: DerivedId,
    pub outcome: RevisionTestOutcome,
    pub reason: String,
}

fn invalid(message: impl Into<String>) -> PluginError {
    PluginError::Message(message.into())
}

fn has_weight_retention_null(evidence: &CounterfactualEvidence) -> bool {
    evidence.null_results.iter().any(|result| {
        result.model == PluginId::new("null.weight-change")
            && matches!(
                &result.reading,
                Reading::Value {
                    value: MeasurementValue::Structured(value)
                } if value.get("model").and_then(serde_json::Value::as_str)
                    == Some("retain_existing_numeric_property")
            )
    })
}

impl crate::Engine {
    /// Record the first minimal-revision step after a held-out counterfactual test.
    ///
    /// The caller supplies the explicit sufficiency verdict and reason. Typed
    /// deltas, null evidence, Pareto relations, and constraints stay separate;
    /// this method never turns them into a score. Later ladder steps must consume
    /// an insufficient earlier attempt before they can be tested.
    #[allow(clippy::too_many_arguments)]
    pub fn record_delta_w_test(
        &self,
        candidate: &Tracked<CandidateProposal>,
        counterfactual: &unclip_epistemic::Calculated<CounterfactualSnapshot>,
        experiment: &Experimental<CounterfactualEvidence>,
        outcome: RevisionTestOutcome,
        reason: &str,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Experimental<RevisionAttempt>> {
        crate::require_calculated_evidence(candidate, "revision candidate")?;
        let reason = reason.trim();
        if run_id.trim().is_empty() || reason.is_empty() {
            return Err(invalid(
                "Delta W tests require a run identity and an explicit reason",
            ));
        }
        if candidate.id().0.trim().is_empty()
            || counterfactual.id().0.trim().is_empty()
            || experiment.id().0.trim().is_empty()
        {
            return Err(invalid("Delta W evidence identities must be nonempty"));
        }
        let dependencies = DependencyCollector::default();
        let proposal = dependencies.read(candidate).clone();
        if proposal.kind != CandidateKind::WeightRevision {
            return Err(invalid(
                "Delta W can test only an explicit numeric-property weight revision",
            ));
        }
        let snapshot = counterfactual.value();
        let evidence = experiment.value();
        if snapshot.candidate != *candidate.id()
            || evidence.candidate != *candidate.id()
            || evidence.counterfactual != *counterfactual.id()
            || snapshot.baseline_domain_version_id != proposal.domain_version_id
        {
            return Err(invalid(
                "Delta W candidate, counterfactual, experiment, and baseline must match",
            ));
        }
        if !counterfactual.provenance().inputs.contains(candidate.id())
            || !counterfactual
                .provenance()
                .inputs
                .contains(&evidence.baseline)
            || !experiment.provenance().inputs.contains(candidate.id())
            || !experiment.provenance().inputs.contains(counterfactual.id())
        {
            return Err(invalid(
                "Delta W evidence must track the candidate and counterfactual dependencies",
            ));
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
                return Err(invalid(
                    "Delta W experiment must track its baseline, frame, split, measurements, comparison, and null evidence",
                ));
            }
        }
        if !snapshot.added_units.is_empty()
            || !snapshot.added_relations.is_empty()
            || snapshot.property_changes.len() != 1
        {
            return Err(invalid(
                "Delta W must change exactly one existing numeric property",
            ));
        }
        if evidence.before.is_empty()
            || evidence.after.is_empty()
            || evidence.delta_profile.deltas.is_empty()
        {
            return Err(invalid(
                "Delta W requires completed before/after measurements and typed comparison deltas",
            ));
        }
        if !has_weight_retention_null(evidence) {
            return Err(invalid(
                "Delta W requires a measured null.weight-change result",
            ));
        }
        if outcome == RevisionTestOutcome::Sufficient
            && evidence
                .constraints
                .iter()
                .any(|constraint| constraint.status != ConstraintStatus::Satisfied)
        {
            return Err(invalid(
                "Delta W cannot be sufficient while an explicit constraint is unsatisfied",
            ));
        }

        let output_id = DerivedId::new(format!("{run_id}/revision/delta-w"));
        if [candidate.id(), counterfactual.id(), experiment.id()].contains(&&output_id) {
            return Err(invalid(
                "Delta W output identity collides with its evidence",
            ));
        }
        dependencies.read(&Tracked::from(counterfactual));
        dependencies.read(&Tracked::from(experiment));
        let params = serde_json::json!({
            "step": RevisionStep::DeltaW,
            "outcome": outcome,
            "reason": reason,
        });
        let token = ExperimentToken::from_harness(
            EmitMetadata {
                id: output_id,
                producer: PluginId::new("experiment.revision-ladder"),
                algorithm: "minimal_revision_delta_w".into(),
                version: semver::Version::new(0, 1, 0),
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
        let attempt = token.emit(RevisionAttempt {
            step: RevisionStep::DeltaW,
            candidate: candidate.id().clone(),
            counterfactual: counterfactual.id().clone(),
            experiment: experiment.id().clone(),
            outcome,
            reason: reason.into(),
        });
        Ok(attempt)
    }
}
