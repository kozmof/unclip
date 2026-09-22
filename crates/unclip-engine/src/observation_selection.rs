//! Explicit observation splits retained in planned-run metadata for replay.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Timestamp, Tracked,
};
use unclip_observe::{Observation, ObservationId};
use unclip_plugin::{PluginError, Result, RunPlan};
use unclip_store::{EngineRunRecord, RecordedInference};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationSplit {
    pub training: Vec<RecordedInference<Observation>>,
    pub held_out: Vec<RecordedInference<Observation>>,
}

fn invalid(message: &str) -> PluginError {
    PluginError::Message(message.into())
}

impl super::Engine {
    /// Reject a candidate whose transitive provenance reaches held-out observations
    /// or their selected inference products. Shared provenance across training and
    /// held-out entries is conservatively treated as leakage.
    pub fn validate_candidate_ancestry(
        &self,
        candidate: &DerivedId,
        candidate_ancestors: &[DerivedId],
        split: &ObservationSplit,
        held_out_inference_products: &[DerivedId],
    ) -> Result<()> {
        if candidate.0.trim().is_empty() || split.held_out.is_empty() {
            return Err(invalid(
                "candidate leakage validation requires candidate and held-out identities",
            ));
        }
        let mut ancestry = BTreeSet::new();
        for id in candidate_ancestors {
            if id.0.trim().is_empty() || id == candidate || !ancestry.insert(id) {
                return Err(invalid(
                    "candidate ancestry requires unique nonempty acyclic identities",
                ));
            }
        }
        let mut held_out = split
            .held_out
            .iter()
            .map(|entry| &entry.provenance)
            .collect::<BTreeSet<_>>();
        for id in held_out_inference_products {
            if id.0.trim().is_empty() || id == candidate {
                return Err(invalid(
                    "held-out inference products require nonempty identities distinct from the candidate",
                ));
            }
            held_out.insert(id);
        }
        let leaked = ancestry
            .intersection(&held_out)
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>();
        if leaked.is_empty() {
            Ok(())
        } else {
            Err(invalid(&format!(
                "candidate provenance depends on held-out evidence: {}",
                leaked.join(", ")
            )))
        }
    }

    /// Select by observation identity, never by input ordering, labels or inferred time.
    /// Each split retains the caller's order and immutable input values for replay.
    /// This checks split disjointness; it does not certify candidate ancestry is leak-free.
    pub fn select_observations(
        &self,
        observations: &[Tracked<Observation>],
        training: &[ObservationId],
        held_out: &[ObservationId],
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<ObservationSplit>> {
        if run_id.trim().is_empty() || held_out.is_empty() {
            return Err(invalid(
                "observation selection requires a run identity and held-out observations",
            ));
        }
        let output_id = DerivedId::new(format!("{run_id}/observation-split"));
        let dependencies = DependencyCollector::default();
        let mut available = BTreeMap::new();
        for input in observations {
            if input.id().0.trim().is_empty() || input.id() == &output_id {
                return Err(invalid(
                    "observation provenance must be nonempty and distinct from the output",
                ));
            }
            // Pool membership and duplicate detection are also actual reads.
            let value = dependencies.read(input);
            if value.id.0.trim().is_empty() || available.insert(value.id.clone(), input).is_some() {
                return Err(invalid(
                    "available observations require unique nonempty observation identities",
                ));
            }
        }
        let mut selected = BTreeSet::new();
        let mut select = |ids: &[ObservationId]| -> Result<Vec<RecordedInference<Observation>>> {
            ids.iter()
                .map(|id| {
                    if !selected.insert(id.clone()) {
                        return Err(invalid(
                            "training and held-out observations must be unique and disjoint",
                        ));
                    }
                    let input = available.get(id).ok_or_else(|| {
                        invalid("selected observation is absent from the input pool")
                    })?;
                    Ok(RecordedInference {
                        provenance: input.id().clone(),
                        value: dependencies.read(input).clone(),
                    })
                })
                .collect()
        };
        let value = ObservationSplit {
            training: select(training)?,
            held_out: select(held_out)?,
        };
        let params = serde_json::json!({"training":training,"held_out":held_out});
        let token = CalculationToken::from_harness(
            EmitMetadata {
                id: output_id,
                producer: PluginId::new("experiment.select-observations"),
                algorithm: "explicit_observation_split".into(),
                version: semver::Version::new(0, 1, 0),
                params_hash: hash_params(&params),
                params,
                source: None,
                timestamp,
                domain_version: None,
                frame_version: None,
                model: None,
            },
            dependencies,
        );
        Ok(token.emit(value))
    }

    /// Build a planned run containing exact split values, identities and selection provenance.
    /// The existing EngineRunRepository persists this metadata before execution.
    pub fn observation_split_run_record(
        &self,
        plan: &RunPlan,
        split: &Calculated<ObservationSplit>,
        run: super::MeasurementRun<'_>,
        metadata: serde_json::Value,
    ) -> Result<EngineRunRecord> {
        if split.id().0 != format!("{}/observation-split", run.id) {
            return Err(invalid("observation split belongs to a different run"));
        }
        Ok(self.run_record(
            plan,
            run.params,
            run.id,
            run.timestamp,
            serde_json::json!({
                "request": metadata,
                "observation_split": {
                    "id": split.id(),
                    "provenance": split.provenance(),
                    "value": split.value(),
                },
            }),
        ))
    }
}
