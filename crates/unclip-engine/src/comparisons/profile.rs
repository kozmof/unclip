//! Explicit one-to-one profile pairing with tracked aggregate provenance.
use crate::comparisons::scalar::compare_measurements;
use crate::support::invalid;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    SharedParams, Tracked,
};
use unclip_measure::{Delta, Measurement};
use unclip_plugin::{Result, RunPlan};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonPair {
    pub before: DerivedId,
    pub after: DerivedId,
}
/// One comparison a profile recorded, with the delta it produced.
///
/// The delta is shared with the [`Calculated`] value returned beside the
/// profile in [`ProfileComparisonResult`]: both halves of that result describe
/// the same comparisons, and a comparator payload can be as large as the
/// graphs or matrices it compared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileDelta {
    pub pair: ComparisonPair,
    pub id: DerivedId,
    pub delta: std::sync::Arc<Delta>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeltaProfile {
    pub pairs: Vec<ComparisonPair>,
    pub deltas: Vec<ProfileDelta>,
    pub unmatched_before: Vec<DerivedId>,
    pub unmatched_after: Vec<DerivedId>,
}
pub struct ProfileComparisonResult {
    pub profile: Calculated<DeltaProfile>,
    pub deltas: Vec<Calculated<Delta>>,
}
fn index<'a>(
    inputs: &'a [Tracked<Measurement>],
    dependencies: &DependencyCollector,
) -> Result<BTreeMap<DerivedId, &'a Tracked<Measurement>>> {
    let mut result = BTreeMap::new();
    for input in inputs {
        crate::require_calculated_evidence(input, "profile input measurement")?;
        if input.id().is_empty() || result.insert(input.id().clone(), input).is_some() {
            return Err(invalid(
                "profile measurements require unique nonempty derived identities",
            ));
        }
        // Profile membership, including unmatched evidence, is a tracked input.
        dependencies.read(input);
    }
    Ok(result)
}

/// Pair only explicit identities. Unmatched evidence remains in the delta profile.
/// All configured comparators run on every pair and retain their own result states.
pub fn compare_profiles(
    plan: &RunPlan,
    before: &[Tracked<Measurement>],
    after: &[Tracked<Measurement>],
    pairs: &[ComparisonPair],
    run: crate::MeasurementRun<'_>,
) -> Result<ProfileComparisonResult> {
    if plan.comparators.is_empty() {
        return Err(invalid(
            "profile comparison requires explicitly selected comparators",
        ));
    }
    let dependencies = DependencyCollector::default();
    let before = index(before, &dependencies)?;
    let after = index(after, &dependencies)?;
    for (id, left) in &before {
        if let Some(right) = after.get(id) {
            if dependencies.read(left) != dependencies.read(right) {
                return Err(invalid(
                    "shared measurement identities have conflicting values",
                ));
            }
        }
    }
    let mut pairs = pairs.to_vec();
    pairs.sort();
    let mut used_before = BTreeSet::new();
    let mut used_after = BTreeSet::new();
    for pair in &pairs {
        if !before.contains_key(&pair.before) || !after.contains_key(&pair.after) {
            return Err(invalid(
                "comparison pair references an unselected measurement",
            ));
        }
        if !used_before.insert(pair.before.clone()) || !used_after.insert(pair.after.clone()) {
            return Err(invalid(
                "profile pairs must be one-to-one without duplicate pairings",
            ));
        }
    }
    let profile_id = DerivedId::new(format!("{}/profile", run.id));
    if before.contains_key(&profile_id) || after.contains_key(&profile_id) {
        return Err(invalid("profile output identity collides with an input"));
    }
    let mut output_ids = BTreeSet::from([profile_id.clone()]);
    let mut entries = Vec::new();
    let mut deltas = Vec::new();
    for (index, pair) in pairs.iter().enumerate() {
        let id = format!("{}/pairs/{index}", run.id);
        let results = compare_measurements(
            plan,
            before[&pair.before],
            after[&pair.after],
            crate::MeasurementRun {
                id: &id,
                timestamp: run.timestamp.clone(),
                params: run.params,
            },
        )?;
        for delta in results {
            if before.contains_key(delta.id())
                || after.contains_key(delta.id())
                || !output_ids.insert(delta.id().clone())
            {
                return Err(invalid(
                    "delta output identity collides with an input or output",
                ));
            }
            entries.push(ProfileDelta {
                pair: pair.clone(),
                id: delta.id().clone(),
                delta: dependencies.read_derived_shared(&delta),
            });
            deltas.push(delta);
        }
    }
    let mut comparators=plan.comparators.iter().map(|plugin| {
        let descriptor=plugin.descriptor();let params=run.params.get(&descriptor.id).cloned().unwrap_or_else(|| SharedParams::new(serde_json::json!({})));
        (&descriptor.id,serde_json::json!({"id":descriptor.id,"version":descriptor.version,"params":params,"params_hash":hash_params(&params)}))
    }).collect::<Vec<_>>();
    comparators.sort_by(|a, b| a.0.cmp(b.0));
    let params = serde_json::json!({"pairs":pairs,"comparators":comparators.into_iter().map(|(_,entry)| entry).collect::<Vec<_>>()});
    let token = CalculationToken::from_harness(
        EmitMetadata::new(
            profile_id,
            unclip_epistemic::PluginId::new("compare.profile"),
            semver::Version::new(0, 1, 0),
            params,
            run.timestamp,
        )
        .with_algorithm("explicit_profile_comparison"),
        dependencies,
    );
    let profile = token.emit(DeltaProfile {
        pairs,
        deltas: entries,
        unmatched_before: before
            .keys()
            .filter(|id| !used_before.contains(*id))
            .cloned()
            .collect(),
        unmatched_after: after
            .keys()
            .filter(|id| !used_after.contains(*id))
            .cloned()
            .collect(),
    });
    Ok(ProfileComparisonResult { profile, deltas })
}
