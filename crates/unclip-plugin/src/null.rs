//! Tracked evidence for calculation-only null explanations.
use unclip_domain::CandidateProposal;
use unclip_epistemic::{CalculationToken, DependencyCollector, EmitMetadata, Tracked};
use unclip_observe::{Observation, PartialRanking};

pub struct NullCtx<'a> {
    candidate: &'a Tracked<CandidateProposal>,
    observations: &'a [Tracked<Observation>],
    rankings: &'a [Tracked<PartialRanking>],
    params: &'a serde_json::Value,
    dependencies: DependencyCollector,
}
impl<'a> NullCtx<'a> {
    pub fn new(
        candidate: &'a Tracked<CandidateProposal>,
        observations: &'a [Tracked<Observation>],
        params: &'a serde_json::Value,
        dependencies: DependencyCollector,
    ) -> Self {
        Self {
            candidate,
            rankings: &[],
            observations,
            params,
            dependencies,
        }
    }
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
    pub fn read<'b, T>(&self, input: &'b Tracked<T>) -> &'b T {
        self.dependencies.read(input)
    }
    pub fn calculation_token(&self, metadata: EmitMetadata) -> CalculationToken {
        CalculationToken::from_harness(metadata, self.dependencies.clone())
    }
}
