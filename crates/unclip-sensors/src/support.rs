use std::collections::{BTreeMap, BTreeSet};

use unclip_domain::UnitId;
use unclip_observe::{ObservationId, ObservedUnitId};
use unclip_plugin::{MeasureCtx, PluginError};

pub(crate) struct SupportAnalysis {
    pub observed_nodes: usize,
    pub observed_relations: usize,
    pub unsupported_units: Vec<String>,
    pub unexplained_relations: Vec<String>,
}

impl SupportAnalysis {
    /// Observed units the domain accounts for.
    ///
    /// `analyze` drives both counters from one pass over the same units, so the
    /// unsupported list cannot outgrow the observed count. The subtraction
    /// saturates regardless: this crate must not panic, and an underflow here
    /// would abort a sensor in debug and wrap to a huge count in release.
    pub fn supported_nodes(&self) -> usize {
        self.observed_nodes
            .saturating_sub(self.unsupported_units.len())
    }

    /// Observed relations the domain accounts for. Saturates for the same
    /// reason as [`Self::supported_nodes`].
    pub fn supported_relations(&self) -> usize {
        self.observed_relations
            .saturating_sub(self.unexplained_relations.len())
    }
}

pub(crate) fn analyze(ctx: &MeasureCtx<'_>) -> SupportAnalysis {
    let mut aligned: BTreeMap<ObservationId, BTreeMap<ObservedUnitId, BTreeSet<UnitId>>> =
        BTreeMap::new();

    for tracked in ctx.alignments() {
        let alignment = ctx.read(tracked);
        let units = aligned.entry(alignment.observation.clone()).or_default();
        for candidate in &alignment.candidates {
            units
                .entry(candidate.observed.clone())
                .or_default()
                .insert(candidate.domain.clone());
        }
    }

    let mut analysis = SupportAnalysis {
        observed_nodes: 0,
        observed_relations: 0,
        unsupported_units: Vec::new(),
        unexplained_relations: Vec::new(),
    };

    for tracked in ctx.observations() {
        let observation = ctx.read(tracked);
        let units = aligned.get(&observation.id);
        analysis.observed_nodes += observation.units.len();
        for unit in &observation.units {
            if !units.is_some_and(|units| units.contains_key(&unit.id)) {
                analysis
                    .unsupported_units
                    .push(format!("{}/{}", observation.id.0, unit.id.0));
            }
        }

        analysis.observed_relations += observation.relations.len();
        for relation in &observation.relations {
            let explained = units.is_some_and(|units| {
                let (Some(sources), Some(targets)) =
                    (units.get(&relation.source), units.get(&relation.target))
                else {
                    return false;
                };
                ctx.domain().relations.values().any(|domain_relation| {
                    domain_relation.kind == relation.kind
                        && sources.contains(&domain_relation.source)
                        && targets.contains(&domain_relation.target)
                })
            });
            if !explained {
                analysis
                    .unexplained_relations
                    .push(format!("{}/{}", observation.id.0, relation.id.0));
            }
        }
    }

    analysis
}

/// Carry a measurement calculation's own error across the plugin boundary.
///
/// The calculations in `unclip-measure` report precisely what failed. Rendering
/// them to a string here would collapse "the solver did not converge" and "the
/// input was too sparse" into the same opaque `PluginError::Message`, so they
/// are wrapped instead and stay matchable by the engine.
pub(crate) fn calculation(error: impl Into<unclip_measure::MeasureError>) -> PluginError {
    PluginError::Measure(error.into())
}

/// Report plugin parameters that do not match the sensor's declared schema.
///
/// Bad configuration and a failed calculation are different problems with
/// different fixes, so they get different variants rather than one opaque
/// message.
pub(crate) fn invalid_params(error: serde_json::Error) -> PluginError {
    PluginError::InvalidParams(error.to_string())
}
