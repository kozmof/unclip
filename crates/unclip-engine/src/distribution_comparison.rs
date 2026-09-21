//! Explicit normalization and Jensen-Shannon divergence over named categories.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{Delta, MeasurementKind, MeasurementValue, Reading};
use unclip_plugin::{Comparator, ComparatorDescriptor, CompareCtx, PluginError, Result};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistributionNormalization {
    Probability,
    Mass,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum DistributionComparison {
    Value {
        divergence_bits: f64,
        normalization: DistributionNormalization,
        categories: Vec<String>,
        before_probabilities: Vec<f64>,
        after_probabilities: Vec<f64>,
        before_total: f64,
        after_total: f64,
    },
    Unavailable {
        reason: String,
        before: Reading,
        after: Reading,
    },
    NotApplicable {
        reason: String,
    },
}
pub struct JensenShannonComparator {
    descriptor: ComparatorDescriptor,
}
impl Default for JensenShannonComparator {
    fn default() -> Self {
        Self {
            descriptor: ComparatorDescriptor {
                id: PluginId::new("compare.jensen-shannon"),
                version: semver::Version::new(0, 1, 0),
                supports: &[MeasurementKind::Distribution],
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["normalization"],"properties":{"normalization":{"enum":["probability","mass"]}}}"#,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    normalization: DistributionNormalization,
}
fn invalid(s: &str) -> PluginError {
    PluginError::Message(s.into())
}
fn distribution(values: &[(String, f64)]) -> Result<(BTreeMap<&str, f64>, f64)> {
    let mut map = BTreeMap::new();
    let mut total = 0.0;
    // Sum in category order so input ordering cannot change rounding or replay.
    for (category, value) in values {
        if category.trim().is_empty()
            || !value.is_finite()
            || *value < 0.0
            || map.insert(category.as_str(), *value).is_some()
        {
            return Err(invalid("distribution categories must be unique and nonempty, with finite nonnegative values"));
        }
    }
    for value in map.values() {
        total += value;
    }
    if !total.is_finite() {
        return Err(invalid("distribution total exceeds finite numeric range"));
    }
    Ok((map, total))
}
impl Comparator for JensenShannonComparator {
    fn descriptor(&self) -> &ComparatorDescriptor {
        &self.descriptor
    }
    fn compare(&self, ctx: &CompareCtx<'_>, token: CalculationToken) -> Result<Calculated<Delta>> {
        let params: Parameters = serde_json::from_value(ctx.params().clone())
            .map_err(|e| PluginError::Message(e.to_string()))?;
        let before = ctx.before();
        let after = ctx.after();
        if before.sensor != after.sensor
            || before.sensor_version != after.sensor_version
            || before.context != after.context
        {
            return Err(invalid("distribution comparison requires the same sensor, version, and measurement context"));
        }
        let mut parsed = Vec::new();
        for reading in [&before.reading, &after.reading] {
            parsed.push(
                if let Reading::Value {
                    value: MeasurementValue::Distribution(values),
                } = reading
                {
                    {
                        let parsed = distribution(values)?;
                        if params.normalization == DistributionNormalization::Probability
                            && parsed.1 > 0.0
                            && (parsed.1 - 1.0).abs() > 1e-12
                        {
                            return Err(invalid(
                                "probability distributions must sum to one within 1e-12",
                            ));
                        }
                        Some(parsed)
                    }
                } else {
                    None
                },
            );
        }
        let unsupported = |reading: &Reading| matches!(reading,Reading::Value {value} if !matches!(value,MeasurementValue::Distribution(_)));
        let result = if unsupported(&before.reading) || unsupported(&after.reading) {
            DistributionComparison::NotApplicable {
                reason:
                    "requires named distributions; no ordering or transport geometry is inferred"
                        .into(),
            }
        } else if let (Some((a, a_total)), Some((b, b_total))) = (&parsed[0], &parsed[1]) {
            if *a_total == 0.0 || *b_total == 0.0 {
                DistributionComparison::Unavailable {
                    reason: "both distributions require positive total mass".into(),
                    before: before.reading.clone(),
                    after: after.reading.clone(),
                }
            } else {
                if params.normalization == DistributionNormalization::Probability
                    && ((a_total - 1.0).abs() > 1e-12 || (b_total - 1.0).abs() > 1e-12)
                {
                    return Err(invalid(
                        "probability distributions must sum to one within 1e-12",
                    ));
                }
                let categories = a.keys().chain(b.keys()).copied().collect::<BTreeSet<_>>();
                let mut left = Vec::new();
                let mut right = Vec::new();
                let mut divergence = 0.0;
                for category in &categories {
                    let p = a.get(category).copied().unwrap_or(0.0) / a_total;
                    let q = b.get(category).copied().unwrap_or(0.0) / b_total;
                    // Avoid halving a subnormal mixture before taking its ratio.
                    if p > 0.0 {
                        divergence += 0.5 * p * (2.0 * (p / (p + q))).log2();
                    }
                    if q > 0.0 {
                        divergence += 0.5 * q * (2.0 * (q / (p + q))).log2();
                    }
                    left.push(p);
                    right.push(q);
                }
                if !divergence.is_finite() {
                    return Err(invalid(
                        "distribution divergence exceeds finite numeric range",
                    ));
                }
                DistributionComparison::Value {
                    divergence_bits: divergence.clamp(0.0, 1.0),
                    normalization: params.normalization,
                    categories: categories.into_iter().map(str::to_owned).collect(),
                    before_probabilities: left,
                    after_probabilities: right,
                    before_total: *a_total,
                    after_total: *b_total,
                }
            }
        } else {
            DistributionComparison::Unavailable {
                reason: "both distribution readings must be measured".into(),
                before: before.reading.clone(),
                after: after.reading.clone(),
            }
        };
        Ok(token.emit(Delta {
            comparator: self.descriptor.id.clone(),
            value: MeasurementValue::Structured(
                serde_json::to_value(result).map_err(|e| PluginError::Message(e.to_string()))?,
            ),
        }))
    }
}
