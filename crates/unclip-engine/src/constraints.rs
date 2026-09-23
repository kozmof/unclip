//! Explicit evidence requirements; no weighted score or candidate acceptance decision.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Timestamp, Tracked,
};
use unclip_measure::Measurement;
use unclip_plugin::{PluginError, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExperimentConstraint {
    ScalarTransfer {
        source: DerivedId,
        target: DerivedId,
        minimum_samples: usize,
        maximum_absolute_difference: f64,
    },
    ConditionalDependency {
        measurement: DerivedId,
        left: unclip_domain::UnitId,
        right: unclip_domain::UnitId,
        conditioning: unclip_domain::UnitId,
        minimum_samples: usize,
        minimum_information: f64,
    },
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintStatus {
    Satisfied,
    Violated,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConstraintAssessment {
    pub constraint: ExperimentConstraint,
    pub status: ConstraintStatus,
    pub observed: BTreeMap<String, usize>,
    pub reading: Option<unclip_measure::Reading>,
    pub transfer: Option<super::TransferAssessment>,
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
            super::require_calculated_evidence(input, "constraint input measurement")?;
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
        let mut conditional_seen = BTreeSet::new();
        let mut transfer_seen = BTreeSet::new();
        let mut assessments = Vec::new();
        for constraint in constraints {
            let mut reading = None;
            let mut transfer = None;
            let (status, observed) = match constraint {
                ExperimentConstraint::ScalarTransfer {
                    source,
                    target,
                    minimum_samples,
                    maximum_absolute_difference,
                } => {
                    if source == target || !transfer_seen.insert((source, target)) {
                        return Err(invalid(
                            "transfer requires distinct identities and unique directed pairs",
                        ));
                    }
                    let source = selected
                        .get(source)
                        .ok_or_else(|| invalid("transfer source is not selected"))?;
                    let target = selected
                        .get(target)
                        .ok_or_else(|| invalid("transfer target is not selected"))?;
                    let (status, result) = super::transfer_constraint::assess(
                        source,
                        target,
                        *minimum_samples,
                        *maximum_absolute_difference,
                    )?;
                    let mut observed = BTreeMap::new();
                    if let Some(count) = source.sample_count {
                        observed.insert("source_samples".into(), count);
                    }
                    if let Some(count) = target.sample_count {
                        observed.insert("target_samples".into(), count);
                    }
                    transfer = Some(result);
                    (status, observed)
                }
                ExperimentConstraint::ConditionalDependency {
                    measurement,
                    left,
                    right,
                    conditioning,
                    minimum_samples,
                    minimum_information,
                } => {
                    if !conditional_seen.insert(measurement)
                        || *minimum_samples < 2
                        || !minimum_information.is_finite()
                        || *minimum_information < 0.0
                        || left.0.trim().is_empty()
                        || right.0.trim().is_empty()
                        || conditioning.0.trim().is_empty()
                        || left == right
                        || conditioning == left
                        || conditioning == right
                    {
                        return Err(invalid("conditional requirements need distinct units, a finite nonnegative threshold and at least two samples"));
                    }
                    let input = selected.get(measurement).ok_or_else(|| {
                        invalid("conditional requirement references an unselected measurement")
                    })?;
                    if input.sensor.0 != "sensor.conditional-mutual-information" {
                        return Err(invalid("conditional dependency requires conditional mutual information evidence"));
                    }
                    reading = Some(input.reading.clone());
                    let observed = input
                        .sample_count
                        .map(|count| BTreeMap::from([("sample_count".into(), count)]))
                        .unwrap_or_default();
                    let status = match &input.reading {
                        unclip_measure::Reading::Value {
                            value: unclip_measure::MeasurementValue::Scalar(value),
                        } => {
                            if !value.is_finite()
                                || *value < 0.0
                                || input.context.values.get("pair")
                                    != Some(&serde_json::json!([left, right]))
                                || input.context.values.get("conditioning_variables")
                                    != Some(&serde_json::json!([conditioning]))
                            {
                                return Err(invalid("conditional evidence value or selected variable context does not match the requirement"));
                            }
                            match input.sample_count {
                                None => ConstraintStatus::Unavailable,
                                Some(count)
                                    if count < *minimum_samples
                                        || *value < *minimum_information =>
                                {
                                    ConstraintStatus::Violated
                                }
                                Some(_) => ConstraintStatus::Satisfied,
                            }
                        }
                        unclip_measure::Reading::Value { .. } => {
                            return Err(invalid("conditional evidence must be scalar"))
                        }
                        _ => ConstraintStatus::Unavailable,
                    };
                    (status, observed)
                }
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
                reading,
                transfer,
            });
        }
        let params = serde_json::json!({"constraints":constraints});
        let token = CalculationToken::from_harness(
            EmitMetadata {
                id: output,
                producer: PluginId::new("experiment.constraints"),
                algorithm: "explicit_evidence_constraints".into(),
                version: semver::Version::new(0, 3, 0),
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
