//! Version-bound measurement profiles for a product and its two input domains.

use std::collections::BTreeSet;

use crate::support::invalid;
use serde::{Deserialize, Serialize};
use unclip_domain::{
    DomainId, DomainSnapshot, FrameId, MeasurementFrame, ProductDomainSnapshot,
    ProductMeasurementFrame,
};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, DomainVersion,
    EmitMetadata, FrameVersion, PluginId, Timestamp, Tracked,
};
use unclip_measure::{Measurement, ProductMeasurementBinding};
use unclip_plugin::Result;

/// One calculated measurement retained with the identity used by provenance.
///
/// The measurement is shared with the derived value it was read from: a
/// composition profile retains one entry per input measurement, and the inputs
/// outlive the profile, so copying each reading and its context per profile is
/// avoidable work that scales with the number of measured axes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileMeasurement {
    pub id: DerivedId,
    pub measurement: std::sync::Arc<Measurement>,
}

/// An ordinary-domain profile pinned to one immutable domain and frame version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundDomainMeasurementProfile {
    pub domain: DomainId,
    pub domain_version: DomainVersion,
    pub frame: FrameId,
    pub frame_version: FrameVersion,
    pub measurements: Vec<ProfileMeasurement>,
}

/// A product-domain profile that retains both input-domain versions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundProductMeasurementProfile {
    pub binding: ProductMeasurementBinding,
    pub measurements: Vec<ProfileMeasurement>,
}

/// Separate `M(A)`, `M(B)`, and `M(A x B)` profiles for later typed comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompositionMeasurementProfile {
    pub left: BoundDomainMeasurementProfile,
    pub right: BoundDomainMeasurementProfile,
    pub product: BoundProductMeasurementProfile,
}

/// Already calculated measurements selected for each side of a composition.
pub struct CompositionMeasurementInputs<'a> {
    pub left: &'a [Calculated<Measurement>],
    pub right: &'a [Calculated<Measurement>],
    pub product: &'a [Calculated<Measurement>],
}

fn validate_frame(domain: &DomainSnapshot, frame: &MeasurementFrame) -> Result<()> {
    if frame.id.trim().is_empty() || frame.version.trim().is_empty() {
        return Err(invalid(
            "composition profiles require nonempty ordinary frame identities and versions",
        ));
    }
    let mut axes = BTreeSet::new();
    for axis in &frame.axes {
        if axis.unit.trim().is_empty()
            || !domain.units.contains_key(&axis.unit)
            || axis
                .label
                .as_ref()
                .is_some_and(|label| label.trim().is_empty())
            || !axes.insert(&axis.unit)
        {
            return Err(invalid(
                "composition profiles require unique ordinary frame axes from their bound domain",
            ));
        }
    }
    Ok(())
}

fn validate_measurement_identity(measurement: &Calculated<Measurement>) -> Result<()> {
    let value = measurement.value();
    let provenance = measurement.provenance();
    if measurement.id().trim().is_empty()
        || value.sensor.trim().is_empty()
        || provenance.producer != value.sensor
        || provenance.version != value.sensor_version
        || provenance.algorithm.trim().is_empty()
        || provenance.params_hash != hash_params(&provenance.params)
    {
        return Err(invalid(
            "composition profiles require internally consistent calculated measurements",
        ));
    }
    Ok(())
}

fn ordinary_measurements(
    inputs: &[Calculated<Measurement>],
    domain_version: &DomainVersion,
    frame_version: &FrameVersion,
    dependencies: &DependencyCollector,
    identities: &mut BTreeSet<DerivedId>,
) -> Result<Vec<ProfileMeasurement>> {
    if inputs.is_empty() {
        return Err(invalid(
            "composition profiles require at least one measurement for each ordinary domain",
        ));
    }
    let mut result = Vec::with_capacity(inputs.len());
    for input in inputs {
        validate_measurement_identity(input)?;
        if input.provenance().domain_version.as_ref() != Some(domain_version)
            || input.provenance().frame_version.as_ref() != Some(frame_version)
        {
            return Err(invalid(
                "ordinary composition measurements must match their exact domain and frame versions",
            ));
        }
        if !identities.insert(input.id().clone()) {
            return Err(invalid(
                "composition profile measurements require globally unique identities",
            ));
        }
        result.push(ProfileMeasurement {
            id: input.id().clone(),
            measurement: dependencies.read_derived_shared(input),
        });
    }
    result.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(result)
}

