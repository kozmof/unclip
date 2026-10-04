//! Stage orchestration and run planning for the semantic leveling engine.
//!
//! # Finding things
//!
//! The plugin families live in four modules named for what they do, and the
//! group is where a new one goes:
//!
//! - [`nulls`] — what the domain already accounts for
//! - [`comparisons`] — the typed difference between two measurements
//! - [`discovery`] — what the domain does not yet account for
//! - [`applications`] — applying a candidate to build its counterfactual
//!
//! Prefer the grouped path: `comparisons::ranking::KendallComparator` says
//! which family a type belongs to and which kind it compares, and it reads
//! once rather than twice — the submodules are named for their subject, not
//! their group.
//!
//! Everything in those modules is also re-exported at the crate root, because
//! that is where callers have always found it. The root is additionally the
//! home of what belongs to no family: the stage orchestration on [`Engine`],
//! the revision ladder, and the free functions that take a [`RunPlan`] and
//! construct nothing, so a caller wanting a pure calculation need not build a
//! registry to reach one.

#![forbid(unsafe_code)]

pub mod applications;
pub mod comparisons;
mod composition_measurement;
mod constraints;
mod cross_domain;
pub use cross_domain::{CrossDomainRun, CrossProductTransferInputs, TransferSide};
pub mod discovery;
mod empirical;
mod experiment;
mod held_out;
mod independence;
pub mod nulls;
mod observation_selection;
mod pareto;
mod product_domain;
pub use product_domain::{create_product_frame, materialize_product_domain};
mod revision;
mod support;
mod transfer_constraint;

pub use applications::candidate::{
    CounterfactualSnapshot, PropertyChange, PropertyTarget, RelationBindings,
};
pub use comparisons::distribution::{
    DistributionComparison, DistributionNormalization, JensenShannonComparator,
};
pub use comparisons::event::{ChangePointAlignmentComparator, EventComparison, EventMatch};
pub use comparisons::graph::{
    DirectedGraphEdge, GraphComparison, GraphIdentityComparator, NamedDirectedGraph,
};
pub use comparisons::independence::{
    compare_product_with_independence, IndependenceComparisonEntry, IndependenceComparisonProfile,
    IndependenceComparisonResult,
};
pub use comparisons::matrix::{MatrixCellDifference, MatrixComparison, PairwiseMatrixComparator};
pub use comparisons::partition::{PartitionComparison, PartitionRandComparator};
pub use comparisons::profile::{
    compare_profiles, ComparisonPair, DeltaProfile, ProfileComparisonResult, ProfileDelta,
};
pub use comparisons::ranking::{KendallComparator, RankingComparison, RboComparator};
pub use comparisons::scalar::{compare_measurements, ScalarDifference, ScalarDifferenceComparator};
pub use comparisons::spectral::{SpectralComparison, SpectrumComparator};
pub use comparisons::structured::{StructuredIdentityComparator, StructuredIdentityComparison};
pub use composition_measurement::{
    measure_composition, BoundDomainMeasurementProfile, BoundProductMeasurementProfile,
    CompositionMeasurementInputs, CompositionMeasurementProfile, CompositionSide,
    ProfileMeasurement,
};
pub use constraints::{
    assess_experiment_constraints, ConstraintAssessment, ConstraintStatus, ExperimentConstraint,
};
pub use discovery::cross_domain::{
    derive_cross_domain_deviations, CrossDomainCandidateGenerator, CrossDomainDeviationEvidence,
};
pub use discovery::motif::RecurringMotifGenerator;
pub use discovery::pairwise::PairwiseCouplingGenerator;
pub use discovery::relation::MissingRelationGenerator;
pub use discovery::residual::{generate_candidates, CandidateInputs, PersistentResidualGenerator};
pub use discovery::structure::{CommunityCandidateGenerator, LatentAxisGenerator};
pub use discovery::temporal::TemporalCouplingGenerator;
pub use empirical::{derive_empirical, EmpiricalMethod, EmpiricalResult};
pub use experiment::{
    persistable_experiment, CounterfactualEvidence, CounterfactualExperiment,
    ExperimentConstraints, ExperimentStorageIds, NullEvidence, PersistableExperiment,
};
pub use held_out::{
    CounterfactualComparison, CounterfactualMeasurementInputs, CounterfactualMeasurements,
    HeldOutInputs,
};
pub use independence::{
    IndependenceDefinition, IndependenceExpectation, IndependenceExpectationProfile,
};
pub use nulls::context::ContextualCooccurrenceNull;
pub use nulls::coupling::CouplingZeroNull;
pub use nulls::domain::{ExistingRelationNull, ExistingUnitNull};
pub use nulls::models::{
    evaluate_null_models, evaluate_null_models_with_inputs, evaluate_null_models_with_rankings,
    NullInputs, RandomCooccurrenceNull,
};
pub use nulls::motif::ExistingMotifNull;
pub use nulls::ranking::RankingConstraintNull;
pub use nulls::role::ExistingRoleNull;
pub use nulls::transformation::ExistingTransformationNull;
pub use nulls::weight::WeightChangeNull;
pub use observation_selection::{
    observation_split_run_record, select_observations, validate_candidate_ancestry,
    ObservationSplit,
};
pub use pareto::{
    compare_pareto, ObjectiveDirection, ParetoAssessment, ParetoDimension, ParetoEvidence,
    ParetoRelation,
};
pub use revision::{
    record_delta_e_test, record_delta_v_test, record_delta_w_test, record_dynamic_coupling_test,
    record_structural_test, RevisionAttempt, RevisionStep, RevisionTest, RevisionTestOutcome,
};
pub use transfer_constraint::TransferAssessment;

use std::sync::Arc;

use unclip_domain::{DomainSnapshot, MeasurementFrame};
use unclip_epistemic::{
    hash_params, Calculated, DependencyCollector, DerivedId, EmitMetadata, InferenceToken,
    Inferred, Interpreted, Operation, PluginParams, SharedParams, SourceRef, Timestamp, Tracked,
};
use unclip_measure::{EmpiricalStructure, Measurement, MeasurementContext};
use unclip_observe::{Alignment, Observation, PartialRanking};
use unclip_plugin::{
    classify_sensor, EngineProfile, InterpretCtx, MeasureCtx, Registry, Result, RunPlan,
    SensorDecision, SensorStage,
};

