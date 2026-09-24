use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    cross_product_transfer, CrossProductTransferConfig, CrossProductTransferOutcome, Measurement,
    MeasurementContext, MeasurementKind, MeasurementValue, ProductMeasurementBinding, Reading,
};
use unclip_plugin::{
    Capability, CrossProductMeasureCtx, CrossProductSensor, EvidenceRequirement, PluginError,
    Result, SensorDescriptor,
};

const APPLICABILITY: &[Capability] = &[Capability::ProductDomain, Capability::Ordered];
const EVIDENCE: &[EvidenceRequirement] = &[
    EvidenceRequirement::ExplicitOrder,
    EvidenceRequirement::MinSamples(2),
];
const PRODUCES: &[MeasurementKind] = &[MeasurementKind::Structured];
const PARAMS_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["mappings","minimum_transitions"],"properties":{"mappings":{"type":"array","minItems":1,"items":{"type":"object","additionalProperties":false,"required":["source_left","source_right","target_left","target_right"],"properties":{"source_left":{"type":"string","minLength":1},"source_right":{"type":"string","minLength":1},"target_left":{"type":"string","minLength":1},"target_right":{"type":"string","minLength":1}}}},"minimum_transitions":{"type":"integer","minimum":1}}}"#;

pub struct CrossProductTransferSensor {
    descriptor: SensorDescriptor,
}

impl Default for CrossProductTransferSensor {
    fn default() -> Self {
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.cross-product-transfer"),
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

fn binding(
    product: &unclip_domain::ProductDomainSnapshot,
    frame: &unclip_domain::ProductMeasurementFrame,
) -> ProductMeasurementBinding {
    ProductMeasurementBinding {
        product: product.id.clone(),
        product_version: product.version.clone(),
        frame: frame.id.clone(),
        frame_version: frame.version.clone(),
        left: product.left.clone(),
        right: product.right.clone(),
    }
}

fn covers_frame(
    movement: &unclip_measure::CrossDomainInteractionMovement,
    frame: &unclip_domain::ProductMeasurementFrame,
) -> bool {
    let frame_coordinates = frame
        .axes
        .iter()
        .map(|axis| (&axis.left, &axis.right))
        .collect::<BTreeSet<_>>();
    let movement_coordinates = movement
        .axes
        .iter()
        .map(|axis| (&axis.left, &axis.right))
        .chain(
            movement
                .unassessed_axes
                .iter()
                .map(|axis| (&axis.left, &axis.right)),
        )
        .collect::<BTreeSet<_>>();
    frame_coordinates.len() == frame.axes.len()
        && movement_coordinates.len() == movement.axes.len() + movement.unassessed_axes.len()
        && movement_coordinates == frame_coordinates
}

impl CrossProductSensor for CrossProductTransferSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn measure(
        &self,
        ctx: &CrossProductMeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Calculated<Measurement>> {
        let config: CrossProductTransferConfig =
            serde_json::from_value(ctx.params().clone()).map_err(invalid)?;
        let source_product = ctx.source_product();
        let source_frame = ctx.source_frame();
        let source_movement = ctx.source_movement();
        let target_product = ctx.target_product();
        let target_frame = ctx.target_frame();
        let target_movement = ctx.target_movement();
        if source_movement.binding != binding(source_product, source_frame)
            || target_movement.binding != binding(target_product, target_frame)
            || !covers_frame(source_movement, source_frame)
            || !covers_frame(target_movement, target_frame)
        {
            return Err(invalid(
                "cross-product transfer requires complete movement evidence bound to the exact source and target product-frame versions",
            ));
        }

        let outcome =
            cross_product_transfer(source_movement, target_movement, config).map_err(invalid)?;
        let (reading, sample_count, status, unassessed) = match outcome {
            CrossProductTransferOutcome::Value { transfer } => {
                let unassessed = transfer.unassessed_transfers.clone();
                (
                    Reading::Value {
                        value: MeasurementValue::Structured(
                            serde_json::to_value(transfer).map_err(invalid)?,
                        ),
                    },
                    None,
                    "value",
                    unassessed,
                )
            }
            CrossProductTransferOutcome::InsufficientEvidence {
                have,
                need,
                unassessed_transfers,
            } => (
                Reading::InsufficientEvidence { have, need },
                Some(have),
                "insufficient_evidence",
                unassessed_transfers,
            ),
        };
        let context = BTreeMap::from([
            (
                "source_product".into(),
                serde_json::to_value(&source_product.id).map_err(invalid)?,
            ),
            (
                "source_product_version".into(),
                serde_json::to_value(&source_product.version).map_err(invalid)?,
            ),
            (
                "source_frame".into(),
                serde_json::to_value(&source_frame.id).map_err(invalid)?,
            ),
            (
                "source_frame_version".into(),
                serde_json::to_value(&source_frame.version).map_err(invalid)?,
            ),
            (
                "target_product".into(),
                serde_json::to_value(&target_product.id).map_err(invalid)?,
            ),
            (
                "target_product_version".into(),
                serde_json::to_value(&target_product.version).map_err(invalid)?,
            ),
            (
                "target_frame".into(),
                serde_json::to_value(&target_frame.id).map_err(invalid)?,
            ),
            (
                "target_frame_version".into(),
                serde_json::to_value(&target_frame.version).map_err(invalid)?,
            ),
            (
                "unassessed_transfers".into(),
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
