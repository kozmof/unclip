//! Plugin contracts, capability-aware contexts, and explicit runtime registry.

#![forbid(unsafe_code)]

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use semver::Version;
use thiserror::Error;
use unclip_domain::{DomainSnapshot, MeasurementFrame};
use unclip_epistemic::{
    Calculated, CalculationToken, DependencyCollector, ExperimentToken, Experimental, FrameVersion,
    InferenceToken, Inferred, InterpretationToken, Interpreted, PluginId, SourceRef, Tracked,
};
use unclip_measure::{Delta, EmpiricalStructure, Measurement, MeasurementKind, Reading};
use unclip_observe::{Alignment, Observation, PartialRanking};

pub type Params = serde_json::Value;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PluginError {
    #[error("plugin id is already registered: {0}")]
    DuplicatePlugin(PluginId),
    #[error("configured plugin is not registered: {0}")]
    MissingPlugin(PluginId),
    #[error("plugin {plugin} does not support measurement kind {kind:?}")]
    UnsupportedKind {
        plugin: PluginId,
        kind: MeasurementKind,
    },
    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, PluginError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Observation,
    Alignment,
    RankingValue,
    GraphValue,
    MultiObservation,
    Ordered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceRequirement {
    TotalOrder,
    MinSamples(usize),
    Ordered,
    ConditioningVariables,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applicability {
    Applicable,
    NotApplicable { reason: String },
}

#[derive(Debug, Clone)]
pub struct SensorDescriptor {
    pub id: PluginId,
    pub version: Version,
    pub applicability: &'static [Capability],
    pub evidence: &'static [EvidenceRequirement],
    pub produces: &'static [MeasurementKind],
    pub params_schema: &'static str,
}

#[derive(Debug, Clone)]
pub struct InferrerDescriptor {
    pub id: PluginId,
    pub version: Version,
    pub params_schema: &'static str,
}

#[derive(Debug, Clone)]
pub struct ComparatorDescriptor {
    pub id: PluginId,
    pub version: Version,
    pub supports: &'static [MeasurementKind],
    pub params_schema: &'static str,
}

#[derive(Debug, Clone)]
pub struct PluginDescriptor {
    pub id: PluginId,
    pub version: Version,
    pub params_schema: &'static str,
}

pub struct MeasureCtx<'a> {
    domain: &'a DomainSnapshot,
    frame: &'a MeasurementFrame,
    observations: &'a [Tracked<Observation>],
    alignments: &'a [Tracked<Alignment>],
    rankings: &'a [Tracked<PartialRanking>],
    params: &'a Params,
    dependencies: DependencyCollector,
}

impl<'a> MeasureCtx<'a> {
    pub fn new(
        domain: &'a DomainSnapshot,
        frame: &'a MeasurementFrame,
        observations: &'a [Tracked<Observation>],
        alignments: &'a [Tracked<Alignment>],
        rankings: &'a [Tracked<PartialRanking>],
        params: &'a Params,
        dependencies: DependencyCollector,
    ) -> Self {
        Self {
            domain,
            frame,
            observations,
            alignments,
            rankings,
            params,
            dependencies,
        }
    }

    pub fn domain(&self) -> &DomainSnapshot {
        self.domain
    }

    pub fn frame(&self) -> &MeasurementFrame {
        self.frame
    }

    pub fn params(&self) -> &Params {
        self.params
    }

    pub fn observations(&self) -> &[Tracked<Observation>] {
        self.observations
    }

    pub fn alignments(&self) -> &[Tracked<Alignment>] {
        self.alignments
    }

    pub fn rankings(&self) -> &[Tracked<PartialRanking>] {
        self.rankings
    }

    pub fn read<'b, T>(&self, input: &'b Tracked<T>) -> &'b T {
        self.dependencies.read(input)
    }

    pub fn dependencies(&self) -> DependencyCollector {
        self.dependencies.clone()
    }

    pub fn frame_version(&self) -> FrameVersion {
        self.frame.version.clone()
    }
}