/// Construct the runtime registry using explicit first-party registration.
pub fn builtin_registry() -> Result<Registry> {
    let mut registry = Registry::default();
    unclip_infer::register_all(&mut registry)?;
    unclip_interpret::register_all(&mut registry)?;
    unclip_sensors::register_all(&mut registry)?;
    registry.register_comparator(std::sync::Arc::new(ScalarDifferenceComparator::default()))?;
    registry.register_comparator(std::sync::Arc::new(KendallComparator::default()))?;
    registry.register_comparator(std::sync::Arc::new(RboComparator::default()))?;
    registry.register_comparator(std::sync::Arc::new(JensenShannonComparator::default()))?;
    registry.register_comparator(std::sync::Arc::new(PairwiseMatrixComparator::default()))?;
    registry.register_comparator(std::sync::Arc::new(SpectrumComparator::default()))?;
    registry.register_comparator(std::sync::Arc::new(PartitionRandComparator::default()))?;
    registry.register_comparator(std::sync::Arc::new(GraphIdentityComparator::default()))?;
    registry.register_comparator(std::sync::Arc::new(StructuredIdentityComparator::default()))?;
    registry.register_comparator(std::sync::Arc::new(
        ChangePointAlignmentComparator::default(),
    ))?;
    registry.register_generator(std::sync::Arc::new(PersistentResidualGenerator::default()))?;
    registry.register_generator(std::sync::Arc::new(MissingRelationGenerator::default()))?;
    registry.register_generator(std::sync::Arc::new(RecurringMotifGenerator::default()))?;
    registry.register_generator(std::sync::Arc::new(PairwiseCouplingGenerator::default()))?;
    registry.register_generator(std::sync::Arc::new(TemporalCouplingGenerator::default()))?;
    registry.register_generator(std::sync::Arc::new(CommunityCandidateGenerator::default()))?;
    registry.register_generator(std::sync::Arc::new(LatentAxisGenerator::default()))?;
    registry.register_generator(std::sync::Arc::new(CrossDomainCandidateGenerator::default()))?;
    registry.register_null_model(std::sync::Arc::new(RandomCooccurrenceNull::default()))?;
    registry.register_null_model(std::sync::Arc::new(RankingConstraintNull::default()))?;
    registry.register_null_model(std::sync::Arc::new(ExistingUnitNull::default()))?;
    registry.register_null_model(std::sync::Arc::new(ExistingRelationNull::default()))?;
    registry.register_null_model(std::sync::Arc::new(WeightChangeNull::default()))?;
    registry.register_null_model(std::sync::Arc::new(ContextualCooccurrenceNull::default()))?;
    registry.register_null_model(std::sync::Arc::new(CouplingZeroNull::default()))?;
    registry.register_null_model(std::sync::Arc::new(ExistingMotifNull::default()))?;
    registry.register_null_model(std::sync::Arc::new(ExistingRoleNull::default()))?;
    registry.register_null_model(std::sync::Arc::new(ExistingTransformationNull::default()))?;
    Ok(registry)
}

/// Inputs controlled by the harness for one inference stage.
pub struct InferenceRun<'a> {
    pub id: &'a str,
    pub source: SourceRef,
    pub timestamp: Timestamp,
    pub params: &'a PluginParams,
    pub io: &'a dyn unclip_plugin::InferenceIo,
}

/// Inferred aggregates together with typed handles for later calculation stages.
#[derive(Debug, Default)]
pub struct InferenceResults {
    pub outputs: Vec<Inferred<unclip_plugin::InferenceOutput>>,
    pub observations: Vec<Tracked<Observation>>,
    pub alignments: Vec<Tracked<Alignment>>,
    pub rankings: Vec<Tracked<PartialRanking>>,
}

impl InferenceResults {
    fn push(&mut self, output: Inferred<unclip_plugin::InferenceOutput>) {
        match output.value() {
            unclip_plugin::InferenceOutput::Bundle {
                observations,
                alignments,
                rankings,
            } => {
                // Tracking a product shares the inferrer's allocation instead
                // of copying it: the emitted output is retained in `outputs`
                // for provenance, so both halves point at one observation.
                self.observations.extend(
                    observations
                        .iter()
                        .map(|value| Tracked::from_derived_shared(&output, Arc::clone(value))),
                );
                self.alignments.extend(
                    alignments
                        .iter()
                        .map(|value| Tracked::from_derived_shared(&output, Arc::clone(value))),
                );
                self.rankings.extend(
                    rankings
                        .iter()
                        .map(|value| Tracked::from_derived_shared(&output, Arc::clone(value))),
                );
            }
            unclip_plugin::InferenceOutput::Observations(values) => {
                self.observations.extend(
                    values
                        .iter()
                        .map(|value| Tracked::from_derived_shared(&output, Arc::clone(value))),
                );
            }
            unclip_plugin::InferenceOutput::Alignments(values) => {
                self.alignments.extend(
                    values
                        .iter()
                        .map(|value| Tracked::from_derived_shared(&output, Arc::clone(value))),
                );
            }
            unclip_plugin::InferenceOutput::Rankings(values) => {
                self.rankings.extend(
                    values
                        .iter()
                        .map(|value| Tracked::from_derived_shared(&output, Arc::clone(value))),
                );
            }
            unclip_plugin::InferenceOutput::Structured(_) => {}
        }
        self.outputs.push(output);
    }
}

#[derive(Debug)]
pub struct PipelineResults {
    pub inference: InferenceResults,
    pub explanations: Vec<Calculated<Measurement>>,
    pub residuals: Vec<Calculated<Measurement>>,
    pub measurements: Vec<Calculated<Measurement>>,
}

