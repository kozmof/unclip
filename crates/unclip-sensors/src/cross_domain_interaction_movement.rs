use std::collections::BTreeMap;

use semver::Version;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    cross_domain_interaction_movement, CrossDomainInteractionMovementConfig,
    CrossDomainInteractionMovementOutcome, Measurement, MeasurementContext, MeasurementKind,
    MeasurementValue, ProductMeasurementBinding, Reading,
};
use unclip_plugin::{
    Capability, EvidenceRequirement, PluginError, ProductMeasureCtx, ProductSensor, Result,
    SensorDescriptor,
};

const APPLICABILITY: &[Capability] = &[Capability::ProductDomain, Capability::Ordered];
const EVIDENCE: &[EvidenceRequirement] = &[
    EvidenceRequirement::ExplicitOrder,
    EvidenceRequirement::MinSamples(2),
];
const PRODUCES: &[MeasurementKind] = &[MeasurementKind::Structured];
const PARAMS_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["minimum_transitions","sequence"],"properties":{"minimum_transitions":{"type":"integer","minimum":1},"sequence":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["observation","position"],"properties":{"observation":{"type":"string"},"position":{"type":"integer"}}}}}}"#;

pub struct CrossDomainInteractionMovementSensor {
    descriptor: SensorDescriptor,
}

impl Default for CrossDomainInteractionMovementSensor {
    fn default() -> Self {
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.cross-domain-interaction-movement"),
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

impl ProductSensor for CrossDomainInteractionMovementSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn measure(
        &self,
        ctx: &ProductMeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Calculated<Measurement>> {
        let config: CrossDomainInteractionMovementConfig =
            serde_json::from_value(ctx.params().clone()).map_err(invalid)?;
        let product = ctx.product();
        let frame = ctx.frame();
        if frame.product != product.id
            || frame.product_version != product.version
            || frame.left != product.left
            || frame.right != product.right
        {
            return Err(invalid(
                "cross-domain interaction movement requires a frame bound to the exact product inputs",
            ));
        }
        let samples = ctx
            .samples()
            .iter()
            .map(|sample| ctx.read(sample).clone())
            .collect::<Vec<_>>();
        let outcome = cross_domain_interaction_movement(
            ProductMeasurementBinding {
                product: product.id.clone(),
                product_version: product.version.clone(),
                frame: frame.id.clone(),
                frame_version: frame.version.clone(),
                left: product.left.clone(),
                right: product.right.clone(),
            },
            &frame.axes,
            &samples,
            config,
        )
        .map_err(invalid)?;

        let (reading, sample_count, status, unassessed) = match outcome {
            CrossDomainInteractionMovementOutcome::Value { movement } => {
                let unassessed = movement.unassessed_axes.clone();
                (
                    Reading::Value {
                        value: MeasurementValue::Structured(
                            serde_json::to_value(movement).map_err(invalid)?,
                        ),
                    },
                    None,
                    "value",
                    unassessed,
                )
            }
            CrossDomainInteractionMovementOutcome::InsufficientEvidence {
                have,
                need,
                unassessed_axes,
            } => (
                Reading::InsufficientEvidence { have, need },
                Some(have),
                "insufficient_evidence",
                unassessed_axes,
            ),
            CrossDomainInteractionMovementOutcome::NoAxes { observation_count } => (
                Reading::NotApplicable {
                    reason: "product frame has no interaction axes".into(),
                },
                Some(observation_count),
                "not_applicable",
                vec![],
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
                "unassessed_axes".into(),
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
