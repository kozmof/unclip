//! Tracked measurement inputs and explicit comparator parameters.
use unclip_epistemic::{CalculationToken, DependencyCollector, EmitMetadata, Tracked};
use unclip_measure::Measurement;
pub struct CompareCtx<'a> {
    before: &'a Tracked<Measurement>,
    after: &'a Tracked<Measurement>,
    params: &'a serde_json::Value,
    dependencies: DependencyCollector,
}
impl<'a> CompareCtx<'a> {
    pub fn new(
        before: &'a Tracked<Measurement>,
        after: &'a Tracked<Measurement>,
        params: &'a serde_json::Value,
        dependencies: DependencyCollector,
    ) -> Self {
        Self {
            before,
            after,
            params,
            dependencies,
        }
    }
    pub fn before(&self) -> &Measurement {
        self.dependencies.read(self.before)
    }
    pub fn after(&self) -> &Measurement {
        self.dependencies.read(self.after)
    }
    pub fn params(&self) -> &serde_json::Value {
        self.params
    }
    pub fn calculation_token(&self, metadata: EmitMetadata) -> CalculationToken {
        CalculationToken::from_harness(metadata, self.dependencies.clone())
    }
}