/// Require evidence produced by calculation.
///
/// An in-memory `Calculated<T>` can only have come from a `CalculationToken`, so
/// its label is trustworthy. A value restored from storage carries whatever
/// operation its caller named: `Tracked::from_calculated` for a row from a table
/// that admits only calculated values, `from_inferred` for a replayed inference
/// product, and `from_recorded` when the operation genuinely is not known there.
///
/// `None` is accepted rather than rejected because the unlabeled case means "no
/// claim was made", and rejecting it would break the import and legacy paths
/// that legitimately cannot make one. That is the limit of what this check
/// proves: it rejects evidence *labeled* as inferred, experimental, or
/// interpreted, and it cannot distinguish trustworthy-but-unlabeled from
/// carelessly-unlabeled. Call the labeling constructors wherever the source
/// table settles the question, so this gate has a fact to check.
pub(crate) fn require_calculated_evidence<T>(input: &Tracked<T>, kind: &str) -> Result<()> {
    match input.operation() {
        None | Some(Operation::Calculated) => Ok(()),
        Some(operation) => Err(unclip_plugin::PluginError::Message(format!(
            "{kind} must be calculated evidence, found {operation:?}"
        ))),
    }
}

/// Inputs already established by observation and inference stages.
pub struct MeasurementInputs<'a> {
    pub domain: &'a DomainSnapshot,
    pub frame: &'a MeasurementFrame,
    pub observations: &'a [Tracked<Observation>],
    pub alignments: &'a [Tracked<Alignment>],
    pub rankings: &'a [Tracked<PartialRanking>],
}

/// Reproducible inputs controlled by the run harness.
pub struct MeasurementRun<'a> {
    pub id: &'a str,
    pub timestamp: Timestamp,
    pub params: &'a PluginParams,
}

/// Reproducible inputs controlled by one interpretation stage.
pub struct InterpretationRun<'a> {
    pub id: &'a str,
    pub timestamp: Timestamp,
    pub params: &'a PluginParams,
    pub io: &'a dyn unclip_plugin::InterpretationIo,
}

/// Owns the plugin registry used to resolve and execute reproducible run plans.
pub struct Engine {
    registry: Registry,
}

impl Engine {
    pub fn with_builtins() -> Result<Self> {
        Ok(Self {
            registry: builtin_registry()?,
        })
    }

    pub fn new(registry: Registry) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn plan(&self, profile: &EngineProfile) -> Result<RunPlan> {
        self.registry.resolve(profile)
    }

    /// Replay persisted inference products and re-execute calculation stages only.
    pub fn verify(
        &self,
        plan: &RunPlan,
        domain: &DomainSnapshot,
        frame: &MeasurementFrame,
        replay: &unclip_record::EngineRunReplay,
        run: MeasurementRun<'_>,
    ) -> Result<Vec<Calculated<Measurement>>> {
        // A replay's observations, alignments, and rankings are inference
        // products by construction, so they are restored as inferred rather
        // than unlabeled. A sensor reads them through the context either way;
        // labeling them keeps them from passing a calculated-evidence gate.
        let inferred = Some(unclip_epistemic::Operation::Inferred);
        let observations = replay
            .observations
            .iter()
            .map(|record| record.tracked(inferred))
            .collect::<Vec<_>>();
        let alignments = replay
            .alignments
            .iter()
            .map(|record| record.tracked(inferred))
            .collect::<Vec<_>>();
        let rankings = replay
            .rankings
            .iter()
            .map(|record| record.tracked(inferred))
            .collect::<Vec<_>>();

        self.measure(
            plan,
            MeasurementInputs {
                domain,
                frame,
                observations: &observations,
                alignments: &alignments,
                rankings: &rankings,
            },
            run,
        )
    }

    /// Execute inference, explanation, residual, and measurement stages in order.
    pub async fn execute(
        &self,
        plan: &RunPlan,
        domain: &DomainSnapshot,
        frame: &MeasurementFrame,
        run: InferenceRun<'_>,
    ) -> Result<PipelineResults> {
        let measurement_run = MeasurementRun {
            id: run.id,
            timestamp: run.timestamp.clone(),
            params: run.params,
        };
        let inference = self.infer(plan, domain, run).await?;
        let calculated = self.measure(
            plan,
            MeasurementInputs {
                domain,
                frame,
                observations: &inference.observations,
                alignments: &inference.alignments,
                rankings: &inference.rankings,
            },
            measurement_run,
        )?;

        // Each measurement is filed under the stage its own sensor declares, so
        // a renamed or third-party sensor lands in the stage it asked for.
        let stages = plan
            .sensors
            .iter()
            .map(|sensor| {
                let descriptor = sensor.descriptor();
                (descriptor.id.clone(), descriptor.stage)
            })
            .collect::<std::collections::BTreeMap<_, _>>();

        let mut explanations = Vec::new();
        let mut residuals = Vec::new();
        let mut measurements = Vec::new();
        for value in calculated {
            // Keyed by the provenance producer, which `measure` stamps from the
            // invoking sensor's descriptor, rather than by the payload's
            // self-reported `sensor` field, which a plugin writes itself.
            let producer = &value.provenance().producer;
            let stage = stages.get(producer).copied().ok_or_else(|| {
                unclip_plugin::PluginError::Message(format!(
                    "measurement {} was produced by {producer}, which is not in the run plan",
                    value.id()
                ))
            })?;
            match stage {
                SensorStage::Explanation => explanations.push(value),
                SensorStage::Residual => residuals.push(value),
                SensorStage::Measurement => measurements.push(value),
            }
        }
        Ok(PipelineResults {
            inference,
            explanations,
            residuals,
            measurements,
        })
    }

    /// Execute calculation sensors in stable stage and plugin-id order.
    ///
    /// Every invocation gets a fresh dependency collector so provenance cannot
    /// leak reads from one sensor into another.
    pub fn measure(
        &self,
        plan: &RunPlan,
        inputs: MeasurementInputs<'_>,
        run: MeasurementRun<'_>,
    ) -> Result<Vec<Calculated<Measurement>>> {
        self.measure_with_dependencies(plan, inputs, run, |_| {})
    }

