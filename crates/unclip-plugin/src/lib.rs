//! Plugin contracts, capability-aware contexts, and explicit runtime registry.

#![forbid(unsafe_code)]

mod candidate;
pub use candidate::CandidateCtx;

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use semver::{Version, VersionReq};
use thiserror::Error;
use unclip_domain::{DomainSnapshot, MeasurementFrame};
use unclip_epistemic::{
    Calculated, CalculationToken, DependencyCollector, EmitMetadata, ExperimentToken, Experimental,
    FrameVersion, InferenceToken, Inferred, InterpretationToken, Interpreted, PluginId, SourceRef,
    Tracked,
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
    #[error("plugin {plugin} version {actual} does not satisfy {required}")]
    IncompatibleVersion {
        plugin: PluginId,
        required: VersionReq,
        actual: Version,
    },
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
    /// Minimum input observations; sensors must also check complete usable samples.
    MinSamples(usize),
    Ordered,
    /// An explicitly configured observation sequence; the sensor validates its contents.
    ExplicitOrder,
    /// At least one distinct, nonempty conditioning-variable name in parameters.
    ConditioningVariables,
    /// Minimum distinct configured conditioning variables. Sensors validate their data.
    MinConditioningVariables(usize),
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

    pub fn calculation_token(&self, mut metadata: EmitMetadata) -> CalculationToken {
        metadata.domain_version = Some(self.domain.version.clone());
        metadata.frame_version = Some(self.frame.version.clone());
        CalculationToken::from_harness(metadata, self.dependencies.clone())
    }

    pub fn frame_version(&self) -> FrameVersion {
        self.frame.version.clone()
    }

    pub fn evidence_gap(&self, requirement: EvidenceRequirement) -> Option<EvidenceGap> {
        match requirement {
            EvidenceRequirement::TotalOrder => {
                let have = usize::from(
                    self.rankings
                        .iter()
                        .any(|ranking| self.read(ranking).is_total()),
                );
                (have < 1).then_some(EvidenceGap {
                    requirement,
                    have,
                    need: 1,
                })
            }
            EvidenceRequirement::MinSamples(need) => {
                let have = self.observations.len();
                (have < need).then_some(EvidenceGap {
                    requirement,
                    have,
                    need,
                })
            }
            EvidenceRequirement::Ordered => {
                let have = self
                    .observations
                    .iter()
                    .filter(|observation| self.read(observation).observed_at.is_some())
                    .count();
                let need = self.observations.len();
                (have < need).then_some(EvidenceGap {
                    requirement,
                    have,
                    need,
                })
            }
            EvidenceRequirement::ExplicitOrder => {
                let have = usize::from(
                    self.params
                        .get("sequence")
                        .is_some_and(|value| !value.is_null()),
                );
                (have == 0).then_some(EvidenceGap {
                    requirement,
                    have,
                    need: 1,
                })
            }
            EvidenceRequirement::ConditioningVariables
            | EvidenceRequirement::MinConditioningVariables(_) => {
                let need = match requirement {
                    EvidenceRequirement::MinConditioningVariables(need) => need,
                    _ => 1,
                };
                let have = self
                    .params
                    .get("conditioning_variables")
                    .and_then(serde_json::Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .filter(|name| !name.trim().is_empty())
                            .collect::<std::collections::BTreeSet<_>>()
                            .len()
                    })
                    .unwrap_or(0);
                (have < need).then_some(EvidenceGap {
                    requirement,
                    have,
                    need,
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvidenceGap {
    pub requirement: EvidenceRequirement,
    pub have: usize,
    pub need: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SensorDecision {
    Run,
    Record(Reading),
}

pub fn classify_sensor(
    sensor: &dyn Sensor,
    ctx: &MeasureCtx<'_>,
    scheduled: bool,
) -> SensorDecision {
    if !scheduled {
        return SensorDecision::Record(Reading::NotMeasured);
    }
    if let Applicability::NotApplicable { reason } = sensor.applies_to(ctx) {
        return SensorDecision::Record(Reading::NotApplicable { reason });
    }
    if let Some(gap) = sensor
        .descriptor()
        .evidence
        .iter()
        .find_map(|requirement| ctx.evidence_gap(*requirement))
    {
        return SensorDecision::Record(Reading::InsufficientEvidence {
            have: gap.have,
            need: gap.need,
        });
    }
    SensorDecision::Run
}

pub struct InferCtx<'a> {
    pub source: SourceRef,
    pub domain: &'a DomainSnapshot,
    pub params: &'a Params,
    pub io: &'a dyn InferenceIo,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InferenceOutput {
    Bundle {
        observations: Vec<Observation>,
        alignments: Vec<Alignment>,
        rankings: Vec<PartialRanking>,
    },
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
    fn generate(
        &self,
        ctx: &CandidateCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<unclip_domain::CandidateProposal>>>;
}

pub trait NullModel: Send + Sync {
    fn descriptor(&self) -> &PluginDescriptor;
    fn evaluate(&self, candidate: &serde_json::Value) -> Result<Reading>;
}

/// Reusable checks for first-party and cooperative third-party sensors.
pub mod conformance {
    use super::{
        classify_sensor, Calculated, MeasureCtx, Measurement, MeasurementKind, Result, Sensor,
        SensorDecision,
    };

    /// Run a sensor twice through the supplied fixture and assert the common
    /// deterministic and descriptor contracts.
    pub fn assert_planning(
        sensor: &dyn Sensor,
        ctx: &MeasureCtx<'_>,
        scheduled: bool,
        expected: SensorDecision,
    ) {
        assert_eq!(classify_sensor(sensor, ctx, scheduled), expected);
    }

    pub fn assert_sensor<F>(sensor: &dyn Sensor, mut run: F)
    where
        F: FnMut(&dyn Sensor) -> Result<Vec<Calculated<Measurement>>>,
    {
        let first = run(sensor).expect("sensor fixture failed on first run");
        let second = run(sensor).expect("sensor fixture failed on repeated run");
        assert_eq!(first, second, "sensor output is not deterministic");

        for derived in &first {
            if let super::Reading::Value { value } = &derived.value().reading {
                let kind: MeasurementKind = value.kind();
                assert!(
                    sensor.descriptor().produces.contains(&kind),
                    "sensor emitted undeclared measurement kind {kind:?}"
                );
            }
            assert_eq!(
                derived.value().sensor,
                sensor.descriptor().id,
                "measurement carries the wrong sensor id"
            );
            assert_eq!(
                derived.value().sensor_version,
                sensor.descriptor().version,
                "measurement carries the wrong sensor version"
            );
        }
    }
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

    pub fn candidate_generators(&self) -> impl Iterator<Item = &Arc<dyn CandidateGenerator>> {
        self.generators.values()
    }

    pub fn null_models(&self) -> impl Iterator<Item = &Arc<dyn NullModel>> {
        self.null_models.values()
    }

    pub fn resolve(&self, profile: &EngineProfile) -> Result<RunPlan> {
        let mut ids = std::collections::BTreeSet::new();
        for selection in profile
            .sensors
            .iter()
            .chain(&profile.inferrers)
            .chain(&profile.comparators)
            .chain(&profile.candidate_generators)
            .chain(&profile.null_models)
        {
            if !ids.insert(&selection.id) {
                return Err(PluginError::DuplicatePlugin(selection.id.clone()));
            }
        }
        Ok(RunPlan {
            sensors: resolve_ids(&self.sensors, &profile.sensors, |plugin| {
                &plugin.descriptor().version
            })?,
            inferrers: resolve_ids(&self.inferrers, &profile.inferrers, |plugin| {
                &plugin.descriptor().version
            })?,
            comparators: resolve_ids(&self.comparators, &profile.comparators, |plugin| {
                &plugin.descriptor().version
            })?,
            candidate_generators: resolve_ids(
                &self.generators,
                &profile.candidate_generators,
                |plugin| &plugin.descriptor().version,
            )?,
            null_models: resolve_ids(&self.null_models, &profile.null_models, |plugin| {
                &plugin.descriptor().version
            })?,
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
    selections: &[PluginSelection],
    version_of: impl Fn(&T) -> &Version,
) -> Result<Vec<Arc<T>>> {
    selections
        .iter()
        .map(|selection| {
            let plugin = entries
                .get(&selection.id)
                .ok_or_else(|| PluginError::MissingPlugin(selection.id.clone()))?;
            let actual = version_of(plugin);
            if !selection.version.matches(actual) {
                return Err(PluginError::IncompatibleVersion {
                    plugin: selection.id.clone(),
                    required: selection.version.clone(),
                    actual: actual.clone(),
                });
            }
            Ok(plugin.clone())
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct PluginSelection {
    pub id: PluginId,
    pub version: VersionReq,
}

impl PluginSelection {
    pub fn any(id: impl Into<String>) -> Self {
        Self {
            id: PluginId::new(id),
            version: VersionReq::STAR,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct EngineProfile {
    pub sensors: Vec<PluginSelection>,
    pub inferrers: Vec<PluginSelection>,
    pub comparators: Vec<PluginSelection>,
    pub candidate_generators: Vec<PluginSelection>,
    pub null_models: Vec<PluginSelection>,
}

pub struct RunPlan {
    pub sensors: Vec<Arc<dyn Sensor>>,
    pub inferrers: Vec<Arc<dyn Inferrer>>,
    pub comparators: Vec<Arc<dyn Comparator>>,
    pub candidate_generators: Vec<Arc<dyn CandidateGenerator>>,
    pub null_models: Vec<Arc<dyn NullModel>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubSensor {
        descriptor: SensorDescriptor,
        applicable: bool,
    }

    impl Sensor for StubSensor {
        fn descriptor(&self) -> &SensorDescriptor {
            &self.descriptor
        }

        fn applies_to(&self, _ctx: &MeasureCtx<'_>) -> Applicability {
            if self.applicable {
                Applicability::Applicable
            } else {
                Applicability::NotApplicable {
                    reason: "unsupported fixture".into(),
                }
            }
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
        sensor_with(&[], true)
    }

    fn sensor_with(evidence: &'static [EvidenceRequirement], applicable: bool) -> Arc<dyn Sensor> {
        Arc::new(StubSensor {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.stub"),
                version: Version::new(0, 1, 0),
                applicability: &[],
                evidence,
                produces: &[],
                params_schema: "{}",
            },
            applicable,
        })
    }

    #[test]
    fn conditioning_requirements_count_distinct_valid_names() {
        use unclip_domain::{DomainId, FrameId};
        use unclip_epistemic::{DomainVersion, FrameVersion};
        let domain = DomainSnapshot {
            id: DomainId::new("test"),
            version: DomainVersion::new("1"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("test"),
            version: FrameVersion::new("1"),
            axes: vec![],
        };
        for (params, have) in [
            (serde_json::json!({}), 0),
            (serde_json::json!({"conditioning_variables": "genre"}), 0),
            (
                serde_json::json!({"conditioning_variables": [null, 1, "", " "]}),
                0,
            ),
            (
                serde_json::json!({"conditioning_variables": ["genre", "genre"]}),
                1,
            ),
            (
                serde_json::json!({"conditioning_variables": ["genre", "source"]}),
                2,
            ),
        ] {
            let ctx = MeasureCtx::new(
                &domain,
                &frame,
                &[],
                &[],
                &[],
                &params,
                DependencyCollector::default(),
            );
            for requirement in [
                EvidenceRequirement::ConditioningVariables,
                EvidenceRequirement::MinConditioningVariables(2),
            ] {
                let need = if requirement == EvidenceRequirement::ConditioningVariables {
                    1
                } else {
                    2
                };
                assert_eq!(
                    ctx.evidence_gap(requirement),
                    (have < need).then_some(EvidenceGap {
                        requirement,
                        have,
                        need
                    })
                );
            }
            let decision = classify_sensor(
                sensor_with(&[EvidenceRequirement::MinConditioningVariables(2)], true).as_ref(),
                &ctx,
                true,
            );
            assert_eq!(
                decision,
                if have < 2 {
                    SensorDecision::Record(Reading::InsufficientEvidence { have, need: 2 })
                } else {
                    SensorDecision::Run
                }
            );
            assert_eq!(
                ctx.evidence_gap(EvidenceRequirement::MinConditioningVariables(0)),
                None
            );
            assert_eq!(ctx.evidence_gap(EvidenceRequirement::MinSamples(0)), None);
        }
    }

    #[test]
    fn planner_preserves_sparse_reading_states() {
        use std::collections::BTreeMap;
        use unclip_domain::{DomainId, FrameId};
        use unclip_epistemic::{DerivedId, DomainVersion, FrameVersion, ParameterHash, Timestamp};

        let domain = DomainSnapshot {
            id: DomainId::new("test"),
            version: DomainVersion::new("1"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("test.general"),
            version: FrameVersion::new("1"),
            axes: Vec::new(),
        };
        let params = serde_json::json!({});
        let ctx = MeasureCtx::new(
            &domain,
            &frame,
            &[],
            &[],
            &[],
            &params,
            DependencyCollector::default(),
        );

        assert_eq!(
            classify_sensor(sensor().as_ref(), &ctx, false),
            SensorDecision::Record(Reading::NotMeasured)
        );
        assert_eq!(
            classify_sensor(sensor_with(&[], false).as_ref(), &ctx, true),
            SensorDecision::Record(Reading::NotApplicable {
                reason: "unsupported fixture".into()
            })
        );
        assert_eq!(
            classify_sensor(
                sensor_with(&[EvidenceRequirement::MinSamples(2)], true).as_ref(),
                &ctx,
                true
            ),
            SensorDecision::Record(Reading::InsufficientEvidence { have: 0, need: 2 })
        );
        assert_eq!(
            classify_sensor(sensor().as_ref(), &ctx, true),
            SensorDecision::Run
        );

        let derived = ctx
            .calculation_token(EmitMetadata {
                id: DerivedId::new("measurement-1"),
                producer: PluginId::new("sensor.stub"),
                algorithm: "stub".into(),
                version: Version::new(0, 1, 0),
                params: serde_json::json!({}),
                params_hash: ParameterHash::new("hash"),
                source: None,
                timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
                domain_version: None,
                frame_version: None,
                model: None,
            })
            .emit(());
        assert_eq!(
            derived.provenance().domain_version,
            Some(DomainVersion::new("1"))
        );
        assert_eq!(
            derived.provenance().frame_version,
            Some(FrameVersion::new("1"))
        );
    }

    #[test]
    fn conformance_accepts_a_deterministic_scalar_sensor() {
        use std::collections::BTreeMap;
        use unclip_domain::{DomainId, FrameId};
        use unclip_epistemic::{
            DerivedId, DomainVersion, EmitMetadata, FrameVersion, ParameterHash, Timestamp,
        };
        use unclip_measure::{MeasurementContext, MeasurementValue};

        struct ScalarSensor(SensorDescriptor);
        impl Sensor for ScalarSensor {
            fn descriptor(&self) -> &SensorDescriptor {
                &self.0
            }

            fn applies_to(&self, _ctx: &MeasureCtx<'_>) -> Applicability {
                Applicability::Applicable
            }

            fn measure(
                &self,
                _ctx: &MeasureCtx<'_>,
                token: CalculationToken,
            ) -> Result<Vec<Calculated<Measurement>>> {
                Ok(vec![token.emit(Measurement {
                    sensor: self.0.id.clone(),
                    sensor_version: self.0.version.clone(),
                    reading: Reading::Value {
                        value: MeasurementValue::Scalar(0.0),
                    },
                    confidence: Some(1.0),
                    sample_count: Some(1),
                    context: MeasurementContext::default(),
                })])
            }
        }

        let sensor = ScalarSensor(SensorDescriptor {
            id: PluginId::new("sensor.scalar"),
            version: Version::new(0, 1, 0),
            applicability: &[],
            evidence: &[],
            produces: &[MeasurementKind::Scalar],
            params_schema: "{}",
        });
        let domain = DomainSnapshot {
            id: DomainId::new("test"),
            version: DomainVersion::new("1"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("test.general"),
            version: FrameVersion::new("1"),
            axes: Vec::new(),
        };
        let params = serde_json::json!({});
        let ctx = MeasureCtx::new(
            &domain,
            &frame,
            &[],
            &[],
            &[],
            &params,
            DependencyCollector::default(),
        );
        conformance::assert_sensor(&sensor, |plugin| {
            plugin.measure(
                &ctx,
                ctx.calculation_token(EmitMetadata {
                    id: DerivedId::new("measurement"),
                    producer: PluginId::new("sensor.scalar"),
                    algorithm: "scalar".into(),
                    version: Version::new(0, 1, 0),
                    params: serde_json::json!({}),
                    params_hash: ParameterHash::new("hash"),
                    source: None,
                    timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
                    domain_version: None,
                    frame_version: None,
                    model: None,
                }),
            )
        });
    }

    #[test]
    fn conformance_accepts_a_deterministic_empty_sensor() {
        let sensor = sensor();
        conformance::assert_sensor(sensor.as_ref(), |_| Ok(Vec::new()));
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
    fn resolution_rejects_incompatible_versions() {
        let mut registry = Registry::default();
        registry.register_sensor(sensor()).unwrap();
        let profile = EngineProfile {
            sensors: vec![PluginSelection {
                id: PluginId::new("sensor.stub"),
                version: VersionReq::parse("^2").unwrap(),
            }],
            ..EngineProfile::default()
        };
        assert!(matches!(
            registry.resolve(&profile).err().unwrap(),
            PluginError::IncompatibleVersion { .. }
        ));
    }

    #[test]
    fn resolution_reports_missing_plugins() {
        let registry = Registry::default();
        let profile = EngineProfile {
            sensors: vec![PluginSelection::any("sensor.missing")],
            ..EngineProfile::default()
        };
        assert_eq!(
            registry.resolve(&profile).err().unwrap(),
            PluginError::MissingPlugin(PluginId::new("sensor.missing"))
        );
    }
}
