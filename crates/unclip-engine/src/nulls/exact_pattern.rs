//! The shared body of the exact-pattern null models.
//!
//! `null.existing-role` and `null.existing-transformation` ask the same
//! question — does a unit of this kind already carry exactly this pattern? —
//! and differed only in six values. They were two 86-line files that were
//! byte-identical modulo identifiers, which is how one of them came to lose a
//! comment the other kept.
//!
//! `null.existing-motif` is deliberately **not** routed through here despite
//! looking similar. It also inspects the pattern's `matching` discriminator when
//! deciding applicability, and it validates the candidate *before* checking that
//! a baseline domain was supplied — so for an invalid candidate with no domain
//! it reports the candidate error where these two report insufficient evidence.
//! Folding that in would need a flag that reorders the checks, and a shared
//! function whose control flow is a parameter is worse than the duplication it
//! removes.

use unclip_domain::{CandidateKind, DomainSnapshot, UnitKind};
use unclip_epistemic::{Calculated, CalculationToken};
use unclip_measure::{MeasurementValue, Reading};
use unclip_plugin::{NullCtx, Result};

use crate::support::invalid;

/// What distinguishes one exact-pattern null from another.
///
/// Everything here is data the descriptor already implies. Passing it as a
/// struct rather than nine arguments is the same reason the rest of the crate
/// does: several of these are `&'static str` and would otherwise be
/// transposable without the compiler noticing.
pub(crate) struct ExactPatternNull {
    /// The proposal kind this null is an alternative to.
    pub candidate_kind: CandidateKind,
    /// The domain unit kind searched for an existing match.
    pub unit_kind: UnitKind,
    /// The unit property holding the stored pattern.
    pub property: &'static str,
    /// Recorded as `model`, naming the comparison performed.
    pub model: &'static str,
    /// Recorded as `matching`, naming the pattern identity used.
    pub matching: &'static str,
    /// Recorded as `scope`, stating what the comparison does *not* establish.
    pub scope: &'static str,
    /// Reported when the candidate is of another kind.
    pub not_applicable: &'static str,
    /// Reported when the candidate's baseline is not the supplied domain.
    pub version_mismatch: &'static str,
}

impl ExactPatternNull {
    /// Evaluate this null against the context's candidate and baseline domain.
    ///
    /// `validate` is the candidate's own structural check, run after the
    /// baseline is confirmed so a missing domain is reported as insufficient
    /// evidence rather than as a candidate defect.
    ///
    /// Its success value is generic and discarded: the per-kind validators
    /// return their own parsed pattern type, while the comparison below needs
    /// the raw JSON to match against a stored `PropertyValue::Structured`. Only
    /// the validation itself is wanted here.
    pub(crate) fn evaluate<T>(
        &self,
        ctx: &NullCtx<'_>,
        token: CalculationToken,
        validate: impl Fn(&unclip_domain::CandidateProposal, &DomainSnapshot) -> Result<T>,
    ) -> Result<Calculated<Reading>> {
        let candidate = ctx.candidate();
        if candidate.kind != self.candidate_kind {
            return Ok(token.emit(Reading::NotApplicable {
                reason: self.not_applicable.into(),
            }));
        }
        let Some(domain) = ctx.domain() else {
            return Ok(token.emit(Reading::InsufficientEvidence { have: 0, need: 1 }));
        };
        crate::nulls::domain::validate(domain)?;
        let key = serde_json::to_string(&(&domain.id.0, &domain.version.0)).map_err(invalid)?;
        if candidate.domain_version_id != key {
            return Err(invalid(self.version_mismatch));
        }
        validate(candidate, domain)?;
        // `validate` establishes the pattern is present, so its absence here
        // would be that function no longer checking what it says it does.
        let pattern = candidate
            .value
            .get("pattern")
            .expect("validated candidate carries a pattern");
        let matches = domain
            .units
            .values()
            .filter(|unit| {
                unit.kind == self.unit_kind
                    && unit
                        .properties
                        .get(self.property)
                        .and_then(|value| match value {
                            unclip_domain::PropertyValue::Structured(value) => Some(value),
                            _ => None,
                        })
                        == Some(pattern)
            })
            .map(|unit| &unit.id)
            .collect::<Vec<_>>();
        Ok(token.emit(Reading::Value {
            value: MeasurementValue::Structured(serde_json::json!({
                "model": self.model,
                "domain_version_id": key,
                "matching": self.matching,
                "match_count": matches.len(),
                "matches": matches,
                "has_existing_alternative": !matches.is_empty(),
                "scope": self.scope,
                "decision": "no automatic candidate acceptance or rejection"
            })),
        }))
    }
}