    fn measure_with_dependencies(
        &self,
        plan: &RunPlan,
        inputs: MeasurementInputs<'_>,
        run: MeasurementRun<'_>,
        seed: impl Fn(&DependencyCollector),
    ) -> Result<Vec<Calculated<Measurement>>> {
        support::require_run_id("measurement", run.id)?;
        let mut sensors = plan.sensors.iter().collect::<Vec<_>>();
        sensors.sort_by(|left, right| {
            let left = left.descriptor();
            let right = right.descriptor();
            left.stage
                .cmp(&right.stage)
                .then_with(|| left.id.cmp(&right.id))
        });

        let empty_params = SharedParams::new(serde_json::json!({}));
        let mut measurements = Vec::new();
        for sensor in sensors {
            let descriptor = sensor.descriptor();
            let params = run.params.get(&descriptor.id).unwrap_or(&empty_params);
            // Checked before `classify_sensor`, which would record a violation
            // as a `NotApplicable` reading. See `require_declared_params`.
            support::require_declared_params(&descriptor.id, descriptor.params_schema, params)?;
            let dependencies = DependencyCollector::default();
            seed(&dependencies);
            let ctx = MeasureCtx::new(
                inputs.domain,
                inputs.frame,
                inputs.observations,
                inputs.alignments,
                inputs.rankings,
                params,
                dependencies,
            );
            let metadata = EmitMetadata::new(
                DerivedId::new(format!("{}/{}", run.id, descriptor.id)),
                descriptor.id.clone(),
                descriptor.version.clone(),
                SharedParams::clone(params),
                run.timestamp.clone(),
            );
            match classify_sensor(sensor.as_ref(), &ctx) {
                SensorDecision::Run => {
                    measurements.extend(sensor.measure(&ctx, ctx.calculation_token(metadata))?);
                }
                SensorDecision::Record(reading) => {
                    measurements.push(ctx.calculation_token(metadata).emit(Measurement {
                        sensor: descriptor.id.clone(),
                        sensor_version: descriptor.version.clone(),
                        reading,
                        confidence: None,
                        sample_count: Some(inputs.observations.len()),
                        context: MeasurementContext::default(),
                    }));
                }
            }
        }
        Ok(measurements)
    }

    /// Execute all configured inferrers before any calculation sensor runs.
    pub async fn infer(
        &self,
        plan: &RunPlan,
        domain: &DomainSnapshot,
        run: InferenceRun<'_>,
    ) -> Result<InferenceResults> {
        support::require_run_id("inference", run.id)?;
        let empty_params = SharedParams::new(serde_json::json!({}));
        let mut results = InferenceResults::default();
        for inferrer in &plan.inferrers {
            let descriptor = inferrer.descriptor();
            let params = run.params.get(&descriptor.id).unwrap_or(&empty_params);
            support::require_declared_params(&descriptor.id, descriptor.params_schema, params)?;
            let ctx = unclip_plugin::InferCtx::new(run.source.clone(), domain, params, run.io);
            let metadata = EmitMetadata::new(
                DerivedId::new(format!("{}/{}", run.id, descriptor.id)),
                descriptor.id.clone(),
                descriptor.version.clone(),
                SharedParams::clone(params),
                run.timestamp.clone(),
            )
            .with_source(run.source.clone())
            .with_domain_version(domain.version.clone());
            let output = inferrer
                .infer(
                    &ctx,
                    InferenceToken::from_harness(metadata, DependencyCollector::default()),
                )
                .await?;
            results.push(output);
        }
        Ok(results)
    }

