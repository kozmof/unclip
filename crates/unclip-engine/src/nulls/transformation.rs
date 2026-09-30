//! Exact existing-domain alternative for transformation proposals.

use serde::Deserialize;
use unclip_domain::{CandidateKind, UnitKind};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::Reading;
use unclip_plugin::{NullCtx, NullModel, PluginDescriptor, Result};

use crate::nulls::exact_pattern::ExactPatternNull;
use crate::support::invalid_params;

const NULL: ExactPatternNull = ExactPatternNull {
    candidate_kind: CandidateKind::Transformation,
    unit_kind: UnitKind::Transformation,
    property: "transformation_pattern",
    model: "existing_transformation_exact_pattern",
    matching: "exact_unit_state_transition",
    scope: "exact directed transformation pattern identity only; labels, causality, and semantic equivalence are excluded",
    not_applicable: "requires an exact transformation proposal",
    version_mismatch: "transformation null baseline differs from candidate domain version",
};

pub struct ExistingTransformationNull {
    descriptor: PluginDescriptor,
}

impl Default for ExistingTransformationNull {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("null.existing-transformation"),
                version: "0.1.0".parse().expect("valid transformation-null version"),
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {}

impl NullModel for ExistingTransformationNull {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    fn evaluate(&self, ctx: &NullCtx<'_>, token: CalculationToken) -> Result<Calculated<Reading>> {
        let _: Parameters =
            serde::Deserialize::deserialize(ctx.params()).map_err(invalid_params)?;
        NULL.evaluate(ctx, token, crate::applications::transformation::validate)
    }
}
