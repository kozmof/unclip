use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use unclip_domain::UnitId;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    canonical_correlation_iter, CanonicalCorrelationConfig, CanonicalCorrelationOutcome,
    CanonicalCorrelationUndefined, Measurement, MeasurementContext, MeasurementKind,
    MeasurementValue, Reading,
};
use unclip_plugin::{
    Capability, EvidenceRequirement, PluginError, ProductMeasureCtx, ProductSensor, Result,
    SensorDescriptor, SensorStage,
};

const APPLICABILITY: &[Capability] = &[Capability::ProductDomain];
const EVIDENCE: &[EvidenceRequirement] = &[EvidenceRequirement::MinSamples(2)];
const PRODUCES: &[MeasurementKind] = &[MeasurementKind::Structured];
const PARAMS_SCHEMA: &str = r#"{
    "type":"object",
    "additionalProperties":false,
    "required":["minimum_samples","regularization","tolerance","max_sweeps"],
    "properties":{"minimum_samples":{"type":"integer","minimum":2},"regularization":{"type":"number","minimum":0},"tolerance":{"type":"number","exclusiveMinimum":0,"exclusiveMaximum":1},"max_sweeps":{"type":"integer","minimum":1}}
}"#;

pub struct CanonicalCorrelationSensor {
    descriptor: SensorDescriptor,
}

impl Default for CanonicalCorrelationSensor {
    fn default() -> Self {
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.canonical-correlation"),
                version: Version::new(0, 1, 0),
                stage: SensorStage::Measurement,
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

fn frame_units(ctx: &ProductMeasureCtx<'_>) -> Result<(Vec<UnitId>, Vec<UnitId>)> {
    let product = ctx.product();
    let frame = ctx.frame();
    if frame.product != product.id
        || frame.product_version != product.version
        || frame.left != product.left
        || frame.right != product.right
    {
        return Err(invalid(
            "canonical correlation requires a frame bound to the exact product inputs",
        ));
    }
    let mut seen_left = BTreeSet::new();
    let mut seen_right = BTreeSet::new();
    let mut left = Vec::new();
    let mut right = Vec::new();
    for axis in &frame.axes {
        if seen_left.insert(&axis.left) {
            left.push(axis.left.clone());
        }
        if seen_right.insert(&axis.right) {
            right.push(axis.right.clone());
        }
    }
    Ok((left, right))
}

impl ProductSensor for CanonicalCorrelationSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn measure(
        &self,
        ctx: &ProductMeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Calculated<Measurement>> {
        let config: CanonicalCorrelationConfig = serde::Deserialize::deserialize(ctx.params())
            .map_err(crate::support::invalid_params)?;
        let (left_units, right_units) = frame_units(ctx)?;
        // Read every selected sample, including when calculation exits early.
        let samples = ctx
            .samples()
            .iter()
            .map(|sample| ctx.read(sample))
            .collect::<Vec<_>>();
        let outcome =
            canonical_correlation_iter(&left_units, &right_units, samples.into_iter(), config)
                .map_err(crate::support::calculation)?;

        let (reading, sample_count, excluded, status) = match outcome {
            CanonicalCorrelationOutcome::Value { analysis } => {
                let sample_count = analysis.sample_count;
                let reading_value = serde_json::to_value(&analysis).map_err(invalid)?;
                let excluded = analysis.excluded_observations;
                (
                    Reading::Value {
                        value: MeasurementValue::Structured(reading_value),
                    },
                    sample_count,
                    excluded,
                    "value",
                )
            }
            CanonicalCorrelationOutcome::InsufficientEvidence {
                have,
                need,
                excluded_observations,
            } => (
                Reading::InsufficientEvidence { have, need },
                have,
                excluded_observations,
                "insufficient_evidence",
            ),
            CanonicalCorrelationOutcome::Undefined {
                sample_count,
                excluded_observations,
                reason,
            } => {
                let reason = match reason {
                    CanonicalCorrelationUndefined::NoVariables => {
                        "product frame has no left or right variables"
                    }
                    CanonicalCorrelationUndefined::NoVariation => {
                        "complete samples have no variation on at least one side"
                    }
                };
                (
                    Reading::NotApplicable {
                        reason: reason.into(),
                    },
                    sample_count,
                    excluded_observations,
                    "not_applicable",
                )
            }
        };
        let product = ctx.product();
        let frame = ctx.frame();
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
                "excluded_observations".into(),
                serde_json::to_value(excluded).map_err(invalid)?,
            ),
            ("status".into(), serde_json::json!(status)),
        ]);
        Ok(token.emit(Measurement {
            sensor: self.descriptor.id.clone(),
            sensor_version: self.descriptor.version.clone(),
            reading,
            confidence: None,
            sample_count: Some(sample_count),
            context: MeasurementContext { values: context },
        }))
    }
}
