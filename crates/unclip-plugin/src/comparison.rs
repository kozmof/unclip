//! Tracked measurement inputs and explicit comparator parameters.
use unclip_epistemic::{
    CalculationToken, DependencyCollector, EmitMetadata, SharedParams, Tracked,
};
use unclip_measure::Measurement;
pub struct CompareCtx<'a> {
    before: &'a Tracked<Measurement>,
    after: &'a Tracked<Measurement>,
    params: &'a SharedParams,
    dependencies: DependencyCollector,
}
impl<'a> CompareCtx<'a> {
    pub fn new(
        before: &'a Tracked<Measurement>,
        after: &'a Tracked<Measurement>,
        params: &'a SharedParams,
        dependencies: DependencyCollector,
    ) -> Self {
        Self {
            before,
            after,
            params,
            dependencies,
        }
    }
    pub fn before(&self) -> &'a Measurement {
        self.dependencies.read(self.before)
    }
    pub fn after(&self) -> &'a Measurement {
        self.dependencies.read(self.after)
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
    pub fn calculation_token(&self, metadata: EmitMetadata) -> CalculationToken {
        CalculationToken::from_harness(metadata, self.dependencies.clone())
    }
}
