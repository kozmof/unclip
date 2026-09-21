//! Pareto ordering on explicit, comparable scalar measurement dimensions.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Timestamp, Tracked,
};
use unclip_measure::{Measurement, MeasurementValue, Reading};
use unclip_plugin::{PluginError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectiveDirection {
    Minimize,
    Maximize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParetoDimension {
    pub name: String,
    pub left: DerivedId,
    pub right: DerivedId,
    pub direction: ObjectiveDirection,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParetoRelation {
    LeftDominates,
    RightDominates,
    Equal,
    Tradeoff,
    Incomparable,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParetoEvidence {
    pub dimension: ParetoDimension,
    pub left: Reading,
    pub right: Reading,
    pub relation: ParetoRelation,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParetoAssessment {
    pub relation: ParetoRelation,
    pub dimensions: Vec<ParetoEvidence>,
}

impl super::Engine {
    /// All selected dimensions must be comparable. No dimensions are inferred,
    /// dropped, weighted, or reduced to an aggregate numerical score.
    pub fn compare_pareto(
        &self,
        measurements: &[Tracked<Measurement>],
        dimensions: &[ParetoDimension],
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<ParetoAssessment>> {
        let invalid = |message: &str| PluginError::Message(message.into());
        if dimensions.is_empty() || run_id.trim().is_empty() {
            return Err(invalid(
                "Pareto comparison requires explicit dimensions and a run identity",
            ));
        }
        let output = DerivedId::new(format!("{run_id}/pareto"));
        let dependencies = DependencyCollector::default();
        let mut selected = BTreeMap::new();
        for input in measurements {
            if input.id().0.trim().is_empty()
                || input.id() == &output
                || selected
                    .insert(input.id().clone(), dependencies.read(input))
                    .is_some()
            {
                return Err(invalid("Pareto measurements require unique nonempty identities distinct from the output"));
            }
        }
        let mut dimensions = dimensions.to_vec();
        dimensions.sort_by(|a, b| a.name.cmp(&b.name));
        let mut names = BTreeSet::new();
        let mut left_ids = BTreeSet::new();
        let mut right_ids = BTreeSet::new();
        let mut evidence = Vec::new();
        let (mut left_better, mut right_better, mut unavailable) = (false, false, false);
        for dimension in &dimensions {
            if dimension.name.trim().is_empty()
                || !names.insert(&dimension.name)
                || !left_ids.insert(&dimension.left)
                || !right_ids.insert(&dimension.right)
            {
                return Err(invalid(
                    "Pareto dimensions require unique names and one-to-one measurement pairings",
                ));
            }
            let left = selected
                .get(&dimension.left)
                .ok_or_else(|| invalid("Pareto left measurement is not selected"))?;
            let right = selected
                .get(&dimension.right)
                .ok_or_else(|| invalid("Pareto right measurement is not selected"))?;
            let compatible = left.sensor == right.sensor
                && left.sensor_version == right.sensor_version
                && left.context == right.context;
            let relation = match (&left.reading, &right.reading) {
                (
                    Reading::Value {
                        value: MeasurementValue::Scalar(a),
                    },
                    Reading::Value {
                        value: MeasurementValue::Scalar(b),
                    },
                ) if compatible => {
                    if !a.is_finite() || !b.is_finite() {
                        return Err(invalid("Pareto scalar evidence must be finite"));
                    }
                    let ordering = if dimension.direction == ObjectiveDirection::Minimize {
                        b.partial_cmp(a)
                    } else {
                        a.partial_cmp(b)
                    };
                    match ordering {
                        Some(std::cmp::Ordering::Greater) => {
                            left_better = true;
                            ParetoRelation::LeftDominates
                        }
                        Some(std::cmp::Ordering::Less) => {
                            right_better = true;
                            ParetoRelation::RightDominates
                        }
                        _ => ParetoRelation::Equal,
                    }
                }
                _ => {
                    unavailable = true;
                    ParetoRelation::Incomparable
                }
            };
            evidence.push(ParetoEvidence {
                dimension: dimension.clone(),
                left: left.reading.clone(),
                right: right.reading.clone(),
                relation,
            });
        }
        let relation = if unavailable {
            ParetoRelation::Incomparable
        } else {
            match (left_better, right_better) {
                (true, false) => ParetoRelation::LeftDominates,
                (false, true) => ParetoRelation::RightDominates,
                (true, true) => ParetoRelation::Tradeoff,
                _ => ParetoRelation::Equal,
            }
        };
        let params = serde_json::json!({"dimensions":dimensions});
        let token = CalculationToken::from_harness(
            EmitMetadata {
                id: output,
                producer: PluginId::new("compare.pareto"),
                algorithm: "explicit_scalar_pareto".into(),
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
        Ok(token.emit(ParetoAssessment {
            relation,
            dimensions: evidence,
        }))
    }
}
