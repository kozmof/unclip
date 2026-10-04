//! Experimental aggregation of executed counterfactual measurements and comparisons.
use crate::constraints::assess_experiment_constraints;
use crate::nulls::models::evaluate_null_models_with_inputs;
use crate::pareto::compare_pareto;
use crate::run_record;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use unclip_epistemic::{
    Calculated, DependencyCollector, DerivedId, EmitMetadata, ExperimentToken, Experimental,
    PluginId, Tracked,
};
use unclip_plugin::{PluginError, Result, RunPlan};

/// Everything one counterfactual experiment concluded, as one recorded value.
///
/// The payloads this gathers — the delta profile, the null readings, the
/// constraint assessments, the Pareto assessment — are all also returned
/// alongside it on [`CounterfactualExperiment`], each still owned by the
/// derived value that produced it. They are therefore shared here rather than
/// copied: one experiment used to hold two of everything it concluded, and
/// these are the largest values it handles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CounterfactualEvidence {
    pub baseline: DerivedId,
    pub frame: DerivedId,
    pub split: DerivedId,
    pub counterfactual: DerivedId,
    pub candidate: DerivedId,
    pub before: Vec<DerivedId>,
    pub after: Vec<DerivedId>,
    pub comparison: DerivedId,
    pub delta_profile: Arc<super::DeltaProfile>,
    pub null_results: Vec<NullEvidence>,
    pub constraint_assessment: Option<DerivedId>,
    pub constraints: Arc<Vec<super::ConstraintAssessment>>,
    pub transfer_measurements: Vec<unclip_record::RecordedInference<unclip_measure::Measurement>>,
    #[serde(default)]
    pub pareto_assessment: Option<DerivedId>,
    #[serde(default)]
    pub pareto: Option<Arc<super::ParetoAssessment>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NullEvidence {
    pub id: DerivedId,
    pub model: PluginId,
    pub reading: Arc<unclip_measure::Reading>,
}

#[derive(Clone, Copy, Default)]
pub struct ExperimentConstraints<'a> {
    pub requirements: &'a [super::ExperimentConstraint],
    pub transfer_measurements: &'a [Tracked<unclip_measure::Measurement>],
    pub pareto_dimensions: &'a [super::ParetoDimension],
}

pub struct CounterfactualExperiment {
    pub evidence: Experimental<CounterfactualEvidence>,
    pub execution: super::CounterfactualComparison,
    pub null_results: Vec<Calculated<unclip_measure::Reading>>,
    pub constraints: Option<Calculated<Vec<super::ConstraintAssessment>>>,
    pub pareto: Option<Calculated<super::ParetoAssessment>>,
}

pub struct PersistableExperiment {
    pub outcome: Experimental<unclip_record::ExperimentOutcome>,
    pub deltas: Vec<unclip_record::ExperimentDelta>,
}

