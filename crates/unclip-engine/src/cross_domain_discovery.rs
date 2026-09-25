//! Anonymous cross-domain proposals from typed product-independence deviations.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use unclip_domain::{CandidateKind, CandidateProposal};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Timestamp, Tracked,
};
use unclip_measure::{EmpiricalStructure, ProductMeasurementBinding, Reading};
use unclip_plugin::{CandidateCtx, CandidateGenerator, PluginDescriptor, PluginError, Result};

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

fn invalid(message: impl Into<String>) -> PluginError {
    PluginError::Message(message.into())
}

fn valid_binding(binding: &ProductMeasurementBinding) -> bool {
    !binding.product.0.trim().is_empty()
        && !binding.product_version.0.trim().is_empty()
        && !binding.frame.0.trim().is_empty()
        && !binding.frame_version.0.trim().is_empty()
        && !binding.left.domain.0.trim().is_empty()
        && !binding.left.version.0.trim().is_empty()
        && !binding.right.domain.0.trim().is_empty()
        && !binding.right.version.0.trim().is_empty()
}

fn is_measured_deviation(entry: &IndependenceComparisonEntry) -> Result<bool> {
    if entry.product_measurement.0.trim().is_empty()
        || entry.expectation_measurement.0.trim().is_empty()
        || entry.comparator.0.trim().is_empty()
        || entry.delta_id.0.trim().is_empty()
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
    serde_json::to_string(&(&input.domain.0, &input.version.0))
}

fn validate_evidence(value: &CrossDomainDeviationEvidence) -> Result<()> {
    if value.comparison_profile.0.trim().is_empty()
        || !valid_binding(&value.binding)
        || !is_measured_deviation(&value.comparison)?
    {
        return Err(invalid(
            "cross-domain candidate evidence must be a measured typed deviation from independence",
        ));
    }
    Ok(())
}

impl crate::Engine {
    /// Derive one anonymous empirical structure per measured typed deviation.
    ///
    /// Equal readings and unavailable comparisons produce no structure. The
    /// complete product binding and typed comparison remain intact for candidate
    /// generation and later interpretation.
    pub fn derive_cross_domain_deviations(
        &self,
        profile: &Calculated<IndependenceComparisonProfile>,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Vec<Calculated<EmpiricalStructure>>> {
        if run_id.trim().is_empty() {
            return Err(invalid(
                "cross-domain deviation derivation requires a nonempty run ID",
            ));
        }
        let provenance = profile.provenance();
        if profile.id().0.trim().is_empty()
            || provenance.producer != PluginId::new("compare.product-independence")
            || provenance.algorithm != "explicit_typed_product_independence_comparison"
            || provenance.params_hash != hash_params(&provenance.params)
            || profile.value().composition_profile.0.trim().is_empty()
            || profile.value().expectation_profile.0.trim().is_empty()
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
                value: serde_json::to_value(&evidence)
                    .map_err(|error| invalid(error.to_string()))?,
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
            dependencies.read(&Tracked::from(profile));
            let token = CalculationToken::from_harness(
                EmitMetadata {
                    id: output_id,
                    producer: PluginId::new("calculate.cross-domain-deviation"),
                    algorithm: "typed_product_independence_deviation".into(),
                    version: semver::Version::new(0, 1, 0),
                    params_hash: hash_params(&params),
                    params,
                    source: None,
                    timestamp: timestamp.clone(),
                    domain_version: None,
                    frame_version: None,
                    model: None,
                },
                dependencies,
            );
            results.push(token.emit(value));
        }
        Ok(results)
    }
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
        let _: Parameters = serde_json::from_value(ctx.params().clone())
            .map_err(|error| invalid(error.to_string()))?;
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
                serde_json::from_value(value.value.clone())
                    .map_err(|error| invalid(error.to_string()))?;
            validate_evidence(&evidence)?;
            let left =
                domain_key(&evidence.binding.left).map_err(|error| invalid(error.to_string()))?;
            let right =
                domain_key(&evidence.binding.right).map_err(|error| invalid(error.to_string()))?;
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
