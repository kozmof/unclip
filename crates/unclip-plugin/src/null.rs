//! Tracked evidence for calculation-only null explanations.
use unclip_domain::{CandidateProposal, DomainSnapshot};
use unclip_epistemic::{
    CalculationToken, DependencyCollector, EmitMetadata, SharedParams, Tracked,
};
use unclip_observe::{Observation, PartialRanking};

pub struct NullCtx<'a> {
    candidate: &'a Tracked<CandidateProposal>,
    observations: &'a [Tracked<Observation>],
    rankings: &'a [Tracked<PartialRanking>],
    domain: Option<&'a Tracked<DomainSnapshot>>,
    params: &'a SharedParams,
    dependencies: DependencyCollector,
}
impl<'a> NullCtx<'a> {
    pub fn new(
        candidate: &'a Tracked<CandidateProposal>,
        observations: &'a [Tracked<Observation>],
        params: &'a SharedParams,
        dependencies: DependencyCollector,
    ) -> Self {
        Self {
            candidate,
            rankings: &[],
            domain: None,
            observations,
            params,
            dependencies,
        }
    }
    #[must_use]
    pub fn with_domain(mut self, domain: Option<&'a Tracked<DomainSnapshot>>) -> Self {
        self.domain = domain;
        self
    }
    pub fn domain(&self) -> Option<&DomainSnapshot> {
        self.domain.map(|domain| self.read(domain))
    }
    #[must_use]
    pub fn with_rankings(mut self, rankings: &'a [Tracked<PartialRanking>]) -> Self {
        self.rankings = rankings;
        self
    }
    pub fn rankings(&self) -> &[Tracked<PartialRanking>] {
        self.rankings
    }
    pub fn candidate(&self) -> &CandidateProposal {
        self.read(self.candidate)
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
