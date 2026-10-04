//! Plugin contracts, capability-aware contexts, and explicit runtime registry.
//!
//! # Writing a plugin
//!
//! Every plugin family follows the same three-part shape.
//!
//! 1. **A descriptor** states the plugin's identity, version, parameter schema,
//!    and — for sensors — its stage, the inputs it understands
//!    ([`Capability`]), the evidence it needs ([`EvidenceRequirement`]), and
//!    the measurement kinds it produces. The engine reads the descriptor to
//!    decide whether to call the plugin at all.
//! 2. **A context** (`MeasureCtx`, `InferCtx`, `CompareCtx`, `CandidateCtx`,
//!    `NullCtx`, `InterpretCtx`) hands over exactly the inputs that family may
//!    see. Contexts expose their inputs only through methods, and those methods
//!    record each read, so a plugin cannot consume an input without that input
//!    appearing in the result's provenance.
//! 3. **An emit token** is the only way to construct the plugin's return value.
//!    A `Calculated<T>` cannot be built by hand, and an `Inferred<T>` cannot be
//!    relabeled as `Calculated<T>`, so the operation recorded against a value
//!    is always the operation that actually produced it.
//!
//! Which token a family receives is the family's epistemic claim, and it is not
//! negotiable: sensors and comparators calculate, inferrers infer, interpreters
//! interpret, experimenters test. Pick the family that matches the claim you
//! can actually support.
//!
//! Register implementations on a [`Registry`], which rejects a plugin id that
//! is already taken by any family, then select them by id and version in an
//! `EngineProfile`. Recorded plans pin the resolved version and parameter hash,
//! which is what lets `unclip level verify` re-run a calculation stage against
//! its stored result.
//!
//! [`conformance`] provides reusable assertions for a sensor's side of this
//! contract; first-party and third-party sensors are held to the same checks.
//!
//! # Type-level boundaries
//!
//! Interpreter implementations must return an epistemically typed value. A raw
//! model response does not satisfy the contract.
//!
//! ```compile_fail
//! use async_trait::async_trait;
//! use unclip_epistemic::InterpretationToken;
//! use unclip_plugin::{InterpretCtx, Interpreter, PluginDescriptor, Result};
//!
//! struct RawInterpreter;
//!
//! #[async_trait]
//! impl Interpreter for RawInterpreter {
//!     fn descriptor(&self) -> &PluginDescriptor { todo!() }
//!
//!     async fn interpret(
//!         &self,
//!         _: &InterpretCtx<'_>,
//!         _: InterpretationToken,
//!     ) -> Result<serde_json::Value> {
//!         todo!()
//!     }
//! }
//! ```
//!
//! A calculated result also cannot be relabeled as interpreted.
//!
//! ```compile_fail
//! use unclip_epistemic::{Calculated, Interpreted};
//!
//! fn relabel(value: Calculated<serde_json::Value>) -> Interpreted<serde_json::Value> {
//!     value
//! }
//! ```

#![forbid(unsafe_code)]

mod candidate;
mod comparison;
mod null;
mod schema;
pub use candidate::CandidateCtx;
pub use comparison::CompareCtx;
pub use null::NullCtx;
pub use schema::{check_schema, validate_params, MalformedSchema, SchemaViolation};

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use semver::{Version, VersionReq};
use thiserror::Error;
use unclip_domain::{
    DomainSnapshot, MeasurementFrame, ProductDomainSnapshot, ProductMeasurementFrame,
};
use unclip_epistemic::{
    Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata, ExperimentToken,
    Experimental, FrameVersion, InferenceToken, Inferred, InterpretationToken, Interpreted,
    ModelRef, PluginId, SharedParams, SourceRef, Tracked,
};
use unclip_measure::{
    CrossDomainInteractionMovement, CrossDomainMutualInformation, CrossDomainSample, Delta,
    EmpiricalStructure, MeasureError, Measurement, MeasurementKind, Reading,
};
use unclip_observe::{Alignment, Observation, PartialRanking};

/// One plugin's parameters, as a context hands them out for reading.
///
/// A context holds them as [`SharedParams`] so a provenance record can keep the
/// configured tree without copying it; `params()` borrows through that handle
/// and `shared_params()` hands out another.
pub type Params = serde_json::Value;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PluginError {
    #[error("plugin id is already registered: {0}")]
    DuplicatePlugin(PluginId),
    /// A plugin id contains a separator that derived ids reserve.
    #[error("plugin id {plugin} must not contain {reserved:?}, which derived IDs reserve")]
    InvalidPluginId { plugin: PluginId, reserved: char },
    /// A plugin's declared `params_schema` is not a schema this crate can
    /// enforce, so registration refused it.
    ///
    /// Separate from [`Self::InvalidParams`]: that one means a caller supplied
    /// parameters the schema rejects, this one means the schema itself cannot
    /// decide the question. The fixes go to different people.
    #[error("plugin {plugin} declares an unusable params schema: {reason}")]
    MalformedSchema { plugin: PluginId, reason: String },
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
    /// A measurement calculation failed, with the calculation's own error
    /// retained.
    ///
    /// Sensors drive `unclip-measure` calculations that report precisely what
    /// went wrong — which solver failed to converge, which sequence index broke
    /// ordering. Converting those to `Message` at the plugin boundary would
    /// leave a caller unable to tell "input was too sparse" from "the solver
    /// diverged", so they cross the boundary intact.
    #[error(transparent)]
    Measure(#[from] MeasureError),
    /// A plugin's parameters could not be deserialized against its schema.
    ///
    /// `serde_json::Error` is neither `Clone` nor `PartialEq`, so the rendered
    /// message is retained rather than the error itself; the separate variant
    /// still lets a caller distinguish bad configuration from a failed
    /// calculation.
    #[error("invalid plugin parameters: {0}")]
    InvalidParams(String),
    /// A plugin returned a value whose provenance names a different producer.
    ///
    /// Emit tokens can be minted outside the engine, so the engine checks every
    /// returned value against the plugin it invoked. A mismatch means the
    /// plugin forged or forwarded another plugin's provenance.
    #[error("plugin {plugin} returned {value} produced by {producer}")]
    ForeignProducer {
        plugin: PluginId,
        value: DerivedId,
        producer: PluginId,
    },
    /// A failure with no more specific variant, carrying its own message.
    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, PluginError>;

/// An input shape a sensor is able to work with.
///
/// Declared in `SensorDescriptor::applicability`. This says what a sensor
/// *understands*, independent of whether enough of it is present — that is
/// [`EvidenceRequirement`]'s job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Reads observations directly.
    Observation,
    /// Reads observed-to-domain alignments.
    Alignment,
    /// Reads ranked values.
    RankingValue,
    /// Reads graph-shaped values.
    GraphValue,
    /// Reads several observations together rather than one at a time.
    MultiObservation,
    /// Requires inputs in a caller-established order.
    Ordered,
    /// Operates on a product of two domains rather than a single domain.
    ProductDomain,
}

/// A precondition the engine checks before invoking a sensor.
///
/// Declared in `SensorDescriptor::evidence`. When a requirement is unmet the
/// engine records a sparse [`Reading`] instead of calling the sensor, so a
/// sensor is never asked to produce a number it has no basis for. These are
/// necessary conditions, not sufficient ones: a sensor still validates the
/// specific data it receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceRequirement {
    /// Rankings must be total — no ties and no unknown tail.
    TotalOrder,
    /// Minimum input observations; sensors must also check complete usable samples.
    MinSamples(usize),
    /// Inputs must carry a caller-established order.
    Ordered,
    /// An explicitly configured observation sequence; the sensor validates its contents.
    ExplicitOrder,
    /// At least one distinct, nonempty conditioning-variable name in parameters.
    ConditioningVariables,
    /// Minimum distinct configured conditioning variables. Sensors validate their data.
    MinConditioningVariables(usize),
}

