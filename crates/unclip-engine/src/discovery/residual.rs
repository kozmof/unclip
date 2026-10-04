//! Deterministic proposals from recurring unmatched observed units.
use crate::support::{invalid, invalid_params};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use unclip_domain::{CandidateKind, CandidateProposal};
use unclip_epistemic::{
    Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata, PluginId,
    SharedParams, Tracked,
};
use unclip_measure::{Measurement, MeasurementValue, Reading};
use unclip_observe::Observation;
use unclip_plugin::{CandidateCtx, CandidateGenerator, PluginDescriptor, Result, RunPlan};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    minimum_observations: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Residual {
    count: usize,
    ids: Vec<String>,
}

pub struct PersistentResidualGenerator {
    descriptor: PluginDescriptor,
}
impl Default for PersistentResidualGenerator {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("generate.persistent-residual"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{
                    "type":"object",
                    "additionalProperties":false,
                    "required":["minimum_observations"],
                    "properties":{"minimum_observations":{"type":"integer","minimum":2}}
                }"#,
            },
        }
    }
}
pub(crate) fn residual_evidence(
    ctx: &CandidateCtx<'_>,
    kind: &str,
) -> Result<BTreeMap<String, BTreeSet<DerivedId>>> {
    let mut measurements = BTreeSet::new();
    let mut residuals = BTreeMap::<String, BTreeSet<DerivedId>>::new();
    for tracked in ctx.measurements() {
        if !measurements.insert(tracked.id()) {
            return Err(invalid("duplicate discovery measurement"));
        }
        let measurement = ctx.read(tracked);
        if measurement.sensor.as_str() != "sensor.residual"
            || measurement
                .context
                .values
                .get("residual_kind")
                .and_then(serde_json::Value::as_str)
                != Some(kind)
        {
            continue;
        }
        let Reading::Value { value } = &measurement.reading else {
            continue;
        };
        let MeasurementValue::Structured(value) = value else {
            return Err(invalid("residual measurement must be structured"));
        };
        let residual: Residual = serde::Deserialize::deserialize(&**value).map_err(invalid)?;
        let unique = residual.ids.iter().collect::<BTreeSet<_>>();
        if residual.count != residual.ids.len() || unique.len() != residual.ids.len() {
            return Err(invalid("residual count and unique identities must agree"));
        }
        for id in residual.ids {
            residuals
                .entry(id)
                .or_default()
                .insert(tracked.id().clone());
        }
    }
    Ok(residuals)
}

pub(crate) fn minimum_observations(ctx: &CandidateCtx<'_>) -> Result<usize> {
    let params: Parameters =
        serde::Deserialize::deserialize(ctx.params()).map_err(invalid_params)?;
    if params.minimum_observations < 2 || ctx.domain_version_id().is_empty() {
        return Err(invalid(
            "persistent residuals require a domain version and at least two observations",
        ));
    }
    Ok(params.minimum_observations)
}

impl CandidateGenerator for PersistentResidualGenerator {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }
    fn generate(
        &self,
        ctx: &CandidateCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<CandidateProposal>>> {
        let minimum = minimum_observations(ctx)?;
        // Qualified residual IDs are resolved against actual observations rather
        // than split on '/', which may occur in either kind of identifier.
        let mut units = BTreeMap::new();
        let mut observations = BTreeSet::new();
        for tracked in ctx.observations() {
            let observation = ctx.read(tracked);
            if !observations.insert(&observation.id) {
                return Err(invalid("duplicate discovery observation"));
            }
            for unit in &observation.units {
                let qualified = format!("{}/{}", observation.id, unit.id);
                if units
                    .insert(qualified, (&observation.id, &unit.id, unit.label.as_str()))
                    .is_some()
                {
                    return Err(invalid("ambiguous qualified residual unit identity"));
                }
            }
        }
        let residuals = residual_evidence(ctx, "unmatched_units")?;
        for id in residuals.keys() {
            if !units.contains_key(id) {
                return Err(invalid(
                    "residual unit is not present in selected observations",
                ));
            }
        }
        let mut groups = BTreeMap::<&str, Vec<serde_json::Value>>::new();
        let mut support = BTreeMap::<&str, BTreeSet<_>>::new();
        for (id, evidence) in residuals {
            let (observation, unit, label) = &units[&id];
            if label.trim().is_empty() {
                continue;
            }
            support.entry(*label).or_default().insert(*observation);
            groups.entry(*label).or_default().push(
                serde_json::json!({"observation":observation,"unit":unit,"measurements":evidence}),
            );
        }
        let mut candidates = Vec::new();
        for (label, examples) in groups {
            let observations = &support[&label];
            if observations.len() < minimum {
                continue;
            }
            candidates.push(token.emit(CandidateProposal {domain_version_id:ctx.domain_version_id().into(),kind:CandidateKind::AtomicMeaning,
                value:crate::support::json_object(serde_json::json!({"pattern":{"matching":"exact_observed_label","observed_label":label},"observation_count":observations.len(),"observations":observations,"examples":examples}))}));
        }
        Ok(candidates)
    }
}

/// Evidence must come from the explicitly selected baseline domain version.
/// The harness supplies already-calculated residuals and their source observations.
#[derive(Clone, Copy)]
pub struct CandidateInputs<'a> {
    pub domain_version_id: &'a str,
    pub measurements: &'a [Tracked<Measurement>],
    pub observations: &'a [Tracked<Observation>],
    pub structures: &'a [Tracked<unclip_measure::EmpiricalStructure>],
}

/// Generate anonymous proposals only; this never inserts domain units.
pub fn generate_candidates(
    plan: &RunPlan,
    inputs: CandidateInputs<'_>,
    run: crate::MeasurementRun<'_>,
) -> Result<Vec<Calculated<CandidateProposal>>> {
    crate::support::require_run_id("candidate generation", run.id)?;
    for measurement in inputs.measurements {
        crate::require_calculated_evidence(measurement, "candidate input measurement")?;
    }
    for structure in inputs.structures {
        crate::require_calculated_evidence(structure, "candidate input structure")?;
    }
    let mut generators = plan.candidate_generators.iter().collect::<Vec<_>>();
    generators.sort_by_key(|generator| &generator.descriptor().id);
    let mut results = Vec::new();
    let empty = SharedParams::new(serde_json::json!({}));
    for generator in generators {
        let descriptor = generator.descriptor();
        let params = run.params.get(&descriptor.id).unwrap_or(&empty);
        crate::support::require_declared_params(&descriptor.id, descriptor.params_schema, params)?;
        let ctx = CandidateCtx::new(
            inputs.domain_version_id,
            inputs.measurements,
            inputs.observations,
            params,
            DependencyCollector::default(),
        )
        .with_structures(inputs.structures);
        let token = ctx.calculation_token(EmitMetadata::new(
            DerivedId::new(format!("{}/{}", run.id, descriptor.id)),
            descriptor.id.clone(),
            descriptor.version.clone(),
            ctx.shared_params(),
            run.timestamp.clone(),
        ));
        let candidates = generator.generate(&ctx, token)?;
        for candidate in &candidates {
            crate::support::require_producer(&descriptor.id, candidate)?;
        }
        results.extend(candidates);
    }
    Ok(results)
}
