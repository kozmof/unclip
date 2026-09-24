use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    detect_cross_domain_communities, CrossDomainCommunityConfig, CrossDomainCommunityOutcome,
    Measurement, MeasurementContext, MeasurementKind, MeasurementValue, ProductMeasurementBinding,
    Reading,
};
use unclip_plugin::{
    Capability, EvidenceRequirement, PluginError, ProductMeasureCtx, ProductSensor, Result,
    SensorDescriptor,
};

const APPLICABILITY: &[Capability] = &[Capability::ProductDomain];
const EVIDENCE: &[EvidenceRequirement] = &[EvidenceRequirement::MinSamples(2)];
const PRODUCES: &[MeasurementKind] = &[MeasurementKind::Structured];
const PARAMS_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["minimum_mutual_information_bits","minimum_samples"],"properties":{"minimum_mutual_information_bits":{"type":"number","minimum":0},"minimum_samples":{"type":"integer","minimum":2}}}"#;

pub struct CrossDomainCommunitySensor {
    descriptor: SensorDescriptor,
}

impl Default for CrossDomainCommunitySensor {
    fn default() -> Self {
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.cross-domain-communities"),
                version: Version::new(0, 1, 0),
                applicability: APPLICABILITY,
                evidence: EVIDENCE,
                produces: PRODUCES,
                params_schema: PARAMS_SCHEMA,
            },
        }
    }
}

fn invalid(error: impl std::fmt::Display) -> PluginError {
    PluginError::Message(error.to_string())
}

impl ProductSensor for CrossDomainCommunitySensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn measure(
        &self,
        ctx: &ProductMeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Calculated<Measurement>> {
        let config: CrossDomainCommunityConfig =
            serde_json::from_value(ctx.params().clone()).map_err(invalid)?;
        let product = ctx.product();
        let frame = ctx.frame();
        let profile = ctx.mutual_information().ok_or_else(|| {
            invalid("cross-domain communities require calculated mutual-information evidence")
        })?;
        let expected_binding = ProductMeasurementBinding {
            product: product.id.clone(),
            product_version: product.version.clone(),
            frame: frame.id.clone(),
            frame_version: frame.version.clone(),
            left: product.left.clone(),
            right: product.right.clone(),
        };
        if profile.binding != expected_binding {
            return Err(invalid(
                "cross-domain communities require mutual information bound to the exact product and frame versions",
            ));
        }
        let frame_coordinates = frame
            .axes
            .iter()
            .map(|axis| (&axis.left, &axis.right))
            .collect::<BTreeSet<_>>();
        let profile_coordinates = profile
            .axes
            .iter()
            .map(|axis| (&axis.left, &axis.right))
            .chain(
                profile
                    .unassessed_axes
                    .iter()
                    .map(|axis| (&axis.left, &axis.right)),
            )
            .collect::<BTreeSet<_>>();
        if frame_coordinates.len() != frame.axes.len()
            || profile_coordinates.len() != profile.axes.len() + profile.unassessed_axes.len()
            || profile_coordinates != frame_coordinates
        {
            return Err(invalid(
                "cross-domain communities require mutual information for every and only product-frame axis",
            ));
        }

        let outcome = detect_cross_domain_communities(profile, config).map_err(invalid)?;
        let (reading, sample_count, status, unassessed) = match outcome {
            CrossDomainCommunityOutcome::Value { detection } => {
                let unassessed = detection.unassessed_interactions.clone();
                (
                    Reading::Value {
                        value: MeasurementValue::Structured(
                            serde_json::to_value(detection).map_err(invalid)?,
                        ),
                    },
                    None,
                    "value",
                    unassessed,
                )
            }
            CrossDomainCommunityOutcome::InsufficientEvidence {
                have,
                need,
                unassessed_interactions,
            } => (
                Reading::InsufficientEvidence { have, need },
                Some(have),
                "insufficient_evidence",
                unassessed_interactions,
            ),
        };
        let context = BTreeMap::from([
            (
                "product_domain".into(),
                serde_json::to_value(&product.id).map_err(invalid)?,
            ),
            (
                "product_version".into(),
                serde_json::to_value(&product.version).map_err(invalid)?,
            ),
            (
                "product_frame".into(),
                serde_json::to_value(&frame.id).map_err(invalid)?,
            ),
            (
                "product_frame_version".into(),
                serde_json::to_value(&frame.version).map_err(invalid)?,
            ),
            (
                "left_domain".into(),
                serde_json::to_value(&product.left).map_err(invalid)?,
            ),
            (
                "right_domain".into(),
                serde_json::to_value(&product.right).map_err(invalid)?,
            ),
            (
                "unassessed_interactions".into(),
                serde_json::to_value(unassessed).map_err(invalid)?,
            ),
            ("status".into(), serde_json::json!(status)),
        ]);
        Ok(token.emit(Measurement {
            sensor: self.descriptor.id.clone(),
            sensor_version: self.descriptor.version.clone(),
            reading,
            confidence: None,
            sample_count,
            context: MeasurementContext { values: context },
        }))
    }
}