fn matches_field(
    values: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    expected: &serde_json::Value,
) -> bool {
    values.get(key) == Some(expected)
}

fn product_measurements(
    inputs: &[Calculated<Measurement>],
    binding: &ProductMeasurementBinding,
    dependencies: &DependencyCollector,
    identities: &mut BTreeSet<DerivedId>,
) -> Result<Vec<ProfileMeasurement>> {
    if inputs.is_empty() {
        return Err(invalid(
            "composition profiles require at least one product measurement",
        ));
    }
    let expected = [
        ("product_domain", serde_json::to_value(&binding.product)),
        (
            "product_version",
            serde_json::to_value(&binding.product_version),
        ),
        ("product_frame", serde_json::to_value(&binding.frame)),
        (
            "product_frame_version",
            serde_json::to_value(&binding.frame_version),
        ),
        ("left_domain", serde_json::to_value(&binding.left)),
        ("right_domain", serde_json::to_value(&binding.right)),
    ];
    let expected = expected
        .into_iter()
        .map(|(key, value)| value.map(|value| (key, value)).map_err(invalid))
        .collect::<Result<Vec<_>>>()?;

    let mut result = Vec::with_capacity(inputs.len());
    for input in inputs {
        validate_measurement_identity(input)?;
        let params = input.provenance().params.as_object().ok_or_else(|| {
            invalid("product measurement provenance parameters must be an object")
        })?;
        let context = &input.value().context.values;
        if input.provenance().domain_version.is_some()
            || input.provenance().frame_version.is_some()
            || expected.iter().any(|(key, value)| {
                let provenance_key = match *key {
                    "left_domain" => "left",
                    "right_domain" => "right",
                    other => other,
                };
                !matches_field(params, provenance_key, value) || context.get(*key) != Some(value)
            })
        {
            return Err(invalid(
                "product composition measurements must match the exact product, frame, and input-domain versions",
            ));
        }
        if !identities.insert(input.id().clone()) {
            return Err(invalid(
                "composition profile measurements require globally unique identities",
            ));
        }
        result.push(ProfileMeasurement {
            id: input.id().clone(),
            measurement: dependencies.read_derived_shared(input),
        });
    }
    result.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(result)
}

/// One input domain of a composition with the frame it was measured against.
///
/// Pairing them is what keeps a composition's two sides straight: the four
/// references used to be positional, with `left_domain`/`right_domain` sharing a
/// type and `left_frame`/`right_frame` sharing another, so a transposed pair
/// compiled and bound each side's measurements to the other side's frame.
pub struct CompositionSide<'a> {
    pub domain: &'a Tracked<DomainSnapshot>,
    pub frame: &'a Tracked<MeasurementFrame>,
}

