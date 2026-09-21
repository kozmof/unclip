//! Baseline sensor execution over a frozen, explicitly selected held-out split.
use std::collections::BTreeSet;
use unclip_domain::{DomainSnapshot, MeasurementFrame};
use unclip_epistemic::{Calculated, DependencyCollector, Tracked};
use unclip_measure::Measurement;
use unclip_observe::{Alignment, PartialRanking};
use unclip_plugin::{PluginError, Result, RunPlan};

pub struct HeldOutInputs<'a> {
    pub baseline: &'a Tracked<DomainSnapshot>,
    pub frame: &'a Tracked<MeasurementFrame>,
    pub split: &'a Tracked<super::ObservationSplit>,
    /// Only held-out inference products are accepted. Missing products remain missing.
    pub alignments: &'a [Tracked<Alignment>],
    pub rankings: &'a [Tracked<PartialRanking>],
}

fn invalid(message: &str) -> PluginError {
    PluginError::Message(message.into())
}

impl super::Engine {
    /// Measures the recorded held-out observations against an explicitly tracked baseline.
    /// Inference products must already have been established for this baseline; this
    /// entry point does not infer alignments or certify their transitive ancestry.
    pub fn measure_held_out_baseline(
        &self,
        plan: &RunPlan,
        inputs: HeldOutInputs<'_>,
        run: super::MeasurementRun<'_>,
    ) -> Result<Vec<Calculated<Measurement>>> {
        if run.id.trim().is_empty() || plan.sensors.is_empty() {
            return Err(invalid(
                "held-out measurement requires a run identity and selected sensors",
            ));
        }
        let dependencies = DependencyCollector::default();
        let domain = dependencies.read(inputs.baseline);
        let frame = dependencies.read(inputs.frame);
        let split = dependencies.read(inputs.split);
        let identities = [inputs.baseline.id(), inputs.frame.id(), inputs.split.id()];
        if identities.iter().any(|id| id.0.trim().is_empty())
            || identities.into_iter().collect::<BTreeSet<_>>().len() != 3
        {
            return Err(invalid(
                "baseline, frame and split require distinct nonempty identities",
            ));
        }
        let mut axes = BTreeSet::new();
        for axis in &frame.axes {
            if !domain.units.contains_key(&axis.unit) || !axes.insert(&axis.unit) {
                return Err(invalid("frame axes must be unique units in the baseline"));
            }
        }
        if split.held_out.is_empty() {
            return Err(invalid("held-out observations must be nonempty"));
        }
        let mut members = BTreeSet::new();
        for entry in split.training.iter().chain(&split.held_out) {
            if entry.value.id.0.trim().is_empty()
                || entry.provenance.0.trim().is_empty()
                || !members.insert(&entry.value.id)
            {
                return Err(invalid(
                    "observation splits require unique disjoint identities and source provenance",
                ));
            }
        }
        let held_out = split
            .held_out
            .iter()
            .map(|entry| &entry.value.id)
            .collect::<BTreeSet<_>>();
        let mut aligned = BTreeSet::new();
        for input in inputs.alignments {
            let value = dependencies.read(input);
            if input.id().0.trim().is_empty()
                || !held_out.contains(&value.observation)
                || !aligned.insert(&value.observation)
            {
                return Err(invalid(
                    "alignments must uniquely reference held-out observations",
                ));
            }
        }
        let mut ranked = BTreeSet::new();
        for input in inputs.rankings {
            let value = dependencies.read(input);
            if input.id().0.trim().is_empty()
                || !held_out.contains(&value.observation)
                || !ranked.insert(&value.observation)
            {
                return Err(invalid(
                    "rankings must uniquely reference held-out observations",
                ));
            }
        }
        let observations = split
            .held_out
            .iter()
            .map(|entry| Tracked::from_recorded(entry.provenance.clone(), entry.value.clone()))
            .collect::<Vec<_>>();
        let results = self.measure_with_dependencies(
            plan,
            super::MeasurementInputs {
                domain,
                frame,
                observations: &observations,
                alignments: inputs.alignments,
                rankings: inputs.rankings,
            },
            run,
            |collector| {
                collector.read(inputs.baseline);
                collector.read(inputs.frame);
                collector.read(inputs.split);
            },
        )?;
        let mut input_ids = dependencies.snapshot().into_iter().collect::<BTreeSet<_>>();
        input_ids.extend(
            split
                .training
                .iter()
                .chain(&split.held_out)
                .map(|entry| entry.provenance.clone()),
        );
        let mut output_ids = BTreeSet::new();
        for result in &results {
            if input_ids.contains(result.id()) || !output_ids.insert(result.id().clone()) {
                return Err(invalid(
                    "held-out measurement output identity collides with an input or output",
                ));
            }
        }
        Ok(results)
    }
}

