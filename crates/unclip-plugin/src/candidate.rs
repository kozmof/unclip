//! Tracked inputs for calculation-only candidate generation.
use unclip_epistemic::{
    CalculationToken, DependencyCollector, EmitMetadata, SharedParams, Tracked,
};
use unclip_measure::{EmpiricalStructure, Measurement};
use unclip_observe::Observation;

pub struct CandidateCtx<'a> {
    domain_version_id: &'a str,
    measurements: &'a [Tracked<Measurement>],
    observations: &'a [Tracked<Observation>],
    structures: &'a [Tracked<EmpiricalStructure>],
    params: &'a SharedParams,
    dependencies: DependencyCollector,
}
impl<'a> CandidateCtx<'a> {
    pub fn new(
        domain_version_id: &'a str,
        measurements: &'a [Tracked<Measurement>],
        observations: &'a [Tracked<Observation>],
        params: &'a SharedParams,
        dependencies: DependencyCollector,
    ) -> Self {
        Self {
            domain_version_id,
            measurements,
            observations,
            params,
            dependencies,
            structures: &[],
        }
    }
    #[must_use]
    pub fn with_structures(mut self, structures: &'a [Tracked<EmpiricalStructure>]) -> Self {
        self.structures = structures;
        self
    }
    pub fn structures(&self) -> &[Tracked<EmpiricalStructure>] {
        self.structures
    }
    pub fn domain_version_id(&self) -> &str {
        self.domain_version_id
    }
    pub fn measurements(&self) -> &[Tracked<Measurement>] {
        self.measurements
    }
    pub fn observations(&self) -> &[Tracked<Observation>] {
        self.observations
    }
    pub fn params(&self) -> &serde_json::Value {
        self.params
    }

    /// The same parameters as a shared handle, for an [`EmitMetadata`] that
    /// keeps them: a provenance record aliases the run's configured tree
    /// instead of copying it.
    pub fn shared_params(&self) -> SharedParams {
        SharedParams::clone(self.params)
    }
    pub fn read<'b, T>(&self, input: &'b Tracked<T>) -> &'b T {
        self.dependencies.read(input)
    }
    pub fn calculation_token(&self, metadata: EmitMetadata) -> CalculationToken {
        CalculationToken::from_harness(metadata, self.dependencies.clone())
    }
}
