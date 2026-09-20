//! Deterministic proposals from recurring unmatched observed units.
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use unclip_domain::{CandidateKind, CandidateProposal};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Tracked,
};
use unclip_measure::{Measurement, MeasurementValue, Reading};
use unclip_observe::Observation;
use unclip_plugin::{
    CandidateCtx, CandidateGenerator, PluginDescriptor, PluginError, Result, RunPlan,
};

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
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["minimum_observations"],"properties":{"minimum_observations":{"type":"integer","minimum":2}}}"#,
            },
        }
    }
}
fn invalid(message: impl ToString) -> PluginError {
    PluginError::Message(message.to_string())
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
        let params: Parameters = serde_json::from_value(ctx.params().clone()).map_err(invalid)?;
        if params.minimum_observations < 2 || ctx.domain_version_id().is_empty() {
            return Err(invalid(
                "persistent residuals require a domain version and at least two observations",
            ));
        }
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
                let qualified = format!("{}/{}", observation.id.0, unit.id.0);
                if units
                    .insert(
                        qualified,
                        (observation.id.clone(), unit.id.clone(), unit.label.clone()),
                    )
                    .is_some()
                {
                    return Err(invalid("ambiguous qualified residual unit identity"));
                }
            }
        }
        let mut measurements = BTreeSet::new();
        let mut residuals = BTreeMap::<String, BTreeSet<DerivedId>>::new();
        for tracked in ctx.measurements() {
            if !measurements.insert(tracked.id()) {
                return Err(invalid("duplicate discovery measurement"));
            }
            let measurement = ctx.read(tracked);
            if measurement.sensor.0 != "sensor.residual"
                || measurement
                    .context
                    .values
                    .get("residual_kind")
                    .and_then(serde_json::Value::as_str)
                    != Some("unmatched_units")
            {
                continue;
            }
            let Reading::Value { value } = &measurement.reading else {
                continue;
            };
            let MeasurementValue::Structured(value) = value else {
                return Err(invalid("unmatched residual measurement must be structured"));
            };
            let residual: Residual = serde_json::from_value(value.clone()).map_err(invalid)?;
            let unique = residual.ids.iter().collect::<BTreeSet<_>>();
            if residual.count != residual.ids.len() || unique.len() != residual.ids.len() {
                return Err(invalid(
                    "residual count and unique unit identities must agree",
                ));
            }
            for id in residual.ids {
                if !units.contains_key(&id) {
                    return Err(invalid(
                        "residual unit is not present in selected observations",
                    ));
                }
                residuals
                    .entry(id)
                    .or_default()
                    .insert(tracked.id().clone());
            }
        }
        let mut groups = BTreeMap::<String, Vec<serde_json::Value>>::new();
        let mut support = BTreeMap::<String, BTreeSet<_>>::new();
        for (id, evidence) in residuals {
            let (observation, unit, label) = &units[&id];
            if label.trim().is_empty() {
                continue;
            }
            support
                .entry(label.clone())
                .or_default()
                .insert(observation.clone());
            groups.entry(label.clone()).or_default().push(
                serde_json::json!({"observation":observation,"unit":unit,"measurements":evidence}),
            );
        }
        let mut candidates = Vec::new();
        for (label, examples) in groups {
            let observations = &support[&label];
            if observations.len() < params.minimum_observations {
                continue;
            }
            candidates.push(token.emit(CandidateProposal {domain_version_id:ctx.domain_version_id().into(),kind:CandidateKind::AtomicMeaning,
                value:serde_json::json!({"pattern":{"matching":"exact_observed_label","observed_label":label},"observation_count":observations.len(),"observations":observations,"examples":examples}).as_object().expect("object").clone()}));
        }
        Ok(candidates)
    }
}

/// Evidence must come from the explicitly selected baseline domain version.
/// The harness supplies already-calculated residuals and their source observations.
pub struct CandidateInputs<'a> {
    pub domain_version_id: &'a str,
    pub measurements: &'a [Tracked<Measurement>],
    pub observations: &'a [Tracked<Observation>],
}
impl super::Engine {
    /// Generate anonymous proposals only; this never inserts domain units.
    pub fn generate_candidates(
        &self,
        plan: &RunPlan,
        inputs: CandidateInputs<'_>,
        run: super::MeasurementRun<'_>,
    ) -> Result<Vec<Calculated<CandidateProposal>>> {
        let mut generators = plan.candidate_generators.iter().collect::<Vec<_>>();
        generators.sort_by_key(|generator| &generator.descriptor().id);
        let mut results = Vec::new();
        let empty = serde_json::json!({});
        for generator in generators {
            let descriptor = generator.descriptor();
            let params = run.params.get(&descriptor.id).unwrap_or(&empty);
            let ctx = CandidateCtx::new(
                inputs.domain_version_id,
                inputs.measurements,
                inputs.observations,
                params,
                DependencyCollector::default(),
            );
            let token = ctx.calculation_token(EmitMetadata {
                id: DerivedId::new(format!("{}/{}", run.id, descriptor.id)),
                producer: descriptor.id.clone(),
                algorithm: descriptor.id.0.clone(),
                version: descriptor.version.clone(),
                params: params.clone(),
                params_hash: hash_params(params),
                source: None,
                timestamp: run.timestamp.clone(),
                domain_version: None,
                frame_version: None,
                model: None,
            });
            results.extend(generator.generate(&ctx, token)?);
        }
        Ok(results)
    }
}