    /// Interpret tracked empirical structures in canonical plugin and source order.
    ///
    /// Every output records the exact model selector, plugin parameters, and
    /// source structure provenance identity supplied to the model.
    pub async fn interpret(
        &self,
        plan: &RunPlan,
        structures: &[Tracked<EmpiricalStructure>],
        run: InterpretationRun<'_>,
    ) -> Result<Vec<Interpreted<serde_json::Value>>> {
        if plan.interpreters.is_empty() {
            return Ok(Vec::new());
        }
        support::require_run_id("interpretation", run.id)?;
        if structures.is_empty() {
            return Err(unclip_plugin::PluginError::Message(
                "interpretation requires empirical structures".into(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for structure in structures {
            require_calculated_evidence(structure, "interpretation source structure")?;
            if structure.id().is_empty() {
                return Err(unclip_plugin::PluginError::Message(
                    "interpretation source structure ID must not be empty".into(),
                ));
            }
            if !seen.insert(structure.id()) {
                return Err(unclip_plugin::PluginError::Message(
                    "duplicate interpretation source structure".into(),
                ));
            }
        }

        let mut sources = structures.iter().collect::<Vec<_>>();
        sources.sort_by_key(|structure| structure.id());
        let mut interpreters = plan.interpreters.iter().collect::<Vec<_>>();
        interpreters.sort_by_key(|interpreter| &interpreter.descriptor().id);
        let empty_params = SharedParams::new(serde_json::json!({}));
        let mut outputs = Vec::with_capacity(sources.len() * interpreters.len());
        for interpreter in interpreters {
            let descriptor = interpreter.descriptor();
            let params = run.params.get(&descriptor.id).unwrap_or(&empty_params);
            support::require_declared_params(&descriptor.id, descriptor.params_schema, params)?;
            let model = interpreter.model_ref(params)?;
            for source in &sources {
                let ctx = InterpretCtx::new(source, params, run.io, DependencyCollector::default());
                let metadata = EmitMetadata::new(
                    DerivedId::new(format!("{}/{}/{}", run.id, descriptor.id, source.id())),
                    descriptor.id.clone(),
                    descriptor.version.clone(),
                    SharedParams::clone(params),
                    run.timestamp.clone(),
                )
                .with_model(model.clone());
                outputs.push(
                    interpreter
                        .interpret(&ctx, ctx.interpretation_token(metadata))
                        .await?,
                );
            }
        }
        Ok(outputs)
    }
}

/// Build a persistable planned-run record from the exact resolved plugins.
pub fn run_record(
    plan: &RunPlan,
    params: &PluginParams,
    id: impl Into<String>,
    started_at: Timestamp,
    metadata: serde_json::Value,
) -> unclip_record::EngineRunRecord {
    /// One family's entries, in id order.
    ///
    /// Generic over the family through [`Described`], which is the only thing
    /// a record needs from a plugin. This was written out six times — build the
    /// entries, then sort them — and the sorts lived twenty lines below the
    /// builds, so the two lists had to be kept in step by eye.
    fn entries<T: ?Sized + unclip_plugin::Described>(
        plugins: &[std::sync::Arc<T>],
        params: &PluginParams,
    ) -> Vec<serde_json::Value> {
        let mut entries = plugins
            .iter()
            .map(|plugin| {
                let id = plugin.plugin_id();
                let values = params
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| SharedParams::new(serde_json::json!({})));
                serde_json::json!({
                    "id": id,
                    "version": plugin.plugin_version(),
                    "params_hash": hash_params(&values),
                    "params": values,
                })
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
        entries
    }

    let inferrers = entries(&plan.inferrers, params);
    let sensors = entries(&plan.sensors, params);
    let comparators = entries(&plan.comparators, params);
    let interpreters = entries(&plan.interpreters, params);
    let candidate_generators = entries(&plan.candidate_generators, params);
    let null_models = entries(&plan.null_models, params);

    unclip_record::EngineRunRecord {
        id: id.into(),
        resolved_plan: serde_json::json!({
            "inferrers": inferrers,
            "sensors": sensors,
            "comparators": comparators,
            "interpreters": interpreters,
            "candidate_generators": candidate_generators,
            "null_models": null_models,
        }),
        status: unclip_record::EngineRunStatus::Planned,
        started_at: started_at.to_string(),
        completed_at: None,
        metadata,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn calculated_evidence_gate_rejects_every_other_operation() {
        use unclip_epistemic::{DerivedId, Tracked};

        let id = || DerivedId::new("evidence");
        // Labeled as calculated, or making no claim at all: accepted.
        assert!(
            super::require_calculated_evidence(&Tracked::from_calculated(id(), 1), "e").is_ok()
        );
        assert!(super::require_calculated_evidence(&Tracked::from_recorded(id(), 1), "e").is_ok());
        // Labeled as some other operation: rejected, naming what it found.
        let error = super::require_calculated_evidence(&Tracked::from_inferred(id(), 1), "e")
            .expect_err("inferred evidence must not pass a calculated-evidence gate");
        assert!(error.to_string().contains("Inferred"), "got: {error}");
    }

    use super::*;
    use std::collections::BTreeMap;
    use unclip_domain::{DomainId, FrameId};
    use unclip_epistemic::{DomainVersion, FrameVersion, PluginId};
    use unclip_measure::Reading;
    use unclip_plugin::PluginSelection;

    #[test]
    fn builtin_registry_contains_explicit_plugins() {
        let registry = builtin_registry().unwrap();
        let inferrers = registry
            .inferrers()
            .map(|plugin| plugin.descriptor().id.as_str())
            .collect::<Vec<_>>();
        let sensors = registry
            .sensors()
            .map(|plugin| plugin.descriptor().id.as_str())
            .collect::<Vec<_>>();
        let product_sensors = registry
            .product_sensors()
            .map(|plugin| plugin.descriptor().id.as_str())
            .collect::<Vec<_>>();
        let cross_product_sensors = registry
            .cross_product_sensors()
            .map(|plugin| plugin.descriptor().id.as_str())
            .collect::<Vec<_>>();
        let comparators = registry
            .comparators()
            .map(|plugin| plugin.descriptor().id.as_str())
            .collect::<Vec<_>>();
        let interpreters = registry
            .interpreters()
            .map(|plugin| plugin.descriptor().id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            inferrers,
            vec!["infer.manual", "infer.pattern", "infer.rank-pattern"]
        );
        assert_eq!(
            sensors,
            vec![
                "sensor.change-points",
                "sensor.co-foreground",
                "sensor.conditional-mutual-information",
                "sensor.coverage",
                "sensor.dtw",
                "sensor.kendall",
                "sensor.kendall-association",
                "sensor.lagged-dependency",
                "sensor.lehmer",
                "sensor.mutual-information",
                "sensor.partial-correlation",
                "sensor.permutation",
                "sensor.rbo",
                "sensor.relative-rank-variance",
                "sensor.residual",
                "sensor.spearman",
                "sensor.trajectories",
            ]
        );
        assert_eq!(
            product_sensors,
            vec![
                "sensor.canonical-correlation",
                "sensor.cross-domain-communities",
                "sensor.cross-domain-interaction-movement",
                "sensor.cross-domain-mutual-information"
            ]
        );

        assert_eq!(cross_product_sensors, vec!["sensor.cross-product-transfer"]);
        assert_eq!(
            comparators,
            vec![
                "compare.change-point-alignment",
                "compare.graph-identity",
                "compare.jensen-shannon",
                "compare.kendall",
                "compare.pairwise-matrix",
                "compare.partition-rand",
                "compare.rbo",
                "compare.scalar-difference",
                "compare.spectrum",
                "compare.structured-identity"
            ]
        );
        assert_eq!(interpreters, vec!["interpret.llm-label"]);
    }

    #[test]
    fn empty_builtin_profile_resolves() {
        let engine = Engine::with_builtins().unwrap();
        let plan = engine.plan(&EngineProfile::default()).unwrap();
        assert!(plan.sensors.is_empty());
        assert!(plan.inferrers.is_empty());
        assert!(plan.comparators.is_empty());
        assert!(plan.interpreters.is_empty());
    }

    #[test]
    fn measurement_stage_is_stable_sparse_and_versioned() {
        let engine = Engine::with_builtins().unwrap();
        let profile = EngineProfile {
            sensors: [
                "sensor.residual",
                "sensor.rbo",
                "sensor.permutation",
                "sensor.lehmer",
                "sensor.kendall",
                "sensor.coverage",
            ]
            .into_iter()
            .map(PluginSelection::any)
            .collect(),
            ..EngineProfile::default()
        };
        let plan = engine.plan(&profile).unwrap();
        let domain = DomainSnapshot {
            id: DomainId::new("test"),
            version: DomainVersion::new("domain-7"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("test.general"),
            version: FrameVersion::new("frame-3"),
            axes: Vec::new(),
        };
        let params = BTreeMap::new();
        let measurements = engine
            .measure(
                &plan,
                MeasurementInputs {
                    domain: &domain,
                    frame: &frame,
                    observations: &[],
                    alignments: &[],
                    rankings: &[],
                },
                MeasurementRun {
                    id: "run-1",
                    timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
                    params: &params,
                },
            )
            .unwrap();

        assert_eq!(measurements.len(), 6);
        assert_eq!(
            measurements
                .iter()
                .map(|measurement| measurement.value().sensor.as_str())
                .collect::<Vec<_>>(),
            vec![
                "sensor.coverage",
                "sensor.residual",
                "sensor.kendall",
                "sensor.lehmer",
                "sensor.permutation",
                "sensor.rbo",
            ]
        );
        assert!(measurements.iter().all(|measurement| matches!(
            measurement.value().reading,
            Reading::NotApplicable { .. }
        )));
        for measurement in measurements {
            assert_eq!(
                measurement.provenance().domain_version,
                Some(DomainVersion::new("domain-7"))
            );
            assert_eq!(
                measurement.provenance().frame_version,
                Some(FrameVersion::new("frame-3"))
            );
            assert!(measurement.provenance().inputs.is_empty());
        }
    }

    /// A sensor handed parameters its schema rejects fails the run, as every
    /// other family does, instead of recording a `NotApplicable` reading.
    #[test]
    fn measurement_refuses_parameters_a_sensor_schema_rejects() {
        let engine = Engine::with_builtins().unwrap();
        let profile = EngineProfile {
            sensors: vec![PluginSelection::any("sensor.coverage")],
            ..EngineProfile::default()
        };
        let plan = engine.plan(&profile).unwrap();
        let domain = DomainSnapshot {
            id: DomainId::new("test"),
            version: DomainVersion::new("domain-1"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("test.general"),
            version: FrameVersion::new("frame-1"),
            axes: Vec::new(),
        };
        let params = BTreeMap::from([(
            PluginId::new("sensor.coverage"),
            serde_json::json!({"typo": 1}).into(),
        )]);
        let error = engine
            .measure(
                &plan,
                MeasurementInputs {
                    domain: &domain,
                    frame: &frame,
                    observations: &[],
                    alignments: &[],
                    rankings: &[],
                },
                MeasurementRun {
                    id: "run-schema",
                    timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
                    params: &params,
                },
            )
            .expect_err("a sensor must be held to its declared schema");
        assert!(
            matches!(error, unclip_plugin::PluginError::InvalidParams(_)),
            "got: {error:?}"
        );
        assert!(error.to_string().contains("typo"), "got: {error}");
    }

    /// Every stage mints `{run_id}/{plugin_id}`, so every stage must refuse a
    /// run id that is blank — including one that is whitespace rather than
    /// empty. `measure` rejected both, `interpret` only the empty case, and
    /// `infer` neither; this pins all three to one rule.
    #[tokio::test]
    async fn every_stage_refuses_a_blank_run_id() {
        let engine = Engine::with_builtins().unwrap();
        let profile = EngineProfile {
            inferrers: vec![PluginSelection::any("infer.pattern")],
            sensors: vec![PluginSelection::any("sensor.coverage")],
            interpreters: vec![PluginSelection::any("interpret.llm-label")],
            ..EngineProfile::default()
        };
        let plan = engine.plan(&profile).unwrap();
        let domain = DomainSnapshot {
            id: DomainId::new("blank"),
            version: DomainVersion::new("domain-1"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("blank.general"),
            version: FrameVersion::new("frame-1"),
            axes: Vec::new(),
        };
        let params = BTreeMap::new();
        let structures = [Tracked::from_calculated(
            DerivedId::new("structure/1"),
            EmpiricalStructure {
                kind: "latent-axis".into(),
                value: serde_json::json!({}),
            },
        )];

        for id in ["", "   ", "\t\n"] {
            let measure = engine
                .measure(
                    &plan,
                    MeasurementInputs {
                        domain: &domain,
                        frame: &frame,
                        observations: &[],
                        alignments: &[],
                        rankings: &[],
                    },
                    MeasurementRun {
                        id,
                        timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
                        params: &params,
                    },
                )
                .expect_err("measurement must refuse a blank run id");
            assert!(
                measure.to_string().contains("measurement requires"),
                "got: {measure}"
            );

            let infer = engine
                .infer(
                    &plan,
                    &domain,
                    InferenceRun {
                        id,
                        source: SourceRef::new("notes/blank.txt"),
                        timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
                        params: &params,
                        io: &OrdinaryTextIo,
                    },
                )
                .await
                .expect_err("inference must refuse a blank run id");
            assert!(
                infer.to_string().contains("inference requires"),
                "got: {infer}"
            );

            let interpret = engine
                .interpret(
                    &plan,
                    &structures,
                    InterpretationRun {
                        id,
                        timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
                        params: &params,
                        io: &RefusingInterpretationIo,
                    },
                )
                .await
                .expect_err("interpretation must refuse a blank run id");
            assert!(
                interpret.to_string().contains("interpretation requires"),
                "got: {interpret}"
            );
        }
    }

    /// A declared `params_schema` has to bind every family, not just sensors.
    ///
    /// `classify_sensor` held the `Sensor` family to its schema and nothing
    /// held the other five: an inferrer, comparator, interpreter, candidate
    /// generator or null model could be handed parameters its own descriptor
    /// forbids and would run anyway. Each of these assertions fails if that
    /// enforcement is removed from one stage.
    #[tokio::test]
    async fn every_family_is_held_to_its_declared_params_schema() {
        let engine = Engine::with_builtins().unwrap();
        let profile = EngineProfile {
            inferrers: vec![PluginSelection::any("infer.pattern")],
            comparators: vec![PluginSelection::any("compare.scalar-difference")],
            interpreters: vec![PluginSelection::any("interpret.llm-label")],
            ..EngineProfile::default()
        };
        let plan = engine.plan(&profile).unwrap();
        let domain = DomainSnapshot {
            id: DomainId::new("schema"),
            version: DomainVersion::new("domain-1"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };

        // `min_confidence` is declared `number, 0.0..=1.0`.
        let inferrer_params = BTreeMap::from([(
            PluginId::new("infer.pattern"),
            serde_json::json!({"min_confidence": 4}).into(),
        )]);
        let error = engine
            .infer(
                &plan,
                &domain,
                InferenceRun {
                    id: "run-schema",
                    source: SourceRef::new("notes/schema.txt"),
                    timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
                    params: &inferrer_params,
                    io: &OrdinaryTextIo,
                },
            )
            .await
            .expect_err("an inferrer must be held to its declared schema");
        assert!(
            matches!(error, unclip_plugin::PluginError::InvalidParams(_)),
            "got: {error:?}"
        );
        assert!(error.to_string().contains("min_confidence"), "got: {error}");

        // `model` and `model_version` are declared required.
        let interpreter_params = BTreeMap::from([(
            PluginId::new("interpret.llm-label"),
            serde_json::json!({"model": "m"}).into(),
        )]);
        let structures = [Tracked::from_calculated(
            DerivedId::new("structure/1"),
            EmpiricalStructure {
                kind: "latent-axis".into(),
                value: serde_json::json!({}),
            },
        )];
        let error = engine
            .interpret(
                &plan,
                &structures,
                InterpretationRun {
                    id: "run-schema",
                    timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
                    params: &interpreter_params,
                    io: &RefusingInterpretationIo,
                },
            )
            .await
            .expect_err("an interpreter must be held to its declared schema");
        assert!(
            matches!(error, unclip_plugin::PluginError::InvalidParams(_)),
            "got: {error:?}"
        );

        // `compare.scalar-difference` declares no parameters at all.
        let comparator_params = BTreeMap::from([(
            PluginId::new("compare.scalar-difference"),
            serde_json::json!({"tolerance": 0.5}).into(),
        )]);
        let measurement = |id: &str| {
            Tracked::from_calculated(
                DerivedId::new(id),
                Measurement {
                    sensor: PluginId::new("sensor.coverage"),
                    sensor_version: semver::Version::new(0, 1, 0),
                    reading: Reading::Value {
                        value: unclip_measure::MeasurementValue::Scalar(1.0),
                    },
                    confidence: None,
                    sample_count: Some(1),
                    context: unclip_measure::MeasurementContext::default(),
                },
            )
        };
        let error = compare_measurements(
            &plan,
            &measurement("before"),
            &measurement("after"),
            MeasurementRun {
                id: "run-schema",
                timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
                params: &comparator_params,
            },
        )
        .expect_err("a comparator must be held to its declared schema");
        assert!(
            matches!(error, unclip_plugin::PluginError::InvalidParams(_)),
            "got: {error:?}"
        );
        assert!(error.to_string().contains("tolerance"), "got: {error}");
    }

    /// Fails if it is ever called: the blank-run-id checks must refuse before
    /// any stage reaches its I/O.
    struct RefusingInterpretationIo;

    #[async_trait::async_trait]
    impl unclip_plugin::InterpretationIo for RefusingInterpretationIo {
        async fn request(
            &self,
            _request: &unclip_plugin::InterpretationRequest,
        ) -> unclip_plugin::Result<serde_json::Value> {
            panic!("interpretation I/O must not be reached for a blank run id")
        }
    }

    struct OrdinaryTextIo;

    #[async_trait::async_trait]
    impl unclip_plugin::InferenceIo for OrdinaryTextIo {
        async fn request(
            &self,
            _source: &SourceRef,
            params: &serde_json::Value,
        ) -> unclip_plugin::Result<serde_json::Value> {
            let fixture: serde_json::Value =
                serde_json::from_str(include_str!("../tests/fixtures/milestone1_pipeline.json"))
                    .map_err(|error| unclip_plugin::PluginError::Message(error.to_string()))?;
            let key = if params.get("min_confidence").is_some() {
                "pattern_input"
            } else {
                "ranking_input"
            };
            Ok(fixture[key].clone())
        }
    }

    #[tokio::test]
    async fn ordinary_text_is_ranked_before_state_sensors_run() {
        use unclip_domain::{FrameAxis, Relation, Unit, UnitId, UnitKind};
        use unclip_measure::MeasurementValue;

        let engine = Engine::with_builtins().unwrap();
        let profile = EngineProfile {
            inferrers: vec![
                PluginSelection::any("infer.pattern"),
                PluginSelection::any("infer.rank-pattern"),
            ],
            sensors: vec![
                PluginSelection::any("sensor.coverage"),
                PluginSelection::any("sensor.residual"),
                PluginSelection::any("sensor.permutation"),
            ],
            ..EngineProfile::default()
        };
        let plan = engine.plan(&profile).unwrap();
        let u1 = UnitId::new("u1");
        let u2 = UnitId::new("u2");
        let domain = DomainSnapshot {
            id: DomainId::new("ordinary"),
            version: DomainVersion::new("domain-1"),
            units: [
                (
                    u1.clone(),
                    Unit {
                        id: u1.clone(),
                        kind: UnitKind::AtomicMeaning,
                        label: None,
                        properties: BTreeMap::new(),
                    },
                ),
                (
                    u2.clone(),
                    Unit {
                        id: u2.clone(),
                        kind: UnitKind::AtomicMeaning,
                        label: None,
                        properties: BTreeMap::new(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
            relations: BTreeMap::<unclip_domain::RelationId, Relation>::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("ordinary.general"),
            version: FrameVersion::new("frame-1"),
            axes: vec![
                FrameAxis {
                    unit: u1.clone(),
                    label: None,
                },
                FrameAxis {
                    unit: u2.clone(),
                    label: None,
                },
            ],
        };
        let params = BTreeMap::from([
            (
                PluginId::new("infer.pattern"),
                SharedParams::new(serde_json::json!({"min_confidence": 0.7})),
            ),
            (
                PluginId::new("infer.rank-pattern"),
                SharedParams::new(
                    serde_json::json!({"ties": "preserve", "unknown_tail": "preserve"}),
                ),
            ),
        ]);
        let results = engine
            .execute(
                &plan,
                &domain,
                &frame,
                InferenceRun {
                    id: "run-text",
                    source: SourceRef::new("notes/ordinary.txt"),
                    timestamp: Timestamp::new("2026-09-18T00:00:00Z"),
                    params: &params,
                    io: &OrdinaryTextIo,
                },
            )
            .await
            .unwrap();

        let golden: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/milestone1_pipeline.json"))
                .unwrap();
        let state = match &results.measurements[0].value().reading {
            Reading::Value {
                value: MeasurementValue::Ranking(state),
            } => state,
            other => panic!("expected ranking measurement, got {other:?}"),
        };
        let actual = serde_json::json!({
            "inference_outputs": results.inference.outputs.len(),
            "observations": results.inference.observations.len(),
            "alignments": results.inference.alignments.len(),
            "rankings": results.inference.rankings.len(),
            "explanation_sensors": results
                .explanations
                .iter()
                .map(|value| value.value().sensor.to_string())
                .collect::<Vec<_>>(),
            "residual_sensors": results
                .residuals
                .iter()
                .map(|value| value.value().sensor.to_string())
                .collect::<Vec<_>>(),
            "measurement_sensors": results
                .measurements
                .iter()
                .map(|value| value.value().sensor.to_string())
                .collect::<Vec<_>>(),
            "ranking_tiers": state.tiers,
            "ranking_unknown": state.unknown,
        });
        assert_eq!(actual, golden["expected"]);

        assert_eq!(results.inference.outputs.len(), 2);
        assert_eq!(results.inference.observations.len(), 2);
        assert_eq!(results.inference.alignments.len(), 1);
        assert_eq!(results.inference.rankings.len(), 1);
        assert_eq!(
            results.inference.rankings[0].id(),
            &DerivedId::new("run-text/infer.rank-pattern")
        );
        assert!(!results.explanations.is_empty());
        assert!(results
            .explanations
            .iter()
            .all(|value| value.value().sensor == PluginId::new("sensor.coverage")));
        assert!(!results.residuals.is_empty());
        assert!(results
            .residuals
            .iter()
            .all(|value| value.value().sensor == PluginId::new("sensor.residual")));
        assert_eq!(results.measurements.len(), 1);
        assert!(matches!(
            &results.measurements[0].value().reading,
            Reading::Value {
                value: MeasurementValue::Ranking(state)
            } if state.tiers == vec![vec![u1]]
                && state.unknown == vec![u2]
                && state.unresolved.is_empty()
        ));
        assert_eq!(
            results.measurements[0].provenance().inputs,
            vec![
                DerivedId::new("run-text/infer.pattern"),
                DerivedId::new("run-text/infer.rank-pattern")
            ]
        );

        let mut replay = unclip_record::EngineRunReplay {
            run: run_record(
                &plan,
                &params,
                "run-text",
                Timestamp::new("2026-09-18T00:00:00Z"),
                serde_json::json!({}),
            ),
            sensor_runs: Vec::new(),
            provenance_ids: results
                .inference
                .outputs
                .iter()
                .map(|output| output.id().to_string())
                .collect(),
            profile_ids: Vec::new(),
            observations: Vec::new(),
            alignments: Vec::new(),
            rankings: Vec::new(),
        };
        for output in &results.inference.outputs {
            if let unclip_plugin::InferenceOutput::Bundle {
                observations,
                alignments,
                rankings,
            } = output.value()
            {
                replay.observations.extend(observations.iter().map(|value| {
                    unclip_record::RecordedInference::shared(output.id().clone(), Arc::clone(value))
                }));
                replay.alignments.extend(alignments.iter().map(|value| {
                    unclip_record::RecordedInference::shared(output.id().clone(), Arc::clone(value))
                }));
                replay.rankings.extend(rankings.iter().map(|value| {
                    unclip_record::RecordedInference::shared(output.id().clone(), Arc::clone(value))
                }));
            }
        }
        let expected = results
            .explanations
            .iter()
            .chain(&results.residuals)
            .chain(&results.measurements)
            .cloned()
            .collect::<Vec<_>>();
        let verified = engine
            .verify(
                &plan,
                &domain,
                &frame,
                &replay,
                MeasurementRun {
                    id: "run-text",
                    timestamp: Timestamp::new("2026-09-18T00:00:00Z"),
                    params: &params,
                },
            )
            .unwrap();
        assert_eq!(verified, expected);
    }
    #[test]
    fn run_record_captures_resolved_plugins_parameters_and_hashes() {
        let engine = Engine::with_builtins().unwrap();
        let profile = EngineProfile {
            inferrers: vec![PluginSelection::any("infer.pattern")],
            sensors: vec![PluginSelection::any("sensor.coverage")],
            ..EngineProfile::default()
        };
        let plan = engine.plan(&profile).unwrap();
        let sensor_params = SharedParams::new(serde_json::json!({}));
        let inference_params = SharedParams::new(serde_json::json!({"min_confidence": 0.4}));
        let params = BTreeMap::from([
            (
                PluginId::new("sensor.coverage"),
                SharedParams::clone(&sensor_params),
            ),
            (
                PluginId::new("infer.pattern"),
                SharedParams::clone(&inference_params),
            ),
        ]);

        let record = run_record(
            &plan,
            &params,
            "run-record",
            Timestamp::new("2026-09-18T00:00:00Z"),
            serde_json::json!({"source": "notes.txt"}),
        );

        assert_eq!(record.status, unclip_record::EngineRunStatus::Planned);
        assert_eq!(record.started_at, "2026-09-18T00:00:00Z");
        assert_eq!(record.metadata, serde_json::json!({"source": "notes.txt"}));
        assert_eq!(
            record.resolved_plan["inferrers"][0],
            serde_json::json!({
                "id": "infer.pattern",
                "version": "1.0.0",
                "params": inference_params,
                "params_hash": hash_params(&serde_json::json!({"min_confidence": 0.4}))
            })
        );
        assert_eq!(
            record.resolved_plan["sensors"][0],
            serde_json::json!({
                "id": "sensor.coverage",
                "version": "0.1.0",
                "params": sensor_params,
                "params_hash": hash_params(&serde_json::json!({}))
            })
        );
        assert_eq!(record.resolved_plan["comparators"], serde_json::json!([]));
    }
}
