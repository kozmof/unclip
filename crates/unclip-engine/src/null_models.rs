//! Fixed-margin random co-occurrence explanation for observed relation proposals.
use serde::Deserialize;
use std::collections::BTreeSet;
use unclip_domain::{CandidateKind, CandidateProposal};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Tracked,
};
use unclip_measure::{MeasurementValue, Reading};
use unclip_observe::Observation;
use unclip_plugin::{NullCtx, NullModel, PluginDescriptor, PluginError, Result, RunPlan};

pub struct RandomCooccurrenceNull {
    descriptor: PluginDescriptor,
}
impl Default for RandomCooccurrenceNull {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("null.random-cooccurrence"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["minimum_observations"],"properties":{"minimum_observations":{"type":"integer","minimum":2}}}"#,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    minimum_observations: usize,
}
fn invalid(message: &str) -> PluginError {
    PluginError::Message(message.into())
}

// Relative hypergeometric masses expanded from the mode avoid factorial overflow
// and underflow of the initial mass in large, concentrated distributions.
pub(super) fn overlap_tail(n: usize, a: usize, b: usize, observed: usize) -> f64 {
    let low = a.saturating_sub(n - b);
    let high = a.min(b);
    let mode = ((((a as f64) + 1.0) * ((b as f64) + 1.0) / ((n as f64) + 2.0)).floor() as usize)
        .clamp(low, high);
    let mut total = 1.0;
    let mut tail = if mode >= observed { 1.0 } else { 0.0 };
    let mut weight = 1.0;
    for k in mode..high {
        weight *=
            ((a - k) as f64 / (k + 1) as f64) * ((b - k) as f64 / ((n - b) - (a - k) + 1) as f64);
        total += weight;
        if k + 1 >= observed {
            tail += weight;
        }
    }
    weight = 1.0;
    for k in (low + 1..=mode).rev() {
        weight *=
            (k as f64 / (a - k + 1) as f64) * (((n - b) - (a - k)) as f64 / (b - k + 1) as f64);
        total += weight;
        if k > observed {
            tail += weight;
        }
    }
    (tail / total).clamp(0.0, 1.0)
}
impl NullModel for RandomCooccurrenceNull {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }
    fn evaluate(&self, ctx: &NullCtx<'_>, token: CalculationToken) -> Result<Calculated<Reading>> {
        let params: Parameters = serde_json::from_value(ctx.params().clone())
            .map_err(|e| PluginError::Message(e.to_string()))?;
        if params.minimum_observations < 2 {
            return Err(invalid("co-occurrence requires at least two observations"));
        }
        let candidate = ctx.candidate();
        let pattern = candidate.value.get("pattern");
        if candidate.kind != CandidateKind::Relation
            || pattern
                .and_then(|p| p.get("matching"))
                .and_then(|v| v.as_str())
                != Some("exact_directed_observed_relation")
        {
            return Ok(token.emit(Reading::NotApplicable {
                reason: "requires an exact observed-label relation proposal".into(),
            }));
        }
        let pattern = pattern.unwrap();
        let left = pattern
            .get("source_label")
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| invalid("missing source label"))?;
        let right = pattern
            .get("target_label")
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| invalid("missing target label"))?;
        if left == right {
            return Ok(token.emit(Reading::NotApplicable {
                reason: "same-label endpoints need a multiplicity null model".into(),
            }));
        }
        let mut inputs = ctx.observations().iter().collect::<Vec<_>>();
        inputs.sort_by_key(|input| input.id());
        let mut ids = BTreeSet::new();
        let mut observations = BTreeSet::new();
        let (mut a, mut b, mut overlap) = (0, 0, 0);
        for input in inputs {
            let observation = ctx.read(input);
            if !ids.insert(input.id())
                || observation.id.0.is_empty()
                || !observations.insert(&observation.id)
            {
                return Err(invalid("duplicate or empty null observation identity"));
            }
            let has_left = observation.units.iter().any(|unit| unit.label == left);
            let has_right = observation.units.iter().any(|unit| unit.label == right);
            a += usize::from(has_left);
            b += usize::from(has_right);
            overlap += usize::from(has_left && has_right);
        }
        let n = observations.len();
        if n < params.minimum_observations {
            return Ok(token.emit(Reading::InsufficientEvidence {
                have: n,
                need: params.minimum_observations,
            }));
        }
        Ok(token.emit(Reading::Value { value: MeasurementValue::Structured(serde_json::json!({
            "model":"fixed_margin_exchangeable_observations", "matching":"exact_observed_label_presence",
            "source_label":left,"target_label":right,"observations":observations,
            "sample_count":n,"source_count":a,"target_count":b,"observed_overlap":overlap,
            "expected_overlap":(a as f64)*(b as f64)/(n as f64),"upper_tail_probability":overlap_tail(n,a,b,overlap),
            "scope":"endpoint co-presence only; does not explain relation direction or kind",
            "assumption":"selected observations are exchangeable; source, time, genre and extraction bias are not controlled",
            "selection_adjusted":false,"causal_claim":false
        })) }))
    }
}
/// Explicit evidence available to configured null models.
#[derive(Default)]
pub struct NullInputs<'a> {
    pub observations: &'a [Tracked<Observation>],
    pub rankings: &'a [Tracked<unclip_observe::PartialRanking>],
    pub domain: Option<&'a Tracked<unclip_domain::DomainSnapshot>>,
}
impl super::Engine {
    /// Evaluate explicitly selected nulls on the caller's evidence split.
    pub fn evaluate_null_models(
        &self,
        plan: &RunPlan,
        candidate: &Tracked<CandidateProposal>,
        observations: &[Tracked<Observation>],
        run: super::MeasurementRun<'_>,
    ) -> Result<Vec<Calculated<Reading>>> {
        self.evaluate_null_models_with_rankings(plan, candidate, observations, &[], run)
    }
    pub fn evaluate_null_models_with_rankings(
        &self,
        plan: &RunPlan,
        candidate: &Tracked<CandidateProposal>,
        observations: &[Tracked<Observation>],
        rankings: &[Tracked<unclip_observe::PartialRanking>],
        run: super::MeasurementRun<'_>,
    ) -> Result<Vec<Calculated<Reading>>> {
        self.evaluate_null_models_with_inputs(
            plan,
            candidate,
            NullInputs {
                observations,
                rankings,
                domain: None,
            },
            run,
        )
    }
    pub fn evaluate_null_models_with_inputs(
        &self,
        plan: &RunPlan,
        candidate: &Tracked<CandidateProposal>,
        inputs: NullInputs<'_>,
        run: super::MeasurementRun<'_>,
    ) -> Result<Vec<Calculated<Reading>>> {
        let mut models = plan.null_models.iter().collect::<Vec<_>>();
        models.sort_by_key(|model| &model.descriptor().id);
        let mut results = Vec::new();
        let empty = serde_json::json!({});
        for model in models {
            let descriptor = model.descriptor();
            let params = run.params.get(&descriptor.id).unwrap_or(&empty);
            let ctx = NullCtx::new(
                candidate,
                inputs.observations,
                params,
                DependencyCollector::default(),
            )
            .with_rankings(inputs.rankings)
            .with_domain(inputs.domain);
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
            results.push(model.evaluate(&ctx, token)?);
        }
        Ok(results)
    }
}
#[cfg(test)]
mod tests {
    use super::overlap_tail;
    #[test]
    fn matches_enumeration_of_all_small_fixed_margin_assignments() {
        for n in 1usize..=8 {
            for a in 0..=n {
                for b in 0..=n {
                    let assignments = (0u32..(1 << n))
                        .filter(|bits| bits.count_ones() as usize == b)
                        .collect::<Vec<_>>();
                    let left = (1u32 << a) - 1;
                    for observed in a.saturating_sub(n - b)..=a.min(b) {
                        let expected = assignments
                            .iter()
                            .filter(|bits| (**bits & left).count_ones() as usize >= observed)
                            .count() as f64
                            / assignments.len() as f64;
                        assert!(
                            (overlap_tail(n, a, b, observed) - expected).abs() < 1e-12,
                            "n={n}, a={a}, b={b}, overlap={observed}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn exact_small_population_and_degenerate_margins() {
        assert!((overlap_tail(4, 2, 2, 2) - 1.0 / 6.0).abs() < 1e-14);
        assert!((overlap_tail(4, 2, 2, 1) - 5.0 / 6.0).abs() < 1e-14);
        assert_eq!(overlap_tail(4, 2, 2, 0), 1.0);
        assert_eq!(overlap_tail(4, 0, 2, 0), 1.0);
        assert_eq!(overlap_tail(4, 4, 2, 2), 1.0);
        assert!((overlap_tail(10000, 5000, 5000, 2500) - 0.5).abs() < 0.02);
    }
}