/// Whether a sensor can run against the inputs it was offered.
///
/// `NotApplicable` carries a reason because it is recorded, not discarded: a
/// run states why a selected sensor produced nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applicability {
    Applicable,
    /// Shared so the reason can become the recorded [`Reading::NotApplicable`]
    /// without being copied; every caller already builds one from a literal.
    NotApplicable {
        reason: Arc<str>,
    },
}

/// Where a sensor runs in the calculation pipeline.
///
/// Stages execute in `Ord` order and `Engine::execute` collects each stage's
/// output into its own field of `PipelineResults`. The order expresses how the
/// results are *read* — an explanation sensor reports what the domain already
/// accounts for, a residual sensor what it does not, and measurement sensors
/// calculate over established evidence.
///
/// It is not a data dependency. `MeasureCtx` carries observations, alignments
/// and rankings; it does not carry earlier stages' measurements, so a residual
/// sensor cannot read an explanation sensor's output and derives its own from
/// the same evidence. Running later buys a residual sensor nothing today, and a
/// sensor written as if it could consume an earlier stage's result will not
/// compile rather than silently see nothing. Introducing that channel means
/// adding the prior measurements to `MeasureCtx` and stating how they are
/// recorded in provenance; until then, treat the stage as a label on the
/// output, not a position in a chain.
///
/// Stage is declared here, on the descriptor, rather than inferred from the
/// plugin id. Inferring it would silently reclassify a renamed sensor and
/// would make the explanation and residual stages unreachable for any sensor
/// outside this workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum SensorStage {
    /// Establishes what the domain accounts for. Runs first.
    Explanation,
    /// Reports what the explanation stage left unaccounted for.
    Residual,
    /// Calculates over established evidence. The default.
    #[default]
    Measurement,
}

#[derive(Debug, Clone)]
pub struct SensorDescriptor {
    pub id: PluginId,
    pub version: Version,
    /// Pipeline stage. See [`SensorStage`]; most sensors are
    /// [`SensorStage::Measurement`].
    pub stage: SensorStage,
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

/// The three things the registry needs from a plugin, whatever family it is in.
///
/// Every family declares its own descriptor type — a sensor's carries a stage
/// and an applicability list, a comparator's the kinds it supports — but
/// registration, id resolution and run records read only the identity, the
/// version and the declared schema. Naming that subset lets those three share
/// one implementation instead of one per family: `Registry::register` and
/// `resolve_ids` were generic over `T: ?Sized` and took a closure at every call
/// site to reach `descriptor().version`, which is this trait written out
/// longhand, eight times, with no name.
pub trait Described {
    fn plugin_id(&self) -> &PluginId;
    fn plugin_version(&self) -> &Version;
    fn plugin_schema(&self) -> &'static str;
}

macro_rules! described_by_fields {
    ($($descriptor:ty),+ $(,)?) => {$(
        impl Described for $descriptor {
            fn plugin_id(&self) -> &PluginId {
                &self.id
            }
            fn plugin_version(&self) -> &Version {
                &self.version
            }
            fn plugin_schema(&self) -> &'static str {
                self.params_schema
            }
        }
    )+};
}

described_by_fields!(
    SensorDescriptor,
    InferrerDescriptor,
    ComparatorDescriptor,
    PluginDescriptor,
);

macro_rules! described_by_descriptor {
    ($($family:ident),+ $(,)?) => {$(
        impl Described for dyn $family {
            fn plugin_id(&self) -> &PluginId {
                self.descriptor().plugin_id()
            }
            fn plugin_version(&self) -> &Version {
                self.descriptor().plugin_version()
            }
            fn plugin_schema(&self) -> &'static str {
                self.descriptor().plugin_schema()
            }
        }
    )+};
}

described_by_descriptor!(
    Sensor,
    ProductSensor,
    CrossProductSensor,
    Inferrer,
    Comparator,
    Interpreter,
    CandidateGenerator,
    NullModel,
);

pub struct MeasureCtx<'a> {
    domain: &'a DomainSnapshot,
    frame: &'a MeasurementFrame,
    observations: &'a [Tracked<Observation>],
    alignments: &'a [Tracked<Alignment>],
    rankings: &'a [Tracked<PartialRanking>],
    params: &'a SharedParams,
    dependencies: DependencyCollector,
}