impl super::Engine {
    /// Emit evidence of an executed comparison, not candidate acceptance or promotion.
    /// Nulls use held-out observations and baseline rankings; constraints retain independent statuses.
    pub fn run_counterfactual_experiment(
        &self,
        plan: &RunPlan,
        inputs: super::CounterfactualMeasurementInputs<'_>,
        candidate: &Tracked<unclip_domain::CandidateProposal>,
        pairs: &[super::ComparisonPair],
        constraint_inputs: ExperimentConstraints<'_>,
        run: super::MeasurementRun<'_>,
    ) -> Result<CounterfactualExperiment> {
        let constraints = constraint_inputs.requirements;
        let pareto_dimensions = constraint_inputs.pareto_dimensions;
        let dependencies = DependencyCollector::default();
        let baseline = dependencies.read(inputs.baseline.baseline);
        let frame = dependencies.read(inputs.baseline.frame);
        let split = dependencies.read(inputs.baseline.split);
        let applied = Tracked::from(inputs.counterfactual);
        let snapshot = dependencies.read(&applied);
        let proposal = dependencies.read(candidate);
        if candidate.id() != &snapshot.candidate
            || !inputs
                .counterfactual
                .provenance()
                .inputs
                .contains(candidate.id())
            || proposal.domain_version_id != snapshot.baseline_domain_version_id
        {
            return Err(PluginError::Message(
                "null candidate must match the applied candidate and baseline".into(),
            ));
        }
        // Record exact selected observation values, including training membership.
        for entry in split.training.iter().chain(&split.held_out) {
            dependencies.read(&Tracked::from_recorded(
                entry.provenance.clone(),
                &entry.value,
            ));
        }
        for input in inputs.baseline.alignments.iter().chain(inputs.alignments) {
            dependencies.read(input);
        }
        for input in inputs.baseline.rankings.iter().chain(inputs.rankings) {
            dependencies.read(input);
        }
        let mut evidence = CounterfactualEvidence {
            baseline: inputs.baseline.baseline.id().clone(),
            frame: inputs.baseline.frame.id().clone(),
            split: inputs.baseline.split.id().clone(),
            counterfactual: applied.id().clone(),
            candidate: snapshot.candidate.clone(),
            null_results: vec![],
            constraint_assessment: None,
            constraints: Arc::default(),
            transfer_measurements: vec![],
            pareto_assessment: None,
            pareto: None,
            before: vec![],
            after: vec![],
            comparison: DerivedId::new(format!("{}/comparison/profile", run.id)),
            delta_profile: Arc::new(super::DeltaProfile {
                pairs: vec![],
                deltas: vec![],
                unmatched_before: vec![],
                unmatched_after: vec![],
            }),
        };
        let id = DerivedId::new(format!("{}/experiment", run.id));
        let domain_version = baseline.version.clone();
        let frame_version = frame.version.clone();
        let timestamp = run.timestamp.clone();
        let record = run_record(plan, run.params, run.id, &timestamp, serde_json::json!({}));
        let observations = split
            .held_out
            .iter()
            .map(|entry| entry.tracked(None))
            .collect::<Vec<_>>();
        let null_inputs = super::NullInputs {
            observations: &observations,
            rankings: inputs.baseline.rankings,
            domain: Some(inputs.baseline.baseline),
        };
        let null_id = format!("{}/nulls", run.id);
        let constraint_run_id = run.id.to_string();
        let null_run = super::MeasurementRun {
            id: &null_id,
            timestamp: &timestamp,
            params: run.params,
        };
        let execution = self.compare_counterfactual(plan, inputs, pairs, run)?;
        let null_results =
            evaluate_null_models_with_inputs(plan, candidate, null_inputs, null_run)?;
        for (selected, values) in [
            (&mut evidence.before, &execution.measurements.before),
            (&mut evidence.after, &execution.measurements.after),
        ] {
            for value in values {
                dependencies.read_derived(value);
                selected.push(value.id().clone());
            }
        }
        for delta in &execution.comparison.deltas {
            dependencies.read_derived(delta);
        }
        let profile = &execution.comparison.profile;
        evidence.comparison = profile.id().clone();
        evidence.delta_profile = dependencies.read_derived_shared(profile);
        for result in &null_results {
            if dependencies.contains(result.id()) || result.id() == &id {
                return Err(PluginError::Message(
                    "null output identity collides with experimental evidence".into(),
                ));
            }
            evidence.null_results.push(NullEvidence {
                id: result.id().clone(),
                model: result.provenance().producer.clone(),
                reading: dependencies.read_derived_shared(result),
            });
        }
        let mut transfer = constraint_inputs
            .transfer_measurements
            .iter()
            .collect::<Vec<_>>();
        transfer.sort_by_key(|input| input.id());
        for input in transfer {
            let used = constraints.iter().any(|constraint| matches!(constraint, super::ExperimentConstraint::ScalarTransfer { source, target, .. } if source == input.id() || target == input.id()));
            if !used
                || input.id().trim().is_empty()
                || dependencies.contains(input.id())
                || input.id() == &id
            {
                return Err(PluginError::Message("transfer inputs must be used by explicit transfer requirements and have unique noncolliding identities".into()));
            }
            if constraints.iter().any(|constraint| matches!(constraint, super::ExperimentConstraint::MinimumSamples { measurement, .. } | super::ExperimentConstraint::ConditionalDependency { measurement, .. } if measurement == input.id())) {
                return Err(PluginError::Message("external transfer evidence cannot replace experiment measurements for other constraints".into()));
            }
            evidence
                .transfer_measurements
                .push(unclip_record::RecordedInference::shared(
                    input.id().clone(),
                    dependencies.read_shared(input),
                ));
        }
        let pareto = if pareto_dimensions.is_empty() {
            None
        } else {
            let measurements = execution
                .measurements
                .before
                .iter()
                .chain(&execution.measurements.after)
                .map(Tracked::from)
                .collect::<Vec<_>>();
            let result = compare_pareto(
                &measurements,
                pareto_dimensions,
                &constraint_run_id,
                timestamp.clone(),
            )?;
            if dependencies.contains(result.id()) || result.id() == &id {
                return Err(PluginError::Message(
                    "Pareto output identity collides with experimental evidence".into(),
                ));
            }
            evidence.pareto_assessment = Some(result.id().clone());
            evidence.pareto = Some(dependencies.read_derived_shared(&result));
            Some(result)
        };
        let assessments = if constraints.is_empty() {
            None
        } else {
            let mut measurements = execution
                .measurements
                .before
                .iter()
                .chain(&execution.measurements.after)
                .map(Tracked::from)
                .collect::<Vec<_>>();
            measurements.extend(
                evidence
                    .transfer_measurements
                    .iter()
                    .map(|entry| entry.tracked(None)),
            );
            let result = assess_experiment_constraints(
                constraints,
                &measurements,
                &applied,
                &constraint_run_id,
                timestamp.clone(),
            )?;
            if dependencies.contains(result.id()) || result.id() == &id {
                return Err(PluginError::Message(
                    "constraint output identity collides with experimental evidence".into(),
                ));
            }
            evidence.constraint_assessment = Some(result.id().clone());
            evidence.constraints = dependencies.read_derived_shared(&result);
            Some(result)
        };
        if dependencies.contains(&id) || evidence.candidate == id {
            return Err(PluginError::Message(
                "experimental output identity collides with an input or intermediate result".into(),
            ));
        }
        let mut params = serde_json::json!({"pairs":evidence.delta_profile.pairs,"constraints":constraints,"transfer_measurements":evidence.transfer_measurements,"pareto_dimensions":pareto_dimensions});
        // Moved in rather than listed in `json!`, which serializes each value
        // through a reference and so would copy a plan nothing else reads.
        params["plan"] = record.resolved_plan;
        let token = ExperimentToken::from_harness(
            EmitMetadata::new(
                id,
                PluginId::new("experiment.counterfactual"),
                semver::Version::new(0, 5, 0),
                params,
                timestamp,
            )
            .with_algorithm("held_out_counterfactual_comparison")
            .with_domain_version(domain_version)
            .with_frame_version(frame_version),
            dependencies,
        );
        Ok(CounterfactualExperiment {
            evidence: token.emit(evidence),
            execution,
            null_results,
            constraints: assessments,
            pareto,
        })
    }
}

