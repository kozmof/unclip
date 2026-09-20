//! Tracked inputs for calculation-only candidate generation.
use unclip_epistemic::{CalculationToken, DependencyCollector, EmitMetadata, Tracked};
use unclip_measure::Measurement;
use unclip_observe::Observation;

pub struct CandidateCtx<'a> {
    domain_version_id: &'a str,
    measurements: &'a [Tracked<Measurement>],
    observations: &'a [Tracked<Observation>],
    params: &'a serde_json::Value,
    dependencies: DependencyCollector,
}
impl<'a> CandidateCtx<'a> {
    pub fn new(
        domain_version_id: &'a str,
        measurements: &'a [Tracked<Measurement>],
        observations: &'a [Tracked<Observation>],
        params: &'a serde_json::Value,
        dependencies: DependencyCollector,
    ) -> Self {
        Self {
            domain_version_id,
            measurements,
            observations,
            params,
            dependencies,
        }
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
    pub fn read<'b, T>(&self, input: &'b Tracked<T>) -> &'b T {
        self.dependencies.read(input)
    }
    pub fn calculation_token(&self, metadata: EmitMetadata) -> CalculationToken {
        CalculationToken::from_harness(metadata, self.dependencies.clone())
    }
}
