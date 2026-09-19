//! Explicit derivation of anonymous structures from recorded matrix measurements.

use std::{collections::BTreeSet, num::NonZeroUsize};

use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Timestamp, Tracked,
};
use unclip_measure::{
    detect_communities, spectral_decomposition, EmpiricalStructure, Measurement, MeasurementValue,
    Reading,
};
use unclip_plugin::{PluginError, Result};

/// Each selected measurement is processed independently. Metrics and profiles
/// are never averaged, and no semantic labels are generated.
#[derive(Debug, Clone, Copy)]
pub enum EmpiricalMethod {
    Communities {
        threshold: f64,
        minimum_samples: NonZeroUsize,
    },
    Spectral {
        minimum_samples: NonZeroUsize,
        tolerance: f64,
        max_sweeps: NonZeroUsize,
    },
}

/// Absence of a structure is explicit and retains the source measurement ID.
#[derive(Debug)]
pub struct EmpiricalResult {
    pub measurement: DerivedId,
    /// `None` means the selected reading or matrix lacks usable evidence.
    /// The original evidence states remain available through `measurement`.
    pub structure: Option<Calculated<EmpiricalStructure>>,
}

impl super::Engine {
    /// Derive structures from recorded measurements selected across profiles.
    /// Input IDs must be unique; output order is canonical by source ID.
    /// Replaying with the same run ID, timestamp, method, and inputs produces
    /// identical values and provenance. Persist successful outputs with
    /// `MeasurementRepository::insert_calculated_structure`.
    pub fn derive_empirical(
        &self,
        measurements: &[Tracked<Measurement>],
        method: EmpiricalMethod,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Vec<EmpiricalResult>> {
        let invalid = |message: &str| PluginError::Message(message.into());
        if run_id.is_empty() || measurements.is_empty() {
            return Err(invalid(
                "empirical derivation requires a run ID and measurements",
            ));
        }
        let (algorithm, params) = match method {
            EmpiricalMethod::Communities {
                threshold,
                minimum_samples,
            } => {
                if !threshold.is_finite() {
                    return Err(invalid("community threshold must be finite"));
                }
                (
                    "empirical.communities",
                    serde_json::json!({"threshold":threshold,"minimum_samples":minimum_samples}),
                )
            }
            EmpiricalMethod::Spectral {
                minimum_samples,
                tolerance,
                max_sweeps,
            } => {
                if !tolerance.is_finite() || tolerance <= 0.0 || tolerance >= 1.0 {
                    return Err(invalid(
                        "spectral tolerance must be strictly between zero and one",
                    ));
                }
                (
                    "empirical.spectral",
                    serde_json::json!({"minimum_samples":minimum_samples,"tolerance":tolerance,"max_sweeps":max_sweeps}),
                )
            }
        };
        let mut seen = BTreeSet::new();
        for measurement in measurements {
            if measurement.id().0.is_empty() {
                return Err(invalid("empirical input measurement ID must not be empty"));
            }
            if !seen.insert(measurement.id()) {
                return Err(invalid("duplicate empirical input measurement"));
            }
        }
        let mut selected = measurements.iter().collect::<Vec<_>>();
        selected.sort_by_key(|measurement| measurement.id());
        selected
            .into_iter()
            .map(|input| {
                let dependencies = DependencyCollector::default();
                let measurement = dependencies.read(input);
                let structure = match &measurement.reading {
                    Reading::Value {
                        value: MeasurementValue::PairwiseMatrix(matrix),
                    } => match method {
                        EmpiricalMethod::Communities {
                            threshold,
                            minimum_samples,
                        } => detect_communities(matrix, threshold, minimum_samples)
                            .map_err(|error| invalid(&error.to_string()))?
                            .map(EmpiricalStructure::try_from)
                            .transpose(),
                        EmpiricalMethod::Spectral {
                            minimum_samples,
                            tolerance,
                            max_sweeps,
                        } => spectral_decomposition(matrix, minimum_samples, tolerance, max_sweeps)
                            .map_err(|error| invalid(&error.to_string()))?
                            .map(EmpiricalStructure::try_from)
                            .transpose(),
                    }
                    .map_err(|error| invalid(&error.to_string()))?,
                    Reading::Value { .. } => {
                        return Err(invalid(
                            "empirical derivation requires typed pairwise matrices",
                        ))
                    }
                    _ => None,
                };
                let structure = structure.map(|value| {
                    // JSON encodes the source ID without delimiter collisions.
                    let source_key =
                        serde_json::to_string(&input.id().0).expect("string serialization");
                    CalculationToken::from_harness(
                        EmitMetadata {
                            id: DerivedId::new(format!("{run_id}/{algorithm}/{source_key}")),
                            producer: PluginId::new(algorithm),
                            algorithm: algorithm.into(),
                            version: semver::Version::new(0, 1, 0),
                            params: params.clone(),
                            params_hash: hash_params(&params),
                            source: None,
                            timestamp: timestamp.clone(),
                            domain_version: None,
                            frame_version: None,
                            model: None,
                        },
                        dependencies,
                    )
                    .emit(value)
                });
                Ok(EmpiricalResult {
                    measurement: input.id().clone(),
                    structure,
                })
            })
            .collect()
    }
}