/// The storage identities a completed experiment is filed under.
///
/// These were five consecutive `&str` parameters — two version ids, the before
/// and after profile ids, and a start time. Any transposition among them
/// compiled and passed the emptiness checks below, and `before`/`after` in
/// particular decide which profile a recorded delta is measured *from*. The
/// only check that could ever have caught a swap is that the two profile ids
/// differ, which a swap preserves.
#[derive(Clone, Copy)]
pub struct ExperimentStorageIds<'a> {
    pub domain_version_id: &'a str,
    pub frame_version_id: &'a str,
    /// The profile measured before the candidate was applied.
    pub before_profile_id: &'a str,
    /// The profile measured after it was applied. Must differ from `before`.
    pub after_profile_id: &'a str,
    pub started_at: &'a str,
}

/// Convert executed evidence to the storage repository's completed bundle.
/// Direct dependencies are the persisted candidate, selected observations,
/// and calculated deltas; the typed result remains intact in `result`.
pub fn persistable_experiment(
    experiment: &CounterfactualExperiment,
    split: &Calculated<super::ObservationSplit>,
    candidate: &Tracked<unclip_domain::CandidateProposal>,
    storage: ExperimentStorageIds<'_>,
) -> Result<PersistableExperiment> {
    let ExperimentStorageIds {
        domain_version_id,
        frame_version_id,
        before_profile_id,
        after_profile_id,
        started_at,
    } = storage;
    let invalid = |message: &str| PluginError::Message(message.into());
    if domain_version_id.trim().is_empty()
        || frame_version_id.trim().is_empty()
        || before_profile_id.trim().is_empty()
        || after_profile_id.trim().is_empty()
        || before_profile_id == after_profile_id
        || started_at.trim().is_empty()
    {
        return Err(invalid(
            "persistable experiments require distinct profiles and nonempty storage identities",
        ));
    }
    let evidence = experiment.evidence.value();
    if evidence.candidate != *candidate.id() || evidence.split != *split.id() {
        return Err(invalid(
            "persisted candidate and split must match the executed experiment",
        ));
    }
    let dependencies = DependencyCollector::default();
    dependencies.read(candidate);
    let split_value = split.value();
    for entry in split_value.training.iter().chain(&split_value.held_out) {
        dependencies.read(&Tracked::from_recorded(
            entry.provenance.clone(),
            &entry.value,
        ));
    }
    let deltas = experiment
        .execution
        .comparison
        .deltas
        .iter()
        .map(|delta| {
            dependencies.read_derived(delta);
            unclip_record::ExperimentDelta {
                before_profile_id: before_profile_id.into(),
                after_profile_id: after_profile_id.into(),
                calculated: delta.clone(),
            }
        })
        .collect::<Vec<_>>();
    if deltas.is_empty() {
        return Err(invalid(
            "persistable experiments require calculated comparison deltas",
        ));
    }
    let plan = experiment
        .evidence
        .provenance()
        .params
        .get("plan")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .ok_or_else(|| invalid("experimental evidence has no resolved plan"))?;
    let result =
        match serde_json::to_value(evidence).map_err(|error| invalid(&error.to_string()))? {
            serde_json::Value::Object(result) => result,
            _ => return Err(invalid("experimental evidence must serialize as an object")),
        };
    let value = unclip_record::ExperimentOutcome {
        candidate_id: candidate.id().clone(),
        domain_version_id: domain_version_id.into(),
        frame_version_id: frame_version_id.into(),
        plan,
        result,
        training: split_value
            .training
            .iter()
            .map(|entry| entry.value.id.clone())
            .collect(),
        held_out: split_value
            .held_out
            .iter()
            .map(|entry| entry.value.id.clone())
            .collect(),
        started_at: started_at.into(),
    };
    let params = serde_json::json!({
        "evidence": experiment.evidence.id(),
        "before_profile": before_profile_id,
        "after_profile": after_profile_id,
    });
    let id = DerivedId::new(format!("{}/completed", experiment.evidence.id()));
    if dependencies.contains(&id) {
        return Err(invalid(
            "persisted experiment identity collides with an evidence input",
        ));
    }
    let token = ExperimentToken::from_harness(
        EmitMetadata::new(
            id,
            PluginId::new("experiment.persist"),
            semver::Version::new(0, 1, 0),
            params,
            experiment.evidence.provenance().timestamp.clone(),
        )
        .with_algorithm("completed_counterfactual_bundle")
        .with_domain_version(experiment.evidence.provenance().domain_version.clone())
        .with_frame_version(experiment.evidence.provenance().frame_version.clone()),
        dependencies,
    );
    Ok(PersistableExperiment {
        outcome: token.emit(value),
        deltas,
    })
}
