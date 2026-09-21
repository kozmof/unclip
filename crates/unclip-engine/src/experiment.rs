//! Experimental aggregation of executed counterfactual measurements and comparisons.
use serde::{Deserialize, Serialize};
use unclip_epistemic::{
    hash_params, DependencyCollector, DerivedId, EmitMetadata, ExperimentToken, Experimental,
    PluginId, Tracked,
};
use unclip_plugin::{PluginError, Result, RunPlan};

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
    pub delta_profile: super::DeltaProfile,
}

pub struct CounterfactualExperiment {
    pub evidence: Experimental<CounterfactualEvidence>,
    pub execution: super::CounterfactualComparison,
}

impl super::Engine {
    /// Emit evidence of an executed comparison, not candidate acceptance or promotion.
    /// Null-model evaluation and constraint decisions are separate pending stages.
    pub fn run_counterfactual_experiment(
        &self,
        plan: &RunPlan,
        inputs: super::CounterfactualMeasurementInputs<'_>,
        pairs: &[super::ComparisonPair],
        run: super::MeasurementRun<'_>,
    ) -> Result<CounterfactualExperiment> {
        if !plan.null_models.is_empty() {
            return Err(PluginError::Message(
                "experimental null-model execution is not yet supported".into(),
            ));
        }
        let dependencies = DependencyCollector::default();
        let baseline = dependencies.read(inputs.baseline.baseline);
        let frame = dependencies.read(inputs.baseline.frame);
        let split = dependencies.read(inputs.baseline.split);
        let applied =
            Tracked::from_derived(inputs.counterfactual, inputs.counterfactual.value().clone());
        let snapshot = dependencies.read(&applied);
        // Record exact selected observation values, including training membership.
        for entry in split.training.iter().chain(&split.held_out) {
            dependencies.read(&Tracked::from_recorded(
                entry.provenance.clone(),
                entry.value.clone(),
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
            before: vec![],
            after: vec![],
            comparison: DerivedId::new(format!("{}/comparison/profile", run.id)),
            delta_profile: super::DeltaProfile {
                pairs: vec![],
                deltas: vec![],
                unmatched_before: vec![],
                unmatched_after: vec![],
            },
        };
        let id = DerivedId::new(format!("{}/experiment", run.id));
        let domain_version = baseline.version.clone();
        let frame_version = frame.version.clone();
        let timestamp = run.timestamp.clone();
        let record = self.run_record(
            plan,
            run.params,
            run.id,
            timestamp.clone(),
            serde_json::json!({}),
        );
        let execution = self.compare_counterfactual(plan, inputs, pairs, run)?;
        for (selected, values) in [
            (&mut evidence.before, &execution.measurements.before),
            (&mut evidence.after, &execution.measurements.after),
        ] {
            for value in values {
                dependencies.read(&Tracked::from_derived(value, value.value().clone()));
                selected.push(value.id().clone());
            }
        }
        for delta in &execution.comparison.deltas {
            dependencies.read(&Tracked::from_derived(delta, delta.value().clone()));
        }
        let profile = &execution.comparison.profile;
        evidence.comparison = profile.id().clone();
        evidence.delta_profile = dependencies
            .read(&Tracked::from_derived(profile, profile.value().clone()))
            .clone();
        if dependencies.snapshot().contains(&id) || evidence.candidate == id {
            return Err(PluginError::Message(
                "experimental output identity collides with an input or intermediate result".into(),
            ));
        }
        let params =
            serde_json::json!({"plan":record.resolved_plan,"pairs":evidence.delta_profile.pairs});
        let token = ExperimentToken::from_harness(
            EmitMetadata {
                id,
                producer: PluginId::new("experiment.counterfactual"),
                algorithm: "held_out_counterfactual_comparison".into(),
                version: semver::Version::new(0, 1, 0),
                params_hash: hash_params(&params),
                params,
                source: None,
                timestamp,
                domain_version: Some(domain_version),
                frame_version: Some(frame_version),
                model: None,
            },
            dependencies,
        );
        Ok(CounterfactualExperiment {
            evidence: token.emit(evidence),
            execution,
        })
    }
}
