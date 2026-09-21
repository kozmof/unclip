//! Anonymous candidate representations from tracked community and spectral results.
use serde::Deserialize;
use std::{collections::BTreeSet, num::NonZeroUsize};
use unclip_domain::{CandidateKind, CandidateProposal};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{CommunityDetection, PairwiseMetric, SpectralDecomposition};
use unclip_plugin::{CandidateCtx, CandidateGenerator, PluginDescriptor, PluginError, Result};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunityParameters {
    metric: PairwiseMetric,
    minimum_samples: NonZeroUsize,
    minimum_members: NonZeroUsize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LatentParameters {
    metric: PairwiseMetric,
    minimum_samples: NonZeroUsize,
    minimum_absolute_eigenvalue: f64,
}

pub struct CommunityCandidateGenerator {
    descriptor: PluginDescriptor,
}
pub struct LatentAxisGenerator {
    descriptor: PluginDescriptor,
}
impl Default for CommunityCandidateGenerator {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("generate.community"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["metric","minimum_samples","minimum_members"],"properties":{"metric":{"enum":["spearman","kendall","mutual_information","relative_rank_variance"]},"minimum_samples":{"type":"integer","minimum":2},"minimum_members":{"type":"integer","minimum":2}}}"#,
            },
        }
    }
}
impl Default for LatentAxisGenerator {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("generate.latent-axis"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["metric","minimum_samples","minimum_absolute_eigenvalue"],"properties":{"metric":{"enum":["spearman","kendall","mutual_information","relative_rank_variance"]},"minimum_samples":{"type":"integer","minimum":2},"minimum_absolute_eigenvalue":{"type":"number","exclusiveMinimum":0}}}"#,
            },
        }
    }
}
fn invalid(message: impl ToString) -> PluginError {
    PluginError::Message(message.to_string())
}
fn selected<'a>(
    ctx: &'a CandidateCtx<'_>,
) -> Result<
    Vec<(
        &'a unclip_epistemic::DerivedId,
        &'a unclip_measure::EmpiricalStructure,
    )>,