impl<'a> MeasureCtx<'a> {
    pub fn new(
        domain: &'a DomainSnapshot,
        frame: &'a MeasurementFrame,
        observations: &'a [Tracked<Observation>],
        alignments: &'a [Tracked<Alignment>],
        rankings: &'a [Tracked<PartialRanking>],
        params: &'a SharedParams,
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

    /// The same parameters as a shared handle, for an [`EmitMetadata`] that
    /// keeps them: a provenance record aliases the run's configured tree
    /// instead of copying it.
    pub fn shared_params(&self) -> SharedParams {
        SharedParams::clone(self.params)
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

    /// Whether one declared requirement is unmet, and by how much.
    ///
    /// Inputs are inspected through a scratch collector that is discarded, so
    /// asking the question does not answer it into provenance. A requirement
    /// check is about the *shape* of the offered evidence; the values a
    /// measurement actually derives from are the ones the sensor reads through
    /// [`Self::read`]. Sharing the context's collector here made every
    /// `Ordered` sensor claim every observation as an input, including the ones
    /// it went on to ignore.
    pub fn evidence_gap(&self, requirement: EvidenceRequirement) -> Option<EvidenceGap> {
        let scratch = DependencyCollector::default();
        match requirement {
            EvidenceRequirement::TotalOrder => {
                let have = usize::from(
                    self.rankings
                        .iter()
                        .any(|ranking| scratch.read(ranking).is_total()),
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
                    .filter(|observation| scratch.read(observation).observed_at.is_some())
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

/// Decide whether a scheduled sensor can run against the offered evidence.
///
/// Every caller reaches this with a sensor a `RunPlan` already selected, so the
/// question is only whether the evidence supports it. This took a `scheduled`
/// flag that returned [`Reading::NotMeasured`] when false; no caller ever
/// passed false, because `RunPlan::sensors` holds selected sensors and nothing
/// else. The flag was reachable only from the tests that exercised it, which is
/// the shape of a guard that proves nothing.
///
/// `Reading::NotMeasured` remains a state a measurement can be *in* — an
/// imported or hand-authored profile may record it — it is simply not a
/// decision this function makes.
pub fn classify_sensor(sensor: &dyn Sensor, ctx: &MeasureCtx<'_>) -> SensorDecision {
    // The descriptor's schema is checked before the sensor sees its parameters,
    // so it constrains third-party sensors on the same terms as first-party ones
    // rather than relying on each to re-validate its own contract. A violation is
    // recorded like any other reason a selected sensor produced nothing.
    if let Err(violation) = validate_params(sensor.descriptor().params_schema, ctx.params()) {
        return SensorDecision::Record(Reading::NotApplicable {
            reason: format!("parameters do not satisfy the declared schema: {violation}").into(),
        });
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

/// Capability-scoped inputs for one inference stage invocation.
///
/// Fields are private and reached through accessors, matching every other
/// context in this module. They were public, which let a plugin destructure the
/// context and take `io` out of it — the one handle whose use is supposed to be
/// confined to this stage — and meant this context alone could not later record
/// or restrict a read without breaking its callers.
pub struct InferCtx<'a> {
    source: SourceRef,
    domain: &'a DomainSnapshot,
    params: &'a SharedParams,
    io: &'a dyn InferenceIo,
}

impl<'a> InferCtx<'a> {
    pub fn new(
        source: SourceRef,
        domain: &'a DomainSnapshot,
        params: &'a SharedParams,
        io: &'a dyn InferenceIo,
    ) -> Self {
        Self {
            source,
            domain,
            params,
            io,
        }
    }

    /// The external source this inference is drawing from.
    pub fn source(&self) -> &SourceRef {
        &self.source
    }

    pub fn domain(&self) -> &DomainSnapshot {
        self.domain
    }

    pub fn params(&self) -> &Params {
        self.params
    }

    /// The same parameters as a shared handle, for an [`EmitMetadata`] that
    /// keeps them: a provenance record aliases the run's configured tree
    /// instead of copying it.
    pub fn shared_params(&self) -> SharedParams {
        SharedParams::clone(self.params)
    }

    /// The model boundary for this stage. Requests made any other way are not
    /// recorded against the run.
    pub fn io(&self) -> &dyn InferenceIo {
        self.io
    }
}

/// What one inference stage produced.
///
/// The products are held as shared handles rather than owned inline. The
/// engine keeps the emitted output for provenance *and* tracks every product
/// inside it as a separate engine input, so owning them here meant copying
/// each observation, alignment, and ranking once per run — the workspace's
/// most repeated copy, scaling with the number of observations. Building one
/// is [`Arc::new`] per product; nothing downstream copies them again.
#[derive(Debug, Clone, PartialEq)]
pub enum InferenceOutput {
    Bundle {
        observations: Vec<Arc<Observation>>,
        alignments: Vec<Arc<Alignment>>,
        rankings: Vec<Arc<PartialRanking>>,
    },
    Observations(Vec<Arc<Observation>>),
    Alignments(Vec<Arc<Alignment>>),
    Rankings(Vec<Arc<PartialRanking>>),
    Structured(serde_json::Value),
}

/// The boundary to an external model for the inference stage.
///
/// This is the one place a model provider is contacted during inference. It is
/// a separate trait from [`Inferrer`] so the nondeterministic call is isolated
/// from the logic that interprets its response, which keeps that logic
/// testable against recorded responses.
#[async_trait]
pub trait InferenceIo: Send + Sync {
    /// Request raw output for one source. The response is untrusted input and
    /// must be validated by the calling [`Inferrer`].
    async fn request(&self, source: &SourceRef, params: &Params) -> Result<serde_json::Value>;
}

/// A deterministic calculation over established evidence.
///
/// Sensors are the workhorse plugin. The engine calls [`applies_to`] first and
/// records a sparse [`Reading`] rather than invoking a sensor whose evidence
/// requirements are unmet, so a sensor never has to invent a result for input
/// it cannot handle.
///
/// # Implementor contract
///
/// - **Be deterministic.** The same inputs must produce the same output, byte
///   for byte. The built-in sensor crate has no clock, RNG, or network
///   dependency, and CI enforces that; a third-party sensor is expected to hold
///   the same line.
/// - **Read inputs through the context.** `MeasureCtx::read` records each
///   input's identity in the emitted provenance. A value obtained another way
///   is missing from provenance and breaks verification.
/// - **Count through the context too.** `MeasureCtx::observations()` and its
///   siblings hand out the slice, and `.len()` on one records nothing — the
///   collector sees a value only when it is read. That is deliberate for an
///   *eligibility* question, which is about the shape of the offered evidence
///   rather than its contents (see [`MeasureCtx::evidence_gap`]), but a
///   measurement derived from a cardinality genuinely depends on the items
///   counted. Read them, or emit the count in
///   [`Measurement::sample_count`] rather than as the reading, so the value a
///   sensor claims and the inputs its provenance names cannot disagree.
/// - **Emit only through the token.** [`CalculationToken`] is the sole
///   constructor for `Calculated<T>`, which is what makes the result's
///   operation label trustworthy.
/// - **Declare what you produce.** Every emitted measurement must carry the
///   descriptor's own id and version, and its value kind must appear in
///   `SensorDescriptor::produces`. [`conformance::assert_sensor`] checks all
///   three.
/// - **Distinguish absence from zero.** Report sparse evidence as
///   [`Reading::InsufficientEvidence`], [`Reading::NotApplicable`], or
///   [`Reading::NotMeasured`] — never as a measured value.
///
/// [`applies_to`]: Sensor::applies_to
pub trait Sensor: Send + Sync {
    fn descriptor(&self) -> &SensorDescriptor;

    /// Whether this sensor can run against the given inputs at all.
    ///
    /// This is a capability question, not an evidence-volume question: the
    /// engine checks `SensorDescriptor::evidence` separately.
    fn applies_to(&self, ctx: &MeasureCtx<'_>) -> Applicability;

    /// Calculate measurements. One call may emit several; the token assigns
    /// each a distinct derived id.
    fn measure(
        &self,
        ctx: &MeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<Measurement>>>;
}

/// Capability-scoped inputs for one product-domain sensor invocation.
pub struct ProductMeasureCtx<'a> {
    product: &'a Tracked<ProductDomainSnapshot>,
    frame: &'a Tracked<ProductMeasurementFrame>,
    samples: &'a [Tracked<CrossDomainSample>],
    mutual_information: Option<&'a Tracked<CrossDomainMutualInformation>>,
    params: &'a SharedParams,
    dependencies: DependencyCollector,
}

impl<'a> ProductMeasureCtx<'a> {
    pub fn new(
        product: &'a Tracked<ProductDomainSnapshot>,
        frame: &'a Tracked<ProductMeasurementFrame>,
        samples: &'a [Tracked<CrossDomainSample>],
        params: &'a SharedParams,
        dependencies: DependencyCollector,
    ) -> Self {
        Self {
            product,
            frame,
            samples,
            mutual_information: None,
            params,
            dependencies,
        }
    }

    pub fn with_mutual_information(
        product: &'a Tracked<ProductDomainSnapshot>,
        frame: &'a Tracked<ProductMeasurementFrame>,
        mutual_information: &'a Tracked<CrossDomainMutualInformation>,
        params: &'a SharedParams,
        dependencies: DependencyCollector,
    ) -> Self {
        Self {
            product,
            frame,
            samples: &[],
            mutual_information: Some(mutual_information),
            params,
            dependencies,
        }
    }

    pub fn product(&self) -> &ProductDomainSnapshot {
        self.dependencies.read(self.product)
    }

    pub fn frame(&self) -> &ProductMeasurementFrame {
        self.dependencies.read(self.frame)
    }

    pub fn samples(&self) -> &[Tracked<CrossDomainSample>] {
        self.samples
    }

    pub fn read<'b>(&self, sample: &'b Tracked<CrossDomainSample>) -> &'b CrossDomainSample {
        self.dependencies.read(sample)
    }

    pub fn mutual_information(&self) -> Option<&CrossDomainMutualInformation> {
        self.mutual_information
            .map(|profile| self.dependencies.read(profile))
    }

    pub fn params(&self) -> &Params {
        self.params
    }

    /// The same parameters as a shared handle, for an [`EmitMetadata`] that
    /// keeps them: a provenance record aliases the run's configured tree
    /// instead of copying it.
    pub fn shared_params(&self) -> SharedParams {
        SharedParams::clone(self.params)
    }

    pub fn calculation_token(&self, metadata: EmitMetadata) -> CalculationToken {
        CalculationToken::from_harness(metadata, self.dependencies.clone())
    }
}

/// A calculation sensor whose input retains two domain identities.
pub trait ProductSensor: Send + Sync {
    fn descriptor(&self) -> &SensorDescriptor;
    fn measure(
        &self,
        ctx: &ProductMeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Calculated<Measurement>>;
}

/// Inputs for a sensor comparing explicitly mapped interactions across two products.
pub struct CrossProductMeasureCtx<'a> {
    source_product: &'a Tracked<ProductDomainSnapshot>,
    source_frame: &'a Tracked<ProductMeasurementFrame>,
    source_movement: &'a Tracked<CrossDomainInteractionMovement>,
    target_product: &'a Tracked<ProductDomainSnapshot>,
    target_frame: &'a Tracked<ProductMeasurementFrame>,
    target_movement: &'a Tracked<CrossDomainInteractionMovement>,
    params: &'a SharedParams,
    dependencies: DependencyCollector,
}

/// One side of a cross-product comparison: a product domain, its measurement
/// frame, and the interaction movement recorded over it.
///
/// The three fields are what a side *is*; naming the side at construction is
/// what keeps a transfer's direction straight. `CrossProductMeasureCtx::new`
/// used to take these six references positionally, where each source parameter
/// had the same type as its target counterpart, so transposing a pair compiled
/// and produced a context that read as the opposite direction throughout.
#[derive(Clone, Copy)]
pub struct CrossProductSide<'a> {
    pub product: &'a Tracked<ProductDomainSnapshot>,
    pub frame: &'a Tracked<ProductMeasurementFrame>,
    pub movement: &'a Tracked<CrossDomainInteractionMovement>,
}

impl<'a> CrossProductMeasureCtx<'a> {
    pub fn new(
        source: CrossProductSide<'a>,
        target: CrossProductSide<'a>,
        params: &'a SharedParams,
        dependencies: DependencyCollector,
    ) -> Self {
        Self {
            source_product: source.product,
            source_frame: source.frame,
            source_movement: source.movement,
            target_product: target.product,
            target_frame: target.frame,
            target_movement: target.movement,
            params,
            dependencies,
        }
    }

    pub fn source_product(&self) -> &ProductDomainSnapshot {
        self.dependencies.read(self.source_product)
    }

    pub fn source_frame(&self) -> &ProductMeasurementFrame {
        self.dependencies.read(self.source_frame)
    }

    pub fn source_movement(&self) -> &CrossDomainInteractionMovement {
        self.dependencies.read(self.source_movement)
    }

    pub fn target_product(&self) -> &ProductDomainSnapshot {
        self.dependencies.read(self.target_product)
    }

    pub fn target_frame(&self) -> &ProductMeasurementFrame {
        self.dependencies.read(self.target_frame)
    }

    pub fn target_movement(&self) -> &CrossDomainInteractionMovement {
        self.dependencies.read(self.target_movement)
    }

    pub fn params(&self) -> &Params {
        self.params
    }

    /// The same parameters as a shared handle, for an [`EmitMetadata`] that
    /// keeps them: a provenance record aliases the run's configured tree
    /// instead of copying it.
    pub fn shared_params(&self) -> SharedParams {
        SharedParams::clone(self.params)
    }

    pub fn calculation_token(&self, metadata: EmitMetadata) -> CalculationToken {
        CalculationToken::from_harness(metadata, self.dependencies.clone())
    }
}

/// A calculation sensor whose inputs retain two distinct product identities.
pub trait CrossProductSensor: Send + Sync {
    fn descriptor(&self) -> &SensorDescriptor;
    fn measure(
        &self,
        ctx: &CrossProductMeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Calculated<Measurement>>;
}

/// Turns a source into observations, alignments, or rankings using a model.
///
/// This is the only stage whose output is labeled [`Inferred`]. That label is
/// permanent and deliberately cannot be converted to `Calculated`: everything
/// downstream can therefore tell model-derived evidence from computed
/// evidence, and `unclip level verify` knows not to claim it can reproduce
/// this stage.
///
/// # Implementor contract
///
/// - Reach the model through [`InferCtx`]'s [`InferenceIo`], not directly, so
///   the request is recorded.
/// - Validate the response before emitting. A model response is untrusted
///   input.
/// - Emit through the supplied [`InferenceToken`]; it stamps the source and
///   model identity into provenance.
#[async_trait]
pub trait Inferrer: Send + Sync {
    fn descriptor(&self) -> &InferrerDescriptor;
    async fn infer(
        &self,
        ctx: &InferCtx<'_>,
        token: InferenceToken,
    ) -> Result<Inferred<InferenceOutput>>;
}

/// Computes the difference between two measurements of the same kind.
///
/// A comparator declares the measurement kinds it understands in
/// `ComparatorDescriptor::supports`; the engine rejects a profile that points
/// one at a kind it does not, rather than letting it produce a meaningless
/// delta.
///
/// # Implementor contract
///
/// Comparison is a calculation, so the same determinism and provenance rules
/// as [`Sensor`] apply. A pair that cannot be meaningfully compared — a sparse
/// reading on either side, say — is reported as such rather than as a zero
/// delta.
pub trait Comparator: Send + Sync {
    fn descriptor(&self) -> &ComparatorDescriptor;
    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct InterpretationRequest {
    pub model: String,
    pub model_version: String,
    pub instructions: String,
    /// Shared with the tracked input it was read from, not copied out of it.
    pub structure: Arc<EmpiricalStructure>,
    pub parameters: Params,
    pub response_schema: Params,
}

/// The boundary to an external model for the interpretation stage.
///
/// Kept separate from [`Interpreter`] for the same reason [`InferenceIo`] is
/// separate from [`Inferrer`]: the nondeterministic call is isolated so the
/// surrounding logic can be tested against a recorded response. The CLI treats
/// that recorded response file as the reproducible boundary to the provider.
///
/// The response is returned shared so a recorded response can be handed out
/// without copying it; a live provider wraps the tree it parsed.
#[async_trait]
pub trait InterpretationIo: Send + Sync {
    async fn request(&self, request: &InterpretationRequest) -> Result<Arc<serde_json::Value>>;
}

/// Capability-scoped access to one tracked empirical structure and model I/O.
pub struct InterpretCtx<'a> {
    structure: &'a Tracked<EmpiricalStructure>,
    params: &'a SharedParams,
    io: &'a dyn InterpretationIo,
    dependencies: DependencyCollector,
}

impl<'a> InterpretCtx<'a> {
    pub fn new(
        structure: &'a Tracked<EmpiricalStructure>,
        params: &'a SharedParams,
        io: &'a dyn InterpretationIo,
        dependencies: DependencyCollector,
    ) -> Self {
        // The source structure is mandatory input even if a cooperative plugin
        // neglects to call `structure` before emitting.
        dependencies.read(structure);
        Self {
            structure,
            params,
            io,
            dependencies,
        }
    }

    pub fn structure(&self) -> &EmpiricalStructure {
        self.dependencies.read(self.structure)
    }

    /// The same structure as a shared handle, for a caller that must keep it
    /// past this borrow without copying its JSON tree.
    pub fn structure_shared(&self) -> Arc<EmpiricalStructure> {
        self.dependencies.read_shared(self.structure)
    }

    pub fn params(&self) -> &Params {
        self.params
    }

    /// The same parameters as a shared handle, for an [`EmitMetadata`] that
    /// keeps them: a provenance record aliases the run's configured tree
    /// instead of copying it.
    pub fn shared_params(&self) -> SharedParams {
        SharedParams::clone(self.params)
    }

    pub fn io(&self) -> &dyn InterpretationIo {
        self.io
    }

    pub fn interpretation_token(&self, metadata: EmitMetadata) -> InterpretationToken {
        InterpretationToken::from_harness(metadata, self.dependencies.clone())
    }
}

/// Assigns semantic meaning to an empirical structure.
///
/// The operation-specific token is the only output constructor supplied by the
/// harness, and the return type retains the `Interpreted<T>` marker.
#[async_trait]
pub trait Interpreter: Send + Sync {
    fn descriptor(&self) -> &PluginDescriptor;

    /// Resolve the exact model identity stored in output provenance.
    fn model_ref(&self, _params: &Params) -> Result<Option<ModelRef>> {
        Ok(None)
    }

    async fn interpret(
        &self,
        ctx: &InterpretCtx<'_>,
        token: InterpretationToken,
    ) -> Result<Interpreted<serde_json::Value>>;
}

/// Tests a candidate revision against held-out evidence.
///
/// Output is labeled [`Experimental`], which is what the apply stage requires:
/// a candidate can only be promoted through a completed experiment that
/// actually tested it. The type prevents a candidate from being promoted on
/// the strength of the measurements that suggested it in the first place.
pub trait Experimenter: Send + Sync {
    fn descriptor(&self) -> &PluginDescriptor;
    fn experiment(
        &self,
        candidate: &serde_json::Value,
        token: ExperimentToken,
    ) -> Result<Experimental<serde_json::Value>>;
}

/// Proposes candidate revisions from residual or measured evidence.
///
/// # Implementor contract
///
/// - **Propose, never apply.** A generator does not modify the domain; it
///   emits proposals for the experiment and apply stages to judge.
/// - **Stay anonymous.** Proposals carry structure and evidence, not semantic
///   labels. Naming is the interpretation stage's job, and its output cannot
///   be consumed as measurement evidence.
/// - **Retain evidence.** Each proposal keeps the observations and
///   measurements that support it, so a reviewer can check the claim.
/// - **Stay silent on sparse input.** Too little evidence means no proposal,
///   not a weak one.
pub trait CandidateGenerator: Send + Sync {
    fn descriptor(&self) -> &PluginDescriptor;
    fn generate(
        &self,
        ctx: &CandidateCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<unclip_domain::CandidateProposal>>>;
}

/// Evaluates a candidate against an explicit alternative explanation.
///
/// A null model answers "what would this evidence look like if the candidate
/// were not true?" for one specific alternative. It is a diagnostic, not a
/// verdict.
///
/// # Implementor contract
///
/// - **Never accept or reject.** Return the evidence; the decision belongs to
///   the operator.
/// - **Return [`Reading::NotApplicable`]** for a candidate shape this null
///   cannot speak to, rather than a misleading number.
/// - **State assumptions** in the emitted value. A null that assumes
///   exchangeable observations, or that tests co-presence rather than
///   direction, says so in its result.
pub trait NullModel: Send + Sync {
    fn descriptor(&self) -> &PluginDescriptor;
    fn evaluate(&self, ctx: &NullCtx<'_>, token: CalculationToken) -> Result<Calculated<Reading>>;
}

/// Reusable checks for first-party and cooperative third-party sensors.
pub mod conformance {
    use super::{
        classify_sensor, Calculated, CrossProductSensor, MeasureCtx, Measurement, MeasurementKind,
        ProductSensor, Result, Sensor, SensorDecision, SensorDescriptor,
    };

    fn assert_measurement(descriptor: &SensorDescriptor, derived: &Calculated<Measurement>) {
        if let super::Reading::Value { value } = &derived.value().reading {
            let kind: MeasurementKind = value.kind();
            assert!(
                descriptor.produces.contains(&kind),
                "sensor emitted undeclared measurement kind {kind:?}"
            );
        }
        assert_eq!(
            derived.value().sensor,
            descriptor.id,
            "measurement carries the wrong sensor id"
        );
        assert_eq!(
            derived.value().sensor_version,
            descriptor.version,
            "measurement carries the wrong sensor version"
        );
    }

    /// Assert the decision a sensor's descriptor and evidence produce.
    pub fn assert_planning(sensor: &dyn Sensor, ctx: &MeasureCtx<'_>, expected: SensorDecision) {
        assert_eq!(classify_sensor(sensor, ctx), expected);
    }

    pub fn assert_sensor<F>(sensor: &dyn Sensor, mut run: F)
    where
        F: FnMut(&dyn Sensor) -> Result<Vec<Calculated<Measurement>>>,
    {
        let first = run(sensor).expect("sensor fixture failed on first run");
        let second = run(sensor).expect("sensor fixture failed on repeated run");
        assert_eq!(first, second, "sensor output is not deterministic");

        for derived in &first {
            assert_measurement(sensor.descriptor(), derived);
        }
    }

    /// Run a product-domain sensor twice and assert the common deterministic
    /// output and descriptor contracts.
    pub fn assert_product_sensor<F>(sensor: &dyn ProductSensor, mut run: F)
    where
        F: FnMut() -> Result<Calculated<Measurement>>,
    {
        let first = run().expect("product sensor fixture failed on first run");
        let second = run().expect("product sensor fixture failed on repeated run");
        assert_eq!(first, second, "product sensor output is not deterministic");
        assert_measurement(sensor.descriptor(), &first);
    }

    /// Run a cross-product sensor twice and assert the common deterministic
    /// output and descriptor contracts.
    pub fn assert_cross_product_sensor<F>(sensor: &dyn CrossProductSensor, mut run: F)
    where
        F: FnMut() -> Result<Calculated<Measurement>>,
    {
        let first = run().expect("cross-product sensor fixture failed on first run");
        let second = run().expect("cross-product sensor fixture failed on repeated run");
        assert_eq!(
            first, second,
            "cross-product sensor output is not deterministic"
        );
        assert_measurement(sensor.descriptor(), &first);
    }
}

#[derive(Default)]
pub struct Registry {
    sensors: BTreeMap<PluginId, Arc<dyn Sensor>>,
    product_sensors: BTreeMap<PluginId, Arc<dyn ProductSensor>>,
    cross_product_sensors: BTreeMap<PluginId, Arc<dyn CrossProductSensor>>,
    inferrers: BTreeMap<PluginId, Arc<dyn Inferrer>>,
    comparators: BTreeMap<PluginId, Arc<dyn Comparator>>,
    interpreters: BTreeMap<PluginId, Arc<dyn Interpreter>>,
    generators: BTreeMap<PluginId, Arc<dyn CandidateGenerator>>,
    null_models: BTreeMap<PluginId, Arc<dyn NullModel>>,
}

impl Registry {
    /// Is this id already taken by any plugin family?
    ///
    /// A `PluginId` names one plugin for the whole registry, not one per
    /// family. `EngineProfile` selects plugins by bare id, and `resolve`
    /// rejects a profile that names the same id twice, so a registry that
    /// allowed one id to mean two different plugins could only ever expose
    /// one of them. Registration rejects the collision instead.
    fn contains(&self, id: &PluginId) -> bool {
        self.sensors.contains_key(id)
            || self.product_sensors.contains_key(id)
            || self.cross_product_sensors.contains_key(id)
            || self.inferrers.contains_key(id)
            || self.comparators.contains_key(id)
            || self.interpreters.contains_key(id)
            || self.generators.contains_key(id)
            || self.null_models.contains_key(id)
    }

    /// Claim an id across every family, check the declared schema, then insert
    /// into the family's own map.
    ///
    /// The schema is checked here rather than on first use because an unusable
    /// schema is a defect in the plugin, not in the run that happened to reach
    /// it. `validate_params` reports a bad schema as a violation, which
    /// `classify_sensor` records as a sparse `NotApplicable` reading — so a
    /// third-party plugin with a malformed schema used to produce a run of
    /// plausible-looking "not applicable" results instead of refusing to load.
    /// A registry-wide test held the first-party plugins to this; registration
    /// holds everyone to it.
    fn register<T: ?Sized + Described>(
        &mut self,
        plugin: Arc<T>,
        family: impl FnOnce(&mut Self) -> &mut BTreeMap<PluginId, Arc<T>>,
    ) -> Result<()> {
        let id = plugin.plugin_id().clone();
        if self.contains(&id) {
            return Err(PluginError::DuplicatePlugin(id));
        }
        // Derived ids are `{run_id}/{plugin_id}`, with `#n` for a token's later
        // emissions. A plugin id holding either separator would make those ids
        // ambiguous, so one run could mint another's id.
        if let Some(reserved) = id.chars().find(|c| matches!(c, '/' | '#')) {
            return Err(PluginError::InvalidPluginId {
                plugin: id,
                reserved,
            });
        }
        if let Err(error) = check_schema(plugin.plugin_schema()) {
            return Err(PluginError::MalformedSchema {
                plugin: id,
                reason: error.to_string(),
            });
        }
        family(self).insert(id, plugin);
        Ok(())
    }

    pub fn register_sensor(&mut self, plugin: Arc<dyn Sensor>) -> Result<()> {
        self.register(plugin, |r| &mut r.sensors)
    }

    pub fn register_product_sensor(&mut self, plugin: Arc<dyn ProductSensor>) -> Result<()> {
        self.register(plugin, |r| &mut r.product_sensors)
    }

    pub fn register_cross_product_sensor(
        &mut self,
        plugin: Arc<dyn CrossProductSensor>,
    ) -> Result<()> {
        self.register(plugin, |r| &mut r.cross_product_sensors)
    }

    pub fn register_inferrer(&mut self, plugin: Arc<dyn Inferrer>) -> Result<()> {
        self.register(plugin, |r| &mut r.inferrers)
    }

    pub fn register_comparator(&mut self, plugin: Arc<dyn Comparator>) -> Result<()> {
        self.register(plugin, |r| &mut r.comparators)
    }

    pub fn register_interpreter(&mut self, plugin: Arc<dyn Interpreter>) -> Result<()> {
        self.register(plugin, |r| &mut r.interpreters)
    }

    pub fn register_generator(&mut self, plugin: Arc<dyn CandidateGenerator>) -> Result<()> {
        self.register(plugin, |r| &mut r.generators)
    }

    pub fn register_null_model(&mut self, plugin: Arc<dyn NullModel>) -> Result<()> {
        self.register(plugin, |r| &mut r.null_models)
    }

    pub fn sensors(&self) -> impl Iterator<Item = &Arc<dyn Sensor>> {
        self.sensors.values()
    }

    pub fn product_sensors(&self) -> impl Iterator<Item = &Arc<dyn ProductSensor>> {
        self.product_sensors.values()
    }

    pub fn product_sensor(&self, id: &PluginId) -> Option<&Arc<dyn ProductSensor>> {
        self.product_sensors.get(id)
    }

    pub fn cross_product_sensors(&self) -> impl Iterator<Item = &Arc<dyn CrossProductSensor>> {
        self.cross_product_sensors.values()
    }

    pub fn cross_product_sensor(&self, id: &PluginId) -> Option<&Arc<dyn CrossProductSensor>> {
        self.cross_product_sensors.get(id)
    }

    pub fn inferrers(&self) -> impl Iterator<Item = &Arc<dyn Inferrer>> {
        self.inferrers.values()
    }

    pub fn comparators(&self) -> impl Iterator<Item = &Arc<dyn Comparator>> {
        self.comparators.values()
    }

    pub fn interpreters(&self) -> impl Iterator<Item = &Arc<dyn Interpreter>> {
        self.interpreters.values()
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
            .chain(&profile.interpreters)
            .chain(&profile.candidate_generators)
            .chain(&profile.null_models)
        {
            if !ids.insert(&selection.id) {
                return Err(PluginError::DuplicatePlugin(selection.id.clone()));
            }
        }
        Ok(RunPlan {
            sensors: resolve_ids(&self.sensors, &profile.sensors)?,
            inferrers: resolve_ids(&self.inferrers, &profile.inferrers)?,
            comparators: resolve_ids(&self.comparators, &profile.comparators)?,
            interpreters: resolve_ids(&self.interpreters, &profile.interpreters)?,
            candidate_generators: resolve_ids(&self.generators, &profile.candidate_generators)?,
            null_models: resolve_ids(&self.null_models, &profile.null_models)?,
        })
    }
}

fn resolve_ids<T: ?Sized + Described>(
    entries: &BTreeMap<PluginId, Arc<T>>,
    selections: &[PluginSelection],
) -> Result<Vec<Arc<T>>> {
    selections
        .iter()
        .map(|selection| {
            let plugin = entries
                .get(&selection.id)
                .ok_or_else(|| PluginError::MissingPlugin(selection.id.clone()))?;
            let actual = plugin.plugin_version();
            if !selection.version.matches(actual) {
                return Err(PluginError::IncompatibleVersion {
                    plugin: selection.id.clone(),
                    required: selection.version.clone(),
                    actual: actual.clone(),
                });
            }
            Ok(Arc::clone(plugin))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct PluginSelection {
    pub id: PluginId,
    pub version: VersionReq,
}

impl PluginSelection {
    pub fn any(id: impl Into<PluginId>) -> Self {
        Self {
            id: PluginId::new(id),
            version: VersionReq::STAR,
        }
    }
}

/// The plugins a run selects, by id and version requirement.
///
/// # Why the product-sensor families are absent
///
/// `ProductSensor` and `CrossProductSensor` register into the [`Registry`] and
/// claim their ids across every family like any other plugin, but they have no
/// field here and [`Registry::resolve`] does not resolve them. They are reached
/// by id at the point of use — `unclip_engine::cross_domain` looks one up per
/// operation through [`Registry::product_sensor`] — because a product
/// measurement names the two domains it spans in the same request that names
/// the sensor. There is no run-wide "the product sensors for this run" the way
/// there is for the six families above, so a profile field would have to be
/// either ignored or re-stated per operation.
///
/// The consequence to know: the duplicate-id check in `resolve` never sees
/// these two families, and a version requirement cannot be expressed for them.
/// Registration still rejects an id collision across all eight families, so the
/// id a caller passes resolves to exactly one plugin.
#[derive(Debug, Clone, Default)]
pub struct EngineProfile {
    pub sensors: Vec<PluginSelection>,
    pub inferrers: Vec<PluginSelection>,
    pub comparators: Vec<PluginSelection>,
    pub interpreters: Vec<PluginSelection>,
    pub candidate_generators: Vec<PluginSelection>,
    pub null_models: Vec<PluginSelection>,
}

pub struct RunPlan {
    pub sensors: Vec<Arc<dyn Sensor>>,
    pub inferrers: Vec<Arc<dyn Inferrer>>,
    pub comparators: Vec<Arc<dyn Comparator>>,
    pub interpreters: Vec<Arc<dyn Interpreter>>,
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
        sensor_with_schema(evidence, applicable, "{}")
    }

    fn sensor_with_schema(
        evidence: &'static [EvidenceRequirement],
        applicable: bool,
        params_schema: &'static str,
    ) -> Arc<dyn Sensor> {
        Arc::new(StubSensor {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.stub"),
                version: Version::new(0, 1, 0),
                stage: SensorStage::Measurement,
                applicability: &[],
                evidence,
                produces: &[],
                params_schema,
            },
            applicable,
        })
    }

    struct StubProductSensor {
        descriptor: SensorDescriptor,
    }

    impl ProductSensor for StubProductSensor {
        fn descriptor(&self) -> &SensorDescriptor {
            &self.descriptor
        }

        fn measure(
            &self,
            _ctx: &ProductMeasureCtx<'_>,
            _token: CalculationToken,
        ) -> Result<Calculated<Measurement>> {
            unreachable!("registration fixture is never executed")
        }
    }

    fn product_sensor() -> Arc<dyn ProductSensor> {
        Arc::new(StubProductSensor {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.stub"),
                version: Version::new(0, 1, 0),
                stage: SensorStage::Measurement,
                applicability: &[Capability::ProductDomain],
                evidence: &[],
                produces: &[MeasurementKind::Structured],
                params_schema: "{}",
            },
        })
    }

    struct StubCrossProductSensor {
        descriptor: SensorDescriptor,
    }

    impl CrossProductSensor for StubCrossProductSensor {
        fn descriptor(&self) -> &SensorDescriptor {
            &self.descriptor
        }

        fn measure(
            &self,
            _ctx: &CrossProductMeasureCtx<'_>,
            _token: CalculationToken,
        ) -> Result<Calculated<Measurement>> {
            unreachable!("registration fixture is never executed")
        }
    }

    fn cross_product_sensor() -> Arc<dyn CrossProductSensor> {
        Arc::new(StubCrossProductSensor {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.stub"),
                version: Version::new(0, 1, 0),
                stage: SensorStage::Measurement,
                applicability: &[Capability::ProductDomain],
                evidence: &[],
                produces: &[MeasurementKind::Structured],
                params_schema: "{}",
            },
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
            (SharedParams::new(serde_json::json!({})), 0),
            (
                SharedParams::new(serde_json::json!({"conditioning_variables": "genre"})),
                0,
            ),
            (
                SharedParams::new(
                    serde_json::json!({"conditioning_variables": [null, 1, "", " "]}),
                ),
                0,
            ),
            (
                SharedParams::new(
                    serde_json::json!({"conditioning_variables": ["genre", "genre"]}),
                ),
                1,
            ),
            (
                SharedParams::new(
                    serde_json::json!({"conditioning_variables": ["genre", "source"]}),
                ),
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

    /// A plugin whose schema cannot be enforced is refused at registration.
    ///
    /// Without this the schema was only consulted on the path that *uses* it,
    /// where a malformed one degrades to a `NotApplicable` reading — the same
    /// shape a legitimately inapplicable sensor produces. A run full of those
    /// looks like sparse evidence rather than a broken plugin.
    #[test]
    fn a_plugin_declaring_an_unenforceable_schema_cannot_register() {
        // `pattern` is a real JSON Schema keyword that `validate_params` does
        // not implement, so honoring it here would be a lie to the caller.
        const UNENFORCEABLE: &str = r#"{"type":"object","additionalProperties":false,
            "properties":{"left":{"type":"string","pattern":"^a"}}}"#;

        let mut registry = Registry::default();
        let error = registry
            .register_sensor(sensor_with_schema(&[], true, UNENFORCEABLE))
            .expect_err("an unenforceable schema must not register");

        let PluginError::MalformedSchema { plugin, reason } = error else {
            panic!("expected MalformedSchema, got {error:?}");
        };
        assert_eq!(plugin, PluginId::new("sensor.stub"));
        assert!(reason.contains("pattern"), "got: {reason}");

        // The id is left unclaimed, so the same plugin registers once fixed.
        assert!(registry
            .register_sensor(sensor_with_schema(
                &[],
                true,
                r#"{"type":"object","additionalProperties":false}"#,
            ))
            .is_ok());
    }

    #[test]
    fn a_sensor_is_not_invoked_with_parameters_its_schema_rejects() {
        use std::collections::BTreeMap;
        use unclip_domain::{DomainId, FrameId};
        use unclip_epistemic::{DomainVersion, FrameVersion};

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
        const SCHEMA: &str = r#"{"type":"object","additionalProperties":false,
            "required":["left"],"properties":{"left":{"type":"string","minLength":1}}}"#;

        // A key the schema does not declare: recorded, not passed to the sensor.
        let params: SharedParams = serde_json::json!({"left": "a", "typo": 1}).into();
        let ctx = MeasureCtx::new(
            &domain,
            &frame,
            &[],
            &[],
            &[],
            &params,
            DependencyCollector::default(),
        );
        let SensorDecision::Record(Reading::NotApplicable { reason }) =
            classify_sensor(sensor_with_schema(&[], true, SCHEMA).as_ref(), &ctx)
        else {
            panic!("a schema violation must be recorded rather than run");
        };
        assert!(reason.contains("unknown key `typo`"), "got: {reason}");

        // Satisfying the same schema leaves the decision to run.
        let params: SharedParams = serde_json::json!({"left": "a"}).into();
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
            classify_sensor(sensor_with_schema(&[], true, SCHEMA).as_ref(), &ctx),
            SensorDecision::Run
        );
    }

    #[test]
    fn planner_preserves_sparse_reading_states() {
        use std::collections::BTreeMap;
        use unclip_domain::{DomainId, FrameId};
        use unclip_epistemic::{DerivedId, DomainVersion, FrameVersion, Timestamp};

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
        let params: SharedParams = serde_json::json!({}).into();
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
            classify_sensor(sensor_with(&[], false).as_ref(), &ctx),
            SensorDecision::Record(Reading::NotApplicable {
                reason: "unsupported fixture".into()
            })
        );
        assert_eq!(
            classify_sensor(
                sensor_with(&[EvidenceRequirement::MinSamples(2)], true).as_ref(),
                &ctx
            ),
            SensorDecision::Record(Reading::InsufficientEvidence { have: 0, need: 2 })
        );
        assert_eq!(
            classify_sensor(sensor().as_ref(), &ctx),
            SensorDecision::Run
        );

        let derived = ctx
            .calculation_token(
                EmitMetadata::new(
                    DerivedId::new("measurement-1"),
                    PluginId::new("sensor.stub"),
                    Version::new(0, 1, 0),
                    serde_json::json!({}),
                    Timestamp::new("2026-09-17T00:00:00Z"),
                )
                .with_algorithm("stub"),
            )
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
        use unclip_epistemic::{DerivedId, DomainVersion, EmitMetadata, FrameVersion, Timestamp};
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
            stage: SensorStage::Measurement,
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
        let params: SharedParams = serde_json::json!({}).into();
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
                ctx.calculation_token(
                    EmitMetadata::new(
                        DerivedId::new("measurement"),
                        PluginId::new("sensor.scalar"),
                        Version::new(0, 1, 0),
                        serde_json::json!({}),
                        Timestamp::new("2026-09-17T00:00:00Z"),
                    )
                    .with_algorithm("scalar"),
                ),
            )
        });
    }

    #[test]
    fn conformance_accepts_a_deterministic_empty_sensor() {
        let sensor = sensor();
        conformance::assert_sensor(sensor.as_ref(), |_| Ok(Vec::new()));
    }

    struct StubGenerator {
        descriptor: PluginDescriptor,
    }

    impl CandidateGenerator for StubGenerator {
        fn descriptor(&self) -> &PluginDescriptor {
            &self.descriptor
        }

        fn generate(
            &self,
            _ctx: &CandidateCtx<'_>,
            _token: CalculationToken,
        ) -> Result<Vec<Calculated<unclip_domain::CandidateProposal>>> {
            unreachable!("registration fixture is never executed")
        }
    }

    fn generator() -> Arc<dyn CandidateGenerator> {
        Arc::new(StubGenerator {
            descriptor: PluginDescriptor {
                id: PluginId::new("sensor.stub"),
                version: Version::new(0, 1, 0),
                params_schema: "{}",
            },
        })
    }

    struct StubNullModel {
        descriptor: PluginDescriptor,
    }

    impl NullModel for StubNullModel {
        fn descriptor(&self) -> &PluginDescriptor {
            &self.descriptor
        }

        fn evaluate(
            &self,
            _ctx: &NullCtx<'_>,
            _token: CalculationToken,
        ) -> Result<Calculated<Reading>> {
            unreachable!("registration fixture is never executed")
        }
    }

    fn null_model() -> Arc<dyn NullModel> {
        Arc::new(StubNullModel {
            descriptor: PluginDescriptor {
                id: PluginId::new("sensor.stub"),
                version: Version::new(0, 1, 0),
                params_schema: "{}",
            },
        })
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
    fn registration_rejects_sensor_ids_across_product_contexts() {
        let mut ordinary_first = Registry::default();
        ordinary_first.register_sensor(sensor()).unwrap();
        assert_eq!(
            ordinary_first
                .register_product_sensor(product_sensor())
                .unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );

        let mut product_first = Registry::default();
        product_first
            .register_product_sensor(product_sensor())
            .unwrap();
        assert_eq!(
            product_first.register_sensor(sensor()).unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );
        assert_eq!(
            ordinary_first
                .register_cross_product_sensor(cross_product_sensor())
                .unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );
        assert_eq!(
            product_first
                .register_cross_product_sensor(cross_product_sensor())
                .unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );

        let mut cross_product_first = Registry::default();
        cross_product_first
            .register_cross_product_sensor(cross_product_sensor())
            .unwrap();
        assert_eq!(
            cross_product_first.register_sensor(sensor()).unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );
        assert_eq!(
            cross_product_first
                .register_product_sensor(product_sensor())
                .unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );
        assert_eq!(
            cross_product_first
                .register_cross_product_sensor(cross_product_sensor())
                .unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );
    }

    /// One id names one plugin registry-wide, not one per family.
    ///
    /// `resolve` already rejects a profile naming the same id in two lists, so
    /// a generator and a null model sharing an id could never both be selected.
    /// Registration rejects the ambiguity up front instead.
    #[test]
    fn registration_rejects_ids_reused_across_plugin_families() {
        let mut generator_first = Registry::default();
        generator_first.register_generator(generator()).unwrap();
        assert_eq!(
            generator_first
                .register_null_model(null_model())
                .unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );
        assert_eq!(
            generator_first.register_sensor(sensor()).unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );

        let mut null_first = Registry::default();
        null_first.register_null_model(null_model()).unwrap();
        assert_eq!(
            null_first.register_generator(generator()).unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );

        let mut sensor_first = Registry::default();
        sensor_first.register_sensor(sensor()).unwrap();
        assert_eq!(
            sensor_first.register_generator(generator()).unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );
        assert_eq!(
            sensor_first.register_null_model(null_model()).unwrap_err(),
            PluginError::DuplicatePlugin(PluginId::new("sensor.stub"))
        );
    }

    /// Derived ids join run and plugin ids with `/` and number later emissions
    /// with `#`, so a plugin id may hold neither.
    #[test]
    fn registration_rejects_ids_with_reserved_separators() {
        for (id, reserved) in [("sensor/stub", '/'), ("sensor#stub", '#')] {
            let mut registry = Registry::default();
            let error = registry
                .register_generator(Arc::new(StubGenerator {
                    descriptor: PluginDescriptor {
                        id: PluginId::new(id),
                        version: Version::new(0, 1, 0),
                        params_schema: "{}",
                    },
                }))
                .unwrap_err();
            assert_eq!(
                error,
                PluginError::InvalidPluginId {
                    plugin: PluginId::new(id),
                    reserved,
                }
            );
        }
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

    #[test]
    fn an_evidence_check_does_not_record_the_inputs_it_inspects() {
        use std::collections::BTreeMap;
        use unclip_domain::{DomainId, FrameId};
        use unclip_epistemic::{DerivedId, DomainVersion, FrameVersion, Timestamp};
        use unclip_observe::{Observation, ObservationId};

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
        // Two observations, only one of which a sensor would go on to use.
        let observations = ["kept", "ignored"]
            .map(|id| {
                Tracked::from_recorded(
                    DerivedId::new(id),
                    Observation {
                        id: ObservationId::new(id),
                        source: SourceRef::new("fixture"),
                        observed_at: Some("2026-09-17T00:00:00Z".into()),
                        units: Vec::new(),
                        relations: Vec::new(),
                        context: BTreeMap::new(),
                    },
                )
            })
            .to_vec();
        let params: SharedParams = serde_json::json!({}).into();
        let ctx = MeasureCtx::new(
            &domain,
            &frame,
            &observations,
            &[],
            &[],
            &params,
            DependencyCollector::default(),
        );

        // `Ordered` inspects every observation to count how many are ordered.
        assert_eq!(ctx.evidence_gap(EvidenceRequirement::Ordered), None);
        assert_eq!(
            ctx.evidence_gap(EvidenceRequirement::MinSamples(3)),
            Some(EvidenceGap {
                requirement: EvidenceRequirement::MinSamples(3),
                have: 2,
                need: 3
            })
        );

        // Only what the sensor itself reads becomes an input.
        let _ = ctx.read(&observations[0]);
        let derived = ctx
            .calculation_token(
                EmitMetadata::new(
                    DerivedId::new("measurement"),
                    PluginId::new("sensor.stub"),
                    Version::new(0, 1, 0),
                    ctx.shared_params(),
                    Timestamp::new("2026-09-17T00:00:00Z"),
                )
                .with_algorithm("stub"),
            )
            .emit(());
        assert_eq!(
            derived.provenance().inputs,
            vec![DerivedId::new("kept")],
            "an evidence check must not claim inputs the sensor never used"
        );
        // The provenance records the configured tree itself. A stage builds one
        // context and one token per plugin per source, so a copy here would be
        // a copy of the run's parameters per emission.
        assert!(std::sync::Arc::ptr_eq(
            &derived.provenance().params,
            &params
        ));
        assert!(std::ptr::eq(ctx.params(), &*params));
    }
}
