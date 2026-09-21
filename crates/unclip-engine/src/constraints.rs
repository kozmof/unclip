//! Explicit evidence requirements; no weighted score or candidate acceptance decision.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Timestamp, Tracked,
};
use unclip_measure::Measurement;
use unclip_plugin::{PluginError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExperimentConstraint {
    MinimumSamples {
        measurement: DerivedId,
        minimum: usize,
    },
    /// Budgets count edits recorded by candidate application, not semantic complexity.
    ComplexityBudget {
        maximum_added_units: usize,
        maximum_added_relations: usize,
        maximum_property_changes: usize,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintStatus {
    Satisfied,
    Violated,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConstraintAssessment {
    pub constraint: ExperimentConstraint,
    pub status: ConstraintStatus,
    pub observed: BTreeMap<String, usize>,
}

impl super::Engine {
    /// Assess only explicit requirements. Missing sample counts are unavailable,
    /// never zero or satisfied. Sample counts do not establish reading availability.
    pub fn assess_experiment_constraints(
        &self,
        constraints: &[ExperimentConstraint],
        measurements: &[Tracked<Measurement>],
        application: &Tracked<super::CounterfactualSnapshot>,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<Vec<ConstraintAssessment>>> {
        let invalid = |message: &str| PluginError::Message(message.into());
        if run_id.trim().is_empty() || constraints.is_empty() {
            return Err(invalid(
                "constraint assessment requires a run identity and explicit requirements",
            ));
        }
        let dependencies = DependencyCollector::default();
        let output = DerivedId::new(format!("{run_id}/constraints"));
        let mut selected = BTreeMap::new();
        for input in measurements {
            if input.id().0.trim().is_empty()
                || input.id() == &output
                || input.id() == application.id()
                || selected
                    .insert(input.id().clone(), dependencies.read(input))
                    .is_some()
            {
                return Err(invalid("constraint measurements require unique nonempty identities distinct from application and output"));
            }
        }
        if application.id().0.trim().is_empty() || application.id() == &output {
            return Err(invalid(
                "application identity must be nonempty and distinct from constraint output",
            ));
        }
        let mut sample_requirements = BTreeSet::new();
        let mut complexity_seen = false;
        let mut assessments = Vec::new();
        for constraint in constraints {
            let (status, observed) = match constraint {
                ExperimentConstraint::MinimumSamples {
                    measurement,
                    minimum,
                } => {
                    if *minimum == 0 || !sample_requirements.insert(measurement) {
                        return Err(invalid(
                            "sample floors must be positive and unique per measurement",
                        ));
                    }
                    let input = selected.get(measurement).ok_or_else(|| {
                        invalid("sample requirement references an unselected measurement")
                    })?;
                    match input.sample_count {
                        Some(count) => (
                            if count >= *minimum {
                                ConstraintStatus::Satisfied
                            } else {
                                ConstraintStatus::Violated
                            },
                            BTreeMap::from([("sample_count".into(), count)]),
                        ),
                        None => (ConstraintStatus::Unavailable, BTreeMap::new()),
                    }
                }
                ExperimentConstraint::ComplexityBudget {
                    maximum_added_units,
                    maximum_added_relations,
                    maximum_property_changes,
                } => {
                    if complexity_seen {
                        return Err(invalid("only one explicit complexity budget is permitted"));
                    }
                    complexity_seen = true;
                    let value = dependencies.read(application);
                    let observed = BTreeMap::from([
                        ("added_units".into(), value.added_units.len()),
                        ("added_relations".into(), value.added_relations.len()),
                        ("property_changes".into(), value.property_changes.len()),
                    ]);
                    let satisfied = value.added_units.len() <= *maximum_added_units
                        && value.added_relations.len() <= *maximum_added_relations
                        && value.property_changes.len() <= *maximum_property_changes;
                    (
                        if satisfied {
                            ConstraintStatus::Satisfied
                        } else {
                            ConstraintStatus::Violated
                        },
                        observed,
                    )
                }
            };
            assessments.push(ConstraintAssessment {
                constraint: constraint.clone(),
                status,
                observed,
            });
        }
        let params = serde_json::json!({"constraints":constraints});
        let token = CalculationToken::from_harness(
            EmitMetadata {
                id: output,
                producer: PluginId::new("experiment.constraints"),
                algorithm: "explicit_sample_and_edit_budgets".into(),
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
        Ok(token.emit(assessments))
    }
}