pub struct InferCtx<'a> {
    pub source: SourceRef,
    pub domain: &'a DomainSnapshot,
    pub params: &'a Params,
    pub io: &'a dyn InferenceIo,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InferenceOutput {
    Observations(Vec<Observation>),
    Alignments(Vec<Alignment>),
    Rankings(Vec<PartialRanking>),
    Structured(serde_json::Value),
}

#[async_trait]
pub trait InferenceIo: Send + Sync {
    async fn request(&self, source: &SourceRef, params: &Params) -> Result<serde_json::Value>;
}

pub trait Sensor: Send + Sync {
    fn descriptor(&self) -> &SensorDescriptor;
    fn applies_to(&self, ctx: &MeasureCtx<'_>) -> Applicability;
    fn measure(
        &self,
        ctx: &MeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<Measurement>>>;
}

#[async_trait]
pub trait Inferrer: Send + Sync {
    fn descriptor(&self) -> &InferrerDescriptor;
    async fn infer(
        &self,
        ctx: &InferCtx<'_>,
        token: InferenceToken,
    ) -> Result<Inferred<InferenceOutput>>;
}

pub trait Comparator: Send + Sync {
    fn descriptor(&self) -> &ComparatorDescriptor;
    fn compare(
        &self,
        before: &Reading,
        after: &Reading,
        token: CalculationToken,
    ) -> Result<Calculated<Delta>>;
}

pub trait Interpreter: Send + Sync {
    fn descriptor(&self) -> &PluginDescriptor;
    fn interpret(
        &self,
        structure: &EmpiricalStructure,
        token: InterpretationToken,
    ) -> Result<Interpreted<serde_json::Value>>;
}

pub trait Experimenter: Send + Sync {
    fn descriptor(&self) -> &PluginDescriptor;
    fn experiment(
        &self,
        candidate: &serde_json::Value,
        token: ExperimentToken,
    ) -> Result<Experimental<serde_json::Value>>;
}

pub trait CandidateGenerator: Send + Sync {
    fn descriptor(&self) -> &PluginDescriptor;
    fn generate(&self, structure: &EmpiricalStructure) -> Result<Vec<serde_json::Value>>;
}

pub trait NullModel: Send + Sync {
    fn descriptor(&self) -> &PluginDescriptor;
    fn evaluate(&self, candidate: &serde_json::Value) -> Result<Reading>;
}

#[derive(Default)]
pub struct Registry {
    sensors: BTreeMap<PluginId, Arc<dyn Sensor>>,
    inferrers: BTreeMap<PluginId, Arc<dyn Inferrer>>,
    comparators: BTreeMap<PluginId, Arc<dyn Comparator>>,
    interpreters: BTreeMap<PluginId, Arc<dyn Interpreter>>,
    generators: BTreeMap<PluginId, Arc<dyn CandidateGenerator>>,
    null_models: BTreeMap<PluginId, Arc<dyn NullModel>>,
}

impl Registry {
    pub fn with_builtins() -> Self {
        Self::default()
    }

    pub fn register_sensor(&mut self, plugin: Arc<dyn Sensor>) -> Result<()> {
        let id = plugin.descriptor().id.clone();
        insert_unique(&mut self.sensors, id, plugin)
    }

    pub fn register_inferrer(&mut self, plugin: Arc<dyn Inferrer>) -> Result<()> {
        let id = plugin.descriptor().id.clone();
        insert_unique(&mut self.inferrers, id, plugin)
    }

    pub fn register_comparator(&mut self, plugin: Arc<dyn Comparator>) -> Result<()> {
        let id = plugin.descriptor().id.clone();
        insert_unique(&mut self.comparators, id, plugin)
    }

    pub fn register_interpreter(&mut self, plugin: Arc<dyn Interpreter>) -> Result<()> {
        let id = plugin.descriptor().id.clone();
        insert_unique(&mut self.interpreters, id, plugin)
    }

    pub fn register_generator(&mut self, plugin: Arc<dyn CandidateGenerator>) -> Result<()> {
        let id = plugin.descriptor().id.clone();
        insert_unique(&mut self.generators, id, plugin)
    }

    pub fn register_null_model(&mut self, plugin: Arc<dyn NullModel>) -> Result<()> {
        let id = plugin.descriptor().id.clone();
        insert_unique(&mut self.null_models, id, plugin)
    }

    pub fn sensors(&self) -> impl Iterator<Item = &Arc<dyn Sensor>> {
        self.sensors.values()
    }

    pub fn inferrers(&self) -> impl Iterator<Item = &Arc<dyn Inferrer>> {
        self.inferrers.values()
    }

    pub fn comparators(&self) -> impl Iterator<Item = &Arc<dyn Comparator>> {
        self.comparators.values()
    }

    pub fn resolve(&self, profile: &EngineProfile) -> Result<RunPlan> {
        Ok(RunPlan {
            sensors: resolve_ids(&self.sensors, &profile.sensors)?,
            inferrers: resolve_ids(&self.inferrers, &profile.inferrers)?,
            comparators: resolve_ids(&self.comparators, &profile.comparators)?,
        })
    }
}

fn insert_unique<T: ?Sized>(
    entries: &mut BTreeMap<PluginId, Arc<T>>,
    id: PluginId,
    plugin: Arc<T>,
) -> Result<()> {
    if entries.contains_key(&id) {
        return Err(PluginError::DuplicatePlugin(id));
    }
    entries.insert(id, plugin);
    Ok(())
}

fn resolve_ids<T: ?Sized>(
    entries: &BTreeMap<PluginId, Arc<T>>,
    ids: &[PluginId],
) -> Result<Vec<Arc<T>>> {
    ids.iter()
        .map(|id| {
            entries
                .get(id)
                .cloned()
                .ok_or_else(|| PluginError::MissingPlugin(id.clone()))
        })
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct EngineProfile {
    pub sensors: Vec<PluginId>,
    pub inferrers: Vec<PluginId>,
    pub comparators: Vec<PluginId>,
}

pub struct RunPlan {
    pub sensors: Vec<Arc<dyn Sensor>>,
    pub inferrers: Vec<Arc<dyn Inferrer>>,
    pub comparators: Vec<Arc<dyn Comparator>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubSensor(SensorDescriptor);

    impl Sensor for StubSensor {
        fn descriptor(&self) -> &SensorDescriptor {
            &self.0
        }

        fn applies_to(&self, _ctx: &MeasureCtx<'_>) -> Applicability {
            Applicability::Applicable
        }

        fn measure(
            &self,
            _ctx: &MeasureCtx<'_>,
            _token: CalculationToken,
        ) -> Result<Vec<Calculated<Measurement>>> {
            Ok(Vec::new())
        }
    }

    fn sensor() -> Arc<dyn Sensor> {
        Arc::new(StubSensor(SensorDescriptor {
            id: PluginId::new("sensor.stub"),
            version: Version::new(0, 1, 0),
            applicability: &[],
            evidence: &[],
            produces: &[],
            params_schema: "{}",
        }))
    }

    #[test]
    fn registration_rejects_duplicate_ids() {
        let mut registry = Registry::default();
        registry.register_sensor(sensor()).unwrap();
        assert_eq!(
            registry.register_sensor(sensor()).unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );
    }

    #[test]
    fn resolution_reports_missing_plugins() {
        let registry = Registry::default();
        let profile = EngineProfile {
            sensors: vec![PluginId::new("sensor.missing")],
            ..EngineProfile::default()
        };
        assert_eq!(
            registry.resolve(&profile).err().unwrap(),
            PluginError::MissingPlugin(PluginId::new("sensor.missing"))
        );
    }
}
