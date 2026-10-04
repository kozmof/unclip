//! Anonymous cross-domain proposals from typed product-independence deviations.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use unclip_domain::{CandidateKind, CandidateProposal};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Timestamp,
};
use unclip_measure::{EmpiricalStructure, ProductMeasurementBinding, Reading};
use unclip_plugin::{CandidateCtx, CandidateGenerator, PluginDescriptor, Result};

use crate::support::{invalid, invalid_params};
use crate::{IndependenceComparisonEntry, IndependenceComparisonProfile};

const STRUCTURE_KIND: &str = "cross_domain_independence_deviation";

/// Typed comparison evidence retained without assigning semantic meaning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDomainDeviationEvidence {
    pub comparison_profile: DerivedId,
    pub binding: ProductMeasurementBinding,
    pub comparison: IndependenceComparisonEntry,
}

pub struct CrossDomainCandidateGenerator {
    descriptor: PluginDescriptor,
}

impl Default for CrossDomainCandidateGenerator {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("generate.cross-domain-structure"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {}

fn valid_binding(binding: &ProductMeasurementBinding) -> bool {
    !binding.product.trim().is_empty()
        && !binding.product_version.trim().is_empty()
        && !binding.frame.trim().is_empty()
        && !binding.frame_version.trim().is_empty()
        && !binding.left.domain.trim().is_empty()
        && !binding.left.version.trim().is_empty()
        && !binding.right.domain.trim().is_empty()
        && !binding.right.version.trim().is_empty()
}

fn is_measured_deviation(entry: &IndependenceComparisonEntry) -> Result<bool> {
    if entry.product_measurement.trim().is_empty()
        || entry.expectation_measurement.trim().is_empty()
        || entry.comparator.trim().is_empty()
        || entry.delta_id.trim().is_empty()
        || entry.delta.comparator != entry.comparator
    {
        return Err(invalid(
            "cross-domain deviation evidence requires consistent nonempty comparison identities",
        ));
    }
    match (&entry.expected, &entry.observed) {
        (Reading::Value { value: expected }, Reading::Value { value: observed }) => {
            if expected.kind() != observed.kind() {
                return Err(invalid(
                    "cross-domain deviation evidence requires matching typed readings",
                ));
            }
            Ok(expected != observed)
        }
        _ => Ok(false),
    }
}

fn domain_key(
    input: &unclip_domain::ProductDomainInput,
) -> std::result::Result<String, serde_json::Error> {
    serde_json::to_string(&(input.domain.as_str(), input.version.as_str()))
}

fn validate_evidence(value: &CrossDomainDeviationEvidence) -> Result<()> {
    if value.comparison_profile.trim().is_empty()
        || !valid_binding(&value.binding)
        || !is_measured_deviation(&value.comparison)?
    {
        return Err(invalid(
            "cross-domain candidate evidence must be a measured typed deviation from independence",
        ));
    }
    Ok(())
}

impl CandidateGenerator for CrossDomainCandidateGenerator {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    fn generate(
        &self,
        ctx: &CandidateCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<CandidateProposal>>> {
        let _: Parameters =
            serde::Deserialize::deserialize(ctx.params()).map_err(invalid_params)?;
        if ctx.domain_version_id().trim().is_empty() {
            return Err(invalid(
                "cross-domain candidates require a target source-domain version",
            ));
        }

        let mut structures = ctx.structures().iter().collect::<Vec<_>>();
        structures.sort_by_key(|structure| structure.id());
        let mut identities = BTreeSet::new();
        let mut candidates = Vec::new();
        for structure in structures {
            if !identities.insert(structure.id()) {
                return Err(invalid("duplicate cross-domain candidate structure"));
            }
            let value = ctx.read(structure);
            if value.kind != STRUCTURE_KIND {
                continue;
            }
            let evidence: CrossDomainDeviationEvidence =
                serde::Deserialize::deserialize(&value.value).map_err(invalid)?;
            validate_evidence(&evidence)?;
            let left = domain_key(&evidence.binding.left).map_err(invalid)?;
            let right = domain_key(&evidence.binding.right).map_err(invalid)?;
            if ctx.domain_version_id() != left && ctx.domain_version_id() != right {
                return Err(invalid(
                    "cross-domain candidate target must be one of the product source-domain versions",
                ));
            }
            let comparison = &evidence.comparison;
            candidates.push(
                token.emit(CandidateProposal {
                    domain_version_id: ctx.domain_version_id().into(),
                    kind: CandidateKind::CrossDomainStructure,
                    value: serde_json::json!({
                        "pattern": {
                            "matching": "typed_product_deviation_from_independence",
                            "product_measurement": &comparison.product_measurement,
                            "comparator": &comparison.comparator,
                        },
                        "evidence": {
                            "structure": structure.id(),
                            "comparison_profile": &evidence.comparison_profile,
                            "binding": &evidence.binding,
                            "expectation_measurement": &comparison.expectation_measurement,
                            "delta": &comparison.delta_id,
                            "expected": &comparison.expected,
                            "observed": &comparison.observed,
                            "typed_delta": &comparison.delta,
                        }
                    })
                    .as_object()
                    .expect("candidate value is an object")
                    .clone(),
                }),
            );
        }
        Ok(candidates)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CrossDomainPattern {
    matching: String,
    product_measurement: DerivedId,
    comparator: PluginId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CrossDomainEvidence {
    structure: DerivedId,
    comparison_profile: DerivedId,
    binding: ProductMeasurementBinding,
    expectation_measurement: DerivedId,
    delta: DerivedId,
    expected: Reading,
    observed: Reading,
    typed_delta: unclip_measure::Delta,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CrossDomainProposalValue {
    pattern: CrossDomainPattern,
    evidence: CrossDomainEvidence,
}

pub(crate) fn validate_candidate(
    proposal: &CandidateProposal,
) -> Result<ProductMeasurementBinding> {
    if proposal.kind != CandidateKind::CrossDomainStructure {
        return Err(invalid(
            "cross-domain application requires a cross-domain structure candidate",
        ));
    }
    let value: CrossDomainProposalValue =
        serde::Deserialize::deserialize(serde::de::value::MapDeserializer::new(
            proposal
                .value
                .iter()
                .map(|(key, value)| (key.as_str(), value)),
        ))
        .map_err(invalid)?;
    if value.pattern.matching != "typed_product_deviation_from_independence"
        || value.evidence.structure.trim().is_empty()
        || value.pattern.product_measurement.trim().is_empty()
        || value.pattern.comparator.trim().is_empty()
    {
        return Err(invalid(
            "cross-domain application requires anonymous typed deviation evidence",
        ));
    }
    let comparison = IndependenceComparisonEntry {
        product_measurement: value.pattern.product_measurement,
        expectation_measurement: value.evidence.expectation_measurement,
        comparator: value.pattern.comparator,
        delta_id: value.evidence.delta,
        expected: value.evidence.expected,
        observed: value.evidence.observed,
        delta: std::sync::Arc::new(value.evidence.typed_delta),
    };
    validate_evidence(&CrossDomainDeviationEvidence {
        comparison_profile: value.evidence.comparison_profile,
        binding: value.evidence.binding.clone(),
        comparison,
    })?;
    let left = domain_key(&value.evidence.binding.left).map_err(invalid)?;
    let right = domain_key(&value.evidence.binding.right).map_err(invalid)?;
    if proposal.domain_version_id != left && proposal.domain_version_id != right {
        return Err(invalid(
            "cross-domain candidate target must be one of the product source-domain versions",
        ));
    }
    Ok(value.evidence.binding)
}

/// Derive one anonymous empirical structure per measured typed deviation.
///
/// Equal readings and unavailable comparisons produce no structure. The
/// complete product binding and typed comparison remain intact for candidate
/// generation and later interpretation.
pub fn derive_cross_domain_deviations(
    profile: &Calculated<IndependenceComparisonProfile>,
    run_id: &str,
    timestamp: &Timestamp,
) -> Result<Vec<Calculated<EmpiricalStructure>>> {
    if run_id.trim().is_empty() {
        return Err(invalid(
            "cross-domain deviation derivation requires a nonempty run ID",
        ));
    }
    let provenance = profile.provenance();
    if profile.id().trim().is_empty()
        || provenance.producer != PluginId::new("compare.product-independence")
        || provenance.algorithm.as_ref() != "explicit_typed_product_independence_comparison"
        || provenance.params_hash != hash_params(&provenance.params)
        || profile.value().composition_profile.trim().is_empty()
        || profile.value().expectation_profile.trim().is_empty()
        || !valid_binding(&profile.value().binding)
    {
        return Err(invalid(
            "cross-domain deviations require a valid calculated independence comparison profile",
        ));
    }

    let mut comparisons = profile.value().comparisons.iter().collect::<Vec<_>>();
    comparisons.sort_by(|left, right| {
        (&left.product_measurement, &left.comparator)
            .cmp(&(&right.product_measurement, &right.comparator))
    });
    let mut pairs = BTreeSet::new();
    let mut deltas = BTreeSet::new();
    let mut results = Vec::new();
    for (index, comparison) in comparisons.into_iter().enumerate() {
        if !pairs.insert((&comparison.product_measurement, &comparison.comparator))
            || !deltas.insert(&comparison.delta_id)
        {
            return Err(invalid(
                "cross-domain deviation comparisons require unique measurement/comparator pairs and deltas",
            ));
        }
        if !is_measured_deviation(comparison)? {
            continue;
        }
        let evidence = CrossDomainDeviationEvidence {
            comparison_profile: profile.id().clone(),
            binding: profile.value().binding.clone(),
            comparison: comparison.clone(),
        };
        let value = EmpiricalStructure {
            kind: STRUCTURE_KIND.into(),
            value: serde_json::to_value(&evidence).map_err(invalid)?,
        };
        let params = serde_json::json!({
            "comparison_profile": profile.id(),
            "binding": &evidence.binding,
            "product_measurement": &comparison.product_measurement,
            "comparator": &comparison.comparator,
            "delta": &comparison.delta_id,
            "selection": "exact_typed_reading_inequality",
        });
        let output_id = DerivedId::new(format!("{run_id}/cross-domain-deviations/{index}"));
        if &output_id == profile.id() {
            return Err(invalid(
                "cross-domain deviation output identity collides with its comparison input",
            ));
        }
        let dependencies = DependencyCollector::default();
        dependencies.read_derived(profile);
        let token = CalculationToken::from_harness(
            EmitMetadata::new(
                output_id,
                PluginId::new("calculate.cross-domain-deviation"),
                semver::Version::new(0, 1, 0),
                params,
                timestamp.clone(),
            )
            .with_algorithm("typed_product_independence_deviation"),
            dependencies,
        );
        results.push(token.emit(value));
    }
    Ok(results)
}