/// Both sides use one frame, split, sensor plan and parameter map. Inference
/// products are supplied separately because candidate application can change them.
pub struct CounterfactualMeasurementInputs<'a> {
    pub baseline: HeldOutInputs<'a>,
    pub counterfactual: &'a Calculated<super::CounterfactualSnapshot>,
    pub alignments: &'a [Tracked<Alignment>],
    pub rankings: &'a [Tracked<PartialRanking>],
}

pub struct CounterfactualMeasurements {
    pub before: Vec<Calculated<Measurement>>,
    pub after: Vec<Calculated<Measurement>>,
}

impl super::Engine {
    /// Run D and D+c on exactly the same recorded observations and frame.
    /// New candidate units are not implicitly added to the frame or aligned.
    pub fn measure_counterfactual(
        &self,
        plan: &RunPlan,
        inputs: CounterfactualMeasurementInputs<'_>,
        run: super::MeasurementRun<'_>,
    ) -> Result<CounterfactualMeasurements> {
        let dependencies = DependencyCollector::default();
        let baseline = dependencies.read(inputs.baseline.baseline);
        let snapshot = inputs.counterfactual.value();
        let baseline_key = serde_json::to_string(&(&baseline.id.0, &baseline.version.0))
            .map_err(|error| invalid(&error.to_string()))?;
        if snapshot.baseline_domain_version_id != baseline_key
            || snapshot.domain.id != baseline.id
            || snapshot.domain.version == baseline.version
            || !inputs
                .counterfactual
                .provenance()
                .inputs
                .contains(inputs.baseline.baseline.id())
        {
            return Err(invalid(
                "counterfactual must derive from the selected baseline and have a distinct version",
            ));
        }
        let counterfactual = Tracked::from_derived(inputs.counterfactual, snapshot.domain.clone());
        if counterfactual.id() == inputs.baseline.baseline.id() {
            return Err(invalid(
                "counterfactual and baseline identities must differ",
            ));
        }
        let before_id = format!("{}/before", run.id);
        let after_id = format!("{}/after", run.id);
        if run.id.trim().is_empty() {
            return Err(invalid(
                "counterfactual measurement requires a run identity",
            ));
        }
        let after_inputs = HeldOutInputs {
            baseline: &counterfactual,
            frame: inputs.baseline.frame,
            split: inputs.baseline.split,
            alignments: inputs.alignments,
            rankings: inputs.rankings,
        };
        // Check cross-side identity collisions as well as each side's local checks.
        let mut input_ids = BTreeSet::from([
            inputs.baseline.baseline.id().clone(),
            counterfactual.id().clone(),
            inputs.baseline.frame.id().clone(),
            inputs.baseline.split.id().clone(),
        ]);
        input_ids.extend(
            inputs
                .baseline
                .alignments
                .iter()
                .chain(inputs.alignments)
                .map(|v| v.id().clone()),
        );
        input_ids.extend(
            inputs
                .baseline
                .rankings
                .iter()
                .chain(inputs.rankings)
                .map(|v| v.id().clone()),
        );
        let before = self.measure_held_out_baseline(
            plan,
            inputs.baseline,
            super::MeasurementRun {
                id: &before_id,
                timestamp: run.timestamp.clone(),
                params: run.params,
            },
        )?;
        let after = self.measure_held_out_baseline(
            plan,
            after_inputs,
            super::MeasurementRun {
                id: &after_id,
                timestamp: run.timestamp,
                params: run.params,
            },
        )?;
        let mut output_ids = BTreeSet::new();
        for result in before.iter().chain(&after) {
            if input_ids.contains(result.id()) || !output_ids.insert(result.id().clone()) {
                return Err(invalid(
                    "counterfactual measurement output identity collides with an input or output",
                ));
            }
        }
        Ok(CounterfactualMeasurements { before, after })
    }
}
