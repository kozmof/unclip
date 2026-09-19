//! Explicit conversions into the extensible empirical-structure envelope.
//! These conversions add no interpretation or semantic labels. Provenance is
//! attached by the calculation harness before persistence, not by conversion.

use crate::{
    ChangePointDetection, CommunityDetection, EmpiricalStructure, RegimePartition,
    SpectralDecomposition,
};

macro_rules! empirical_payload {
    ($payload:ty, $kind:literal) => {
        impl TryFrom<$payload> for EmpiricalStructure {
            type Error = serde_json::Error;

            fn try_from(payload: $payload) -> Result<Self, Self::Error> {
                let value = serde_json::to_value(payload)?;
                // serde_json represents non-finite floats as null. Reject that
                // lossy conversion rather than storing a corrupted typed result.
                serde_json::from_value::<$payload>(value.clone())?;
                Ok(Self {
                    kind: $kind.into(),
                    value,
                })
            }
        }

        impl TryFrom<&EmpiricalStructure> for $payload {
            type Error = serde_json::Error;

            fn try_from(structure: &EmpiricalStructure) -> Result<Self, Self::Error> {
                if structure.kind != $kind {
                    return Err(<serde_json::Error as serde::de::Error>::custom(concat!(
                        "expected empirical structure kind ",
                        $kind
                    )));
                }
                serde_json::from_value(structure.value.clone())
            }
        }
    };
}

empirical_payload!(CommunityDetection, "communities");
empirical_payload!(SpectralDecomposition, "spectral");
empirical_payload!(ChangePointDetection, "change_points");
empirical_payload!(RegimePartition, "regimes");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Eigenpair, PairwiseMetric};

    #[test]
    fn conversion_rejects_nonfinite_values_and_mismatched_or_unknown_payloads() {
        let mut spectral = SpectralDecomposition {
            metric: PairwiseMetric::Spearman,
            units: vec![],
            eigenpairs: vec![Eigenpair {
                eigenvalue: f64::NAN,
                loadings: vec![],
            }],
            minimum_cell_samples: 2,
            tolerance: 1e-12,
            sweeps: 0,
        };
        assert!(EmpiricalStructure::try_from(spectral.clone()).is_err());
        spectral.eigenpairs[0].eigenvalue = 1.0;
        let mut structure = EmpiricalStructure::try_from(spectral).unwrap();
        assert!(CommunityDetection::try_from(&structure).is_err());
        structure.value["label"] = serde_json::json!("invented semantic meaning");
        assert!(SpectralDecomposition::try_from(&structure).is_err());
        assert!(ChangePointDetection::try_from(&structure).is_err());
    }
}
