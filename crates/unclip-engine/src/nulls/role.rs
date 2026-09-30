//! Exact existing-domain alternative for semantic-role proposals.

use serde::Deserialize;
use unclip_domain::{CandidateKind, UnitKind};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::Reading;
use unclip_plugin::{NullCtx, NullModel, PluginDescriptor, Result};

use crate::nulls::exact_pattern::ExactPatternNull;
use crate::support::invalid_params;

const NULL: ExactPatternNull = ExactPatternNull {
    candidate_kind: CandidateKind::SemanticRole,
    unit_kind: UnitKind::SemanticRole,
    property: "role_pattern",
    model: "existing_role_exact_pattern",
    matching: "exact_relation_kind_signature",
    scope:
        "exact semantic-role pattern identity only; labels and semantic equivalence are excluded",
    not_applicable: "requires an exact semantic-role proposal",
    version_mismatch: "semantic-role null baseline differs from candidate domain version",
};

pub struct ExistingRoleNull {
    descriptor: PluginDescriptor,
}

impl Default for ExistingRoleNull {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("null.existing-role"),
                version: "0.1.0".parse().expect("valid role-null version"),
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {}

impl NullModel for ExistingRoleNull {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    fn evaluate(&self, ctx: &NullCtx<'_>, token: CalculationToken) -> Result<Calculated<Reading>> {
        let _: Parameters =
            serde::Deserialize::deserialize(ctx.params()).map_err(invalid_params)?;
        NULL.evaluate(ctx, token, crate::applications::role::validate)
    }
}