> {
    if ctx.domain_version_id().is_empty() {
        return Err(invalid("structure candidates require a domain version"));
    }
    let mut inputs = ctx.structures().iter().collect::<Vec<_>>();
    inputs.sort_by_key(|input| input.id());
    let mut seen = BTreeSet::new();
    let mut structures = Vec::new();
    for input in inputs {
        if !seen.insert(input.id()) {
            return Err(invalid("duplicate discovery structure"));
        }
        structures.push((input.id(), ctx.read(input)));
    }
    Ok(structures)
}
pub(super) fn validate_community(value: &CommunityDetection) -> Result<()> {
    let valid_threshold = match value.metric {
        PairwiseMetric::Spearman | PairwiseMetric::Kendall => {
            (-1.0..=1.0).contains(&value.threshold)
        }
        _ => value.threshold >= 0.0,
    };
    if !value.threshold.is_finite()
        || !valid_threshold
        || value.assessed_pairs == 0
        || value.qualifying_pairs > value.assessed_pairs
    {
        return Err(invalid("invalid community evidence counts or threshold"));
    }
    let mut units = BTreeSet::new();
    let mut required_edges = 0usize;
    for group in &value.communities {
        if group.is_empty() || group.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(invalid(
                "community members must be nonempty, unique, and ordered",
            ));
        }
        required_edges += group.len() - 1;
        for unit in group {
            if unit.0.is_empty() || !units.insert(unit) {
                return Err(invalid("community partition has empty or repeated units"));
            }
        }
    }
    if units.len() < 2 || required_edges > value.qualifying_pairs {
        return Err(invalid(
            "community partition lacks sufficient qualifying edges",
        ));
    }
    let mut unassessed = BTreeSet::new();
    for pair in &value.unassessed {
        if pair.left >= pair.right
            || !units.contains(&pair.left)
            || !units.contains(&pair.right)
            || !unassessed.insert((&pair.left, &pair.right))
        {
            return Err(invalid("invalid unassessed community pair"));
        }
    }
    let total = units
        .len()
        .checked_mul(units.len() - 1)
        .and_then(|n| n.checked_div(2))
        .ok_or_else(|| invalid("community size overflow"))?;
    if value.assessed_pairs.checked_add(value.unassessed.len()) != Some(total) {
        return Err(invalid("community pair counts do not cover the partition"));
    }
    Ok(())
}
impl CandidateGenerator for CommunityCandidateGenerator {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }
    fn generate(
        &self,
        ctx: &CandidateCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<CandidateProposal>>> {
        let params: CommunityParameters =
            serde_json::from_value(ctx.params().clone()).map_err(invalid)?;
        if params.minimum_samples.get() < 2 || params.minimum_members.get() < 2 {
            return Err(invalid(
                "community candidates require at least two members and samples",
            ));
        }
        let mut candidates = Vec::new();
        for (id, structure) in selected(ctx)? {
            if structure.kind != "communities" {
                continue;
            }
            let result = CommunityDetection::try_from(structure).map_err(invalid)?;
            validate_community(&result)?;
            if result.metric != params.metric || result.minimum_samples < params.minimum_samples {
                continue;
            }
            for (index, members) in result.communities.iter().enumerate() {
                if members.len() < params.minimum_members.get() {
                    continue;
                }
                candidates.push(token.emit(CandidateProposal {domain_version_id:ctx.domain_version_id().into(),kind:CandidateKind::CompositeMeaning,
                    value:serde_json::json!({"pattern":{"matching":"empirical_community","members":members},"evidence":{"structure":id,"community_index":index,"result":result},"selection":{"metric":params.metric,"minimum_samples":params.minimum_samples,"minimum_members":params.minimum_members}}).as_object().expect("object").clone()}));
            }
        }
        Ok(candidates)
    }
}
fn validate_spectral(value: &SpectralDecomposition) -> Result<()> {
    let n = value.units.len();
    if n == 0
        || value.eigenpairs.len() != n
        || value.units.windows(2).any(|pair| pair[0] >= pair[1])
        || value.units.iter().any(|unit| unit.0.is_empty())
        || value.minimum_cell_samples < 2
        || !value.tolerance.is_finite()
        || value.tolerance <= 0.0
        || value.tolerance >= 1.0
    {
        return Err(invalid("invalid spectral dimensions, units, or evidence"));
    }
    for pair in &value.eigenpairs {
        if !pair.eigenvalue.is_finite()
            || pair.loadings.len() != n
            || pair.loadings.iter().any(|value| !value.is_finite())
        {
            return Err(invalid("invalid spectral eigenpair"));
        }
        let norm = pair.loadings.iter().map(|value| value * value).sum::<f64>();
        if !norm.is_finite() || (norm - 1.0).abs() > 1e-8 {
            return Err(invalid("spectral loadings must have unit norm"));
        }
    }
    Ok(())
}
impl CandidateGenerator for LatentAxisGenerator {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }
    fn generate(
        &self,
        ctx: &CandidateCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<CandidateProposal>>> {
        let params: LatentParameters =
            serde_json::from_value(ctx.params().clone()).map_err(invalid)?;
        if params.minimum_samples.get() < 2
            || !params.minimum_absolute_eigenvalue.is_finite()
            || params.minimum_absolute_eigenvalue <= 0.0
        {
            return Err(invalid("latent candidates require at least two samples and a positive finite eigenvalue magnitude floor"));
        }
        let mut candidates = Vec::new();
        for (id, structure) in selected(ctx)? {
            if structure.kind != "spectral" {
                continue;
            }
            let result = SpectralDecomposition::try_from(structure).map_err(invalid)?;
            validate_spectral(&result)?;
            if result.metric != params.metric
                || result.minimum_cell_samples < params.minimum_samples.get()
                || result.units.len() < 2
            {
                continue;
            }
            for (index, pair) in result.eigenpairs.iter().enumerate() {
                if pair.eigenvalue.abs() < params.minimum_absolute_eigenvalue {
                    continue;
                }
                candidates.push(token.emit(CandidateProposal {domain_version_id:ctx.domain_version_id().into(),kind:CandidateKind::LatentAxis,
                    value:serde_json::json!({"pattern":{"matching":"empirical_spectral_axis","units":result.units,"eigenvalue":pair.eigenvalue,"loadings":pair.loadings},"evidence":{"structure":id,"eigenpair_index":index,"result":result},"selection":{"metric":params.metric,"minimum_samples":params.minimum_samples,"minimum_absolute_eigenvalue":params.minimum_absolute_eigenvalue}}).as_object().expect("object").clone()}));
            }
        }
        Ok(candidates)
    }
}