/// Bind independently calculated input-domain and product-domain measurements.
///
/// The three profiles remain separate and retain every typed reading. This
/// stage does not infer correspondence, independence, or a scalar score.
pub fn measure_composition(
    left: CompositionSide<'_>,
    right: CompositionSide<'_>,
    product: &Tracked<ProductDomainSnapshot>,
    product_frame: &Tracked<ProductMeasurementFrame>,
    inputs: CompositionMeasurementInputs<'_>,
    run_id: &str,
    timestamp: Timestamp,
) -> Result<Calculated<CompositionMeasurementProfile>> {
    let (left_domain, left_frame) = (left.domain, left.frame);
    let (right_domain, right_frame) = (right.domain, right.frame);
    if run_id.trim().is_empty() {
        return Err(invalid(
            "composition measurement requires a nonempty run ID",
        ));
    }
    crate::require_calculated_evidence(product, "composition product domain")?;
    crate::require_calculated_evidence(product_frame, "composition product frame")?;

    let output_id = DerivedId::new(format!("{run_id}/composition-profile"));
    let dependencies = DependencyCollector::default();
    let left = dependencies.read(left_domain);
    let left_frame_value = dependencies.read(left_frame);
    let right = dependencies.read(right_domain);
    let right_frame_value = dependencies.read(right_frame);
    let product_value = dependencies.read(product);
    let product_frame_value = dependencies.read(product_frame);

    crate::nulls::domain::validate(left)?;
    crate::nulls::domain::validate(right)?;
    validate_frame(left, left_frame_value)?;
    validate_frame(right, right_frame_value)?;
    crate::product_domain::validate_product_snapshot(product_value)?;
    crate::cross_domain::validate_frame(product_value, product_frame_value)?;

    if left.id == right.id
        || product_value.left.domain != left.id
        || product_value.left.version != left.version
        || product_value.right.domain != right.id
        || product_value.right.version != right.version
    {
        return Err(invalid(
            "composition product inputs must match the exact selected left and right domain versions",
        ));
    }

    let structural_ids = [
        left_domain.id(),
        left_frame.id(),
        right_domain.id(),
        right_frame.id(),
        product.id(),
        product_frame.id(),
    ];
    if structural_ids
        .iter()
        .any(|identity| identity.trim().is_empty() || *identity == &output_id)
        || structural_ids.iter().collect::<BTreeSet<_>>().len() != structural_ids.len()
    {
        return Err(invalid(
            "composition structural inputs require distinct nonempty identities that do not collide with the output",
        ));
    }

    let binding = ProductMeasurementBinding {
        product: product_value.id.clone(),
        product_version: product_value.version.clone(),
        frame: product_frame_value.id.clone(),
        frame_version: product_frame_value.version.clone(),
        left: product_value.left.clone(),
        right: product_value.right.clone(),
    };
    let mut identities = structural_ids.into_iter().cloned().collect::<BTreeSet<_>>();
    identities.insert(output_id.clone());
    let left_measurements = ordinary_measurements(
        inputs.left,
        &left.version,
        &left_frame_value.version,
        &dependencies,
        &mut identities,
    )?;
    let right_measurements = ordinary_measurements(
        inputs.right,
        &right.version,
        &right_frame_value.version,
        &dependencies,
        &mut identities,
    )?;
    let product_measurements =
        product_measurements(inputs.product, &binding, &dependencies, &mut identities)?;

    let value = CompositionMeasurementProfile {
        left: BoundDomainMeasurementProfile {
            domain: left.id.clone(),
            domain_version: left.version.clone(),
            frame: left_frame_value.id.clone(),
            frame_version: left_frame_value.version.clone(),
            measurements: left_measurements,
        },
        right: BoundDomainMeasurementProfile {
            domain: right.id.clone(),
            domain_version: right.version.clone(),
            frame: right_frame_value.id.clone(),
            frame_version: right_frame_value.version.clone(),
            measurements: right_measurements,
        },
        product: BoundProductMeasurementProfile {
            binding,
            measurements: product_measurements,
        },
    };
    let params = serde_json::json!({
        "left": {
            "domain": &value.left.domain,
            "domain_version": &value.left.domain_version,
            "frame": &value.left.frame,
            "frame_version": &value.left.frame_version,
            "measurements": value.left.measurements.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
        },
        "right": {
            "domain": &value.right.domain,
            "domain_version": &value.right.domain_version,
            "frame": &value.right.frame,
            "frame_version": &value.right.frame_version,
            "measurements": value.right.measurements.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
        },
        "product": {
            "binding": &value.product.binding,
            "measurements": value.product.measurements.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
        },
    });
    let token = CalculationToken::from_harness(
        EmitMetadata::new(
            output_id,
            PluginId::new("calculate.composition-profile"),
            semver::Version::new(0, 1, 0),
            params,
            timestamp,
        )
        .with_algorithm("versioned_composition_measurement_profiles"),
        dependencies,
    );
    Ok(token.emit(value))
}
