use std::collections::BTreeMap;

use semver::Version;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    cross_domain_mutual_information, CrossDomainMutualInformationConfig,
    CrossDomainMutualInformationOutcome, Measurement, MeasurementContext, MeasurementKind,
    MeasurementValue, Reading,
};
use unclip_plugin::{
    Capability, EvidenceRequirement, PluginError, ProductMeasureCtx, ProductSensor, Result,
    SensorDescriptor,
};

const APPLICABILITY: &[Capability] = &[Capability::ProductDomain];
const EVIDENCE: &[EvidenceRequirement] = &[EvidenceRequirement::MinSamples(2)];
const PRODUCES: &[MeasurementKind] = &[MeasurementKind::Structured];
const PARAMS_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["minimum_samples","bins"],"properties":{"minimum_samples":{"type":"integer","minimum":2},"bins":{"type":"integer","minimum":1,"maximum":1024}}}"#;

pub struct CrossDomainMutualInformationSensor {
    descriptor: SensorDescriptor,
}

impl Default for CrossDomainMutualInformationSensor {
    fn default() -> Self {
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.cross-domain-mutual-information"),
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

impl ProductSensor for CrossDomainMutualInformationSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn measure(
        &self,
        ctx: &ProductMeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Calculated<Measurement>> {
        let config: CrossDomainMutualInformationConfig =
            serde_json::from_value(ctx.params().clone()).map_err(invalid)?;
        let product = ctx.product();
        let frame = ctx.frame();
        if frame.product != product.id
            || frame.product_version != product.version
            || frame.left != product.left
            || frame.right != product.right
        {
            return Err(invalid(
                "cross-domain mutual information requires a frame bound to the exact product inputs",
            ));
        }
        let samples = ctx
            .samples()
            .iter()
            .map(|sample| ctx.read(sample).clone())
            .collect::<Vec<_>>();
        let outcome =
            cross_domain_mutual_information(&frame.axes, &samples, config).map_err(invalid)?;

        let (reading, sample_count, status, unassessed) = match outcome {
            CrossDomainMutualInformationOutcome::Value { analysis } => {
                let unassessed = analysis.unassessed_axes.clone();
                (
                    Reading::Value {
                        value: MeasurementValue::Structured(
                            serde_json::to_value(analysis).map_err(invalid)?,
                        ),
                    },
                    None,
                    "value",
                    unassessed,
                )
            }
            CrossDomainMutualInformationOutcome::InsufficientEvidence {
                have,
                need,
                unassessed_axes,
            } => (
                Reading::InsufficientEvidence { have, need },
                Some(have),
                "insufficient_evidence",
                unassessed_axes,
            ),
            CrossDomainMutualInformationOutcome::NoAxes { observation_count } => (
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
