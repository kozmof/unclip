//! Calculated measurements over explicitly versioned product domains.

use std::collections::BTreeSet;

use unclip_domain::{ProductDomainSnapshot, ProductMeasurementFrame, UnitId};
use unclip_epistemic::{
    hash_params, Calculated, DependencyCollector, DerivedId, EmitMetadata, Operation, PluginId,
    Timestamp, Tracked,
};
use unclip_measure::{
    CanonicalCorrelationConfig, CrossDomainCommunityConfig, CrossDomainInteractionMovementConfig,
    CrossDomainMutualInformation, CrossDomainMutualInformationConfig, CrossDomainSample,
    CrossProductTransferConfig, Measurement,
};
use unclip_plugin::{CrossProductMeasureCtx, PluginError, ProductMeasureCtx, Result};

const CCA_SENSOR_ID: &str = "sensor.canonical-correlation";
const MI_SENSOR_ID: &str = "sensor.cross-domain-mutual-information";
const COMMUNITY_SENSOR_ID: &str = "sensor.cross-domain-communities";
const INTERACTION_MOVEMENT_SENSOR_ID: &str = "sensor.cross-domain-interaction-movement";
const CROSS_PRODUCT_TRANSFER_SENSOR_ID: &str = "sensor.cross-product-transfer";

fn invalid(message: impl std::fmt::Display) -> PluginError {
    PluginError::Message(message.to_string())
}

fn validate_frame(product: &ProductDomainSnapshot, frame: &ProductMeasurementFrame) -> Result<()> {
    if frame.id.0.trim().is_empty()
        || frame.version.0.trim().is_empty()
        || frame.product != product.id
        || frame.product_version != product.version
        || frame.left != product.left
        || frame.right != product.right
    {
        return Err(invalid(
            "canonical correlation requires a product frame bound to the exact product and input versions",
        ));
    }
    let interactions = product
        .interactions
        .iter()
        .map(|interaction| (&interaction.left, &interaction.right))
        .collect::<BTreeSet<_>>();
    let mut axes = BTreeSet::new();
    for axis in &frame.axes {
        if axis.left.0.trim().is_empty()
            || axis.right.0.trim().is_empty()
            || axis
                .label
                .as_ref()
                .is_some_and(|label| label.trim().is_empty())
            || !interactions.contains(&(&axis.left, &axis.right))
            || !axes.insert((&axis.left, &axis.right))
        {
            return Err(invalid(
                "canonical correlation requires unique materialized product-frame axes",
            ));
        }
    }
    Ok(())
}

fn frame_units(frame: &ProductMeasurementFrame) -> (Vec<UnitId>, Vec<UnitId>) {
    let mut seen_left = BTreeSet::new();
    let mut seen_right = BTreeSet::new();
    let mut left = Vec::new();
    let mut right = Vec::new();
    for axis in &frame.axes {
        if seen_left.insert(axis.left.clone()) {
            left.push(axis.left.clone());
        }
        if seen_right.insert(axis.right.clone()) {
            right.push(axis.right.clone());
        }
    }
    (left, right)
}

impl crate::Engine {
    /// Measure linear cross-domain association without flattening either input domain.
    ///
    /// Product axes define the ordered left and right variable bases. Samples are
    /// complete-case filtered by the registered product sensor, and every supplied
    /// sample is read through its invocation-scoped dependency collector.
    #[allow(clippy::too_many_arguments)]
    pub fn measure_canonical_correlation(
        &self,
        product: &Tracked<ProductDomainSnapshot>,
        frame: &Tracked<ProductMeasurementFrame>,
        samples: &[Tracked<CrossDomainSample>],
        config: CanonicalCorrelationConfig,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<Measurement>> {
        if run_id.trim().is_empty() {
            return Err(invalid("canonical correlation requires a nonempty run ID"));
        }
        crate::require_calculated_evidence(product, "CCA product domain")?;
        crate::require_calculated_evidence(frame, "CCA product frame")?;
        if samples.iter().any(|sample| {
            matches!(
                sample.operation(),
                Some(Operation::Experimental | Operation::Interpreted)
            )
        }) {
            return Err(invalid(
                "canonical correlation samples must be inferred, calculated, or restored evidence",
            ));
        }

        let sensor_id = PluginId::new(CCA_SENSOR_ID);
        let sensor = self
            .registry()
            .product_sensor(&sensor_id)
            .ok_or_else(|| PluginError::MissingPlugin(sensor_id.clone()))?;
        let descriptor = sensor.descriptor();
        let output_id = DerivedId::new(format!("{run_id}/{}", descriptor.id));
        if product.id() == &output_id
            || frame.id() == &output_id
            || samples.iter().any(|sample| sample.id() == &output_id)
        {
            return Err(invalid(
                "canonical correlation output identity collides with an input",
            ));
        }

        let dependencies = DependencyCollector::default();
        let product_value = dependencies.read(product);
        super::product_domain::validate_product_snapshot(product_value)?;
        let frame_value = dependencies.read(frame);
        validate_frame(product_value, frame_value)?;
        let (left_units, right_units) = frame_units(frame_value);
        let sensor_params = serde_json::to_value(config).map_err(invalid)?;
        let params = serde_json::json!({
            "product": product.id(),
            "product_domain": &product_value.id,
            "product_version": &product_value.version,
            "product_frame": &frame_value.id,
            "product_frame_version": &frame_value.version,
            "left": &product_value.left,
            "right": &product_value.right,
            "left_units": &left_units,
            "right_units": &right_units,
            "minimum_samples": config.minimum_samples,
            "regularization": config.regularization,
            "tolerance": config.tolerance,
            "max_sweeps": config.max_sweeps,
        });
        let metadata = EmitMetadata {
            id: output_id,
            producer: descriptor.id.clone(),
            algorithm: "regularized_canonical_correlation".into(),
            version: descriptor.version.clone(),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp,
            domain_version: None,
            frame_version: None,
            model: None,
        };
        let ctx = ProductMeasureCtx::new(product, frame, samples, &sensor_params, dependencies);
        sensor.measure(&ctx, ctx.calculation_token(metadata))
    }
}

impl crate::Engine {
    /// Measure nonlinear dependence independently for each product-frame interaction axis.
    ///
    /// Numeric values are discretized only through the caller's explicit equal-width
    /// bin count. Each axis uses its own pairwise-complete evidence and retains its
    /// bin boundaries and sparse observation identities in the structured result.
    #[allow(clippy::too_many_arguments)]
    pub fn measure_cross_domain_mutual_information(
        &self,
        product: &Tracked<ProductDomainSnapshot>,
        frame: &Tracked<ProductMeasurementFrame>,
        samples: &[Tracked<CrossDomainSample>],
        config: CrossDomainMutualInformationConfig,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<Measurement>> {
        if run_id.trim().is_empty() {
            return Err(invalid(
                "cross-domain mutual information requires a nonempty run ID",
            ));
        }
        crate::require_calculated_evidence(product, "cross-domain MI product domain")?;
        crate::require_calculated_evidence(frame, "cross-domain MI product frame")?;
        if samples.iter().any(|sample| {
            matches!(
                sample.operation(),
                Some(Operation::Experimental | Operation::Interpreted)
            )
        }) {
            return Err(invalid(
                "cross-domain MI samples must be inferred, calculated, or restored evidence",
            ));
        }

        let sensor_id = PluginId::new(MI_SENSOR_ID);
        let sensor = self
            .registry()
            .product_sensor(&sensor_id)
            .ok_or_else(|| PluginError::MissingPlugin(sensor_id.clone()))?;
        let descriptor = sensor.descriptor();
        let output_id = DerivedId::new(format!("{run_id}/{}", descriptor.id));
        if product.id() == &output_id
            || frame.id() == &output_id
            || samples.iter().any(|sample| sample.id() == &output_id)
        {
            return Err(invalid(
                "cross-domain MI output identity collides with an input",
            ));
        }

        let dependencies = DependencyCollector::default();
        let product_value = dependencies.read(product);
        super::product_domain::validate_product_snapshot(product_value)?;
        let frame_value = dependencies.read(frame);
        validate_frame(product_value, frame_value)?;
        let sensor_params = serde_json::to_value(config).map_err(invalid)?;
        let params = serde_json::json!({
            "product": product.id(),
            "product_domain": &product_value.id,
            "product_version": &product_value.version,
            "product_frame": &frame_value.id,
            "product_frame_version": &frame_value.version,
            "left": &product_value.left,
            "right": &product_value.right,
            "axes": &frame_value.axes,
            "minimum_samples": config.minimum_samples,
            "bins": config.bins,
        });
        let metadata = EmitMetadata {
            id: output_id,
            producer: descriptor.id.clone(),
            algorithm: "equal_width_cross_domain_mutual_information".into(),
            version: descriptor.version.clone(),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp,
            domain_version: None,
            frame_version: None,
            model: None,
        };
        let ctx = ProductMeasureCtx::new(product, frame, samples, &sensor_params, dependencies);
        sensor.measure(&ctx, ctx.calculation_token(metadata))
    }
}

impl crate::Engine {
    /// Detect anonymous bipartite communities from calculated interaction evidence.
    ///
    /// Only product-frame coordinates represented by the bound mutual-information
    /// profile participate. Thresholding is explicit, and connected components keep
    /// left and right identities separate even when their unit IDs are equal.
    #[allow(clippy::too_many_arguments)]
    pub fn measure_cross_domain_communities(
        &self,
        product: &Tracked<ProductDomainSnapshot>,
        frame: &Tracked<ProductMeasurementFrame>,
        mutual_information: &Tracked<CrossDomainMutualInformation>,
        config: CrossDomainCommunityConfig,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<Measurement>> {
        if run_id.trim().is_empty() {
            return Err(invalid(
                "cross-domain communities require a nonempty run ID",
            ));
        }
        crate::require_calculated_evidence(product, "community product domain")?;
        crate::require_calculated_evidence(frame, "community product frame")?;
        crate::require_calculated_evidence(
            mutual_information,
            "community mutual-information profile",
        )?;

        let sensor_id = PluginId::new(COMMUNITY_SENSOR_ID);
        let sensor = self
            .registry()
            .product_sensor(&sensor_id)
            .ok_or_else(|| PluginError::MissingPlugin(sensor_id.clone()))?;
        let descriptor = sensor.descriptor();
        let output_id = DerivedId::new(format!("{run_id}/{}", descriptor.id));
        if product.id() == &output_id
            || frame.id() == &output_id
            || mutual_information.id() == &output_id
        {
            return Err(invalid(
                "cross-domain community output identity collides with an input",
            ));
        }

        let dependencies = DependencyCollector::default();
        let product_value = dependencies.read(product);
        super::product_domain::validate_product_snapshot(product_value)?;
        let frame_value = dependencies.read(frame);
        validate_frame(product_value, frame_value)?;
        let sensor_params = serde_json::to_value(config).map_err(invalid)?;
        let params = serde_json::json!({
            "product": product.id(),
            "product_domain": &product_value.id,
            "product_version": &product_value.version,
            "product_frame": &frame_value.id,
            "product_frame_version": &frame_value.version,
            "left": &product_value.left,
            "right": &product_value.right,
            "mutual_information": mutual_information.id(),
            "minimum_mutual_information_bits": config.minimum_mutual_information_bits,
            "minimum_samples": config.minimum_samples,
        });
        let metadata = EmitMetadata {
            id: output_id,
            producer: descriptor.id.clone(),
            algorithm: "thresholded_bipartite_mutual_information_communities".into(),
            version: descriptor.version.clone(),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp,
            domain_version: None,
            frame_version: None,
            model: None,
        };
        let ctx = ProductMeasureCtx::with_mutual_information(
            product,
            frame,
            mutual_information,
            &sensor_params,
            dependencies,
        );
        sensor.measure(&ctx, ctx.calculation_token(metadata))
    }
}

impl crate::Engine {
    /// Measure scale-free movement agreement on product-frame interaction axes.
    ///
    /// Consecutive transitions come only from the explicit observation sequence.
    /// Sparse endpoints exclude that exact transition without closing the gap.
    #[allow(clippy::too_many_arguments)]
    pub fn measure_cross_domain_interaction_movement(
        &self,
        product: &Tracked<ProductDomainSnapshot>,
        frame: &Tracked<ProductMeasurementFrame>,
        samples: &[Tracked<CrossDomainSample>],
        config: CrossDomainInteractionMovementConfig,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<Measurement>> {
        if run_id.trim().is_empty() {
            return Err(invalid(
                "cross-domain interaction movement requires a nonempty run ID",
            ));
        }
        crate::require_calculated_evidence(product, "interaction-movement product domain")?;
        crate::require_calculated_evidence(frame, "interaction-movement product frame")?;
        if samples.iter().any(|sample| {
            matches!(
                sample.operation(),
                Some(Operation::Experimental | Operation::Interpreted)
            )
        }) {
            return Err(invalid(
                "interaction-movement samples must be inferred, calculated, or restored evidence",
            ));
        }

        let sensor_id = PluginId::new(INTERACTION_MOVEMENT_SENSOR_ID);
        let sensor = self
            .registry()
            .product_sensor(&sensor_id)
            .ok_or_else(|| PluginError::MissingPlugin(sensor_id.clone()))?;
        let descriptor = sensor.descriptor();
        let output_id = DerivedId::new(format!("{run_id}/{}", descriptor.id));
        if product.id() == &output_id
            || frame.id() == &output_id
            || samples.iter().any(|sample| sample.id() == &output_id)
        {
            return Err(invalid(
                "interaction-movement output identity collides with an input",
            ));
        }

        let dependencies = DependencyCollector::default();
        let product_value = dependencies.read(product);
        super::product_domain::validate_product_snapshot(product_value)?;
        let frame_value = dependencies.read(frame);
        validate_frame(product_value, frame_value)?;
        let sensor_params = serde_json::to_value(&config).map_err(invalid)?;
        let params = serde_json::json!({
            "product": product.id(),
            "product_domain": &product_value.id,
            "product_version": &product_value.version,
            "product_frame": &frame_value.id,
            "product_frame_version": &frame_value.version,
            "left": &product_value.left,
            "right": &product_value.right,
            "axes": &frame_value.axes,
            "minimum_transitions": config.minimum_transitions,
            "sequence": &config.sequence,
        });
        let metadata = EmitMetadata {
            id: output_id,
            producer: descriptor.id.clone(),
            algorithm: "signed_consecutive_cross_domain_movement".into(),
            version: descriptor.version.clone(),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp,
            domain_version: None,
            frame_version: None,
            model: None,
        };
        let ctx = ProductMeasureCtx::new(product, frame, samples, &sensor_params, dependencies);
        sensor.measure(&ctx, ctx.calculation_token(metadata))
    }
}

impl crate::Engine {
    /// Measure movement-signature transfer across explicitly mapped product axes.
    ///
    /// Source and target products, frames, and movement profiles remain separate
    /// calculated dependencies. Axis mappings are caller-supplied and one-to-one.
    #[allow(clippy::too_many_arguments)]
    pub fn measure_cross_product_transfer(
        &self,
        source_product: &Tracked<ProductDomainSnapshot>,
        source_frame: &Tracked<ProductMeasurementFrame>,
        source_movement: &Tracked<unclip_measure::CrossDomainInteractionMovement>,
        target_product: &Tracked<ProductDomainSnapshot>,
        target_frame: &Tracked<ProductMeasurementFrame>,
        target_movement: &Tracked<unclip_measure::CrossDomainInteractionMovement>,
        config: CrossProductTransferConfig,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<Measurement>> {
        if run_id.trim().is_empty() {
            return Err(invalid("cross-product transfer requires a nonempty run ID"));
        }
        for (input, kind) in [
            (source_product, "transfer source product domain"),
            (target_product, "transfer target product domain"),
        ] {
            crate::require_calculated_evidence(input, kind)?;
        }
        for (input, kind) in [
            (source_frame, "transfer source product frame"),
            (target_frame, "transfer target product frame"),
        ] {
            crate::require_calculated_evidence(input, kind)?;
        }
        for (input, kind) in [
            (source_movement, "transfer source movement"),
            (target_movement, "transfer target movement"),
        ] {
            crate::require_calculated_evidence(input, kind)?;
        }

        let sensor_id = PluginId::new(CROSS_PRODUCT_TRANSFER_SENSOR_ID);
        let sensor = self
            .registry()
            .cross_product_sensor(&sensor_id)
            .ok_or_else(|| PluginError::MissingPlugin(sensor_id.clone()))?;
        let descriptor = sensor.descriptor();
        let output_id = DerivedId::new(format!("{run_id}/{}", descriptor.id));
        let input_ids = [
            source_product.id(),
            source_frame.id(),
            source_movement.id(),
            target_product.id(),
            target_frame.id(),
            target_movement.id(),
        ];
        if input_ids.iter().any(|id| id.0.trim().is_empty())
            || input_ids.contains(&&output_id)
            || input_ids.iter().copied().collect::<BTreeSet<_>>().len() != input_ids.len()
        {
            return Err(invalid(
                "cross-product transfer requires six unique nonempty input identities distinct from the output",
            ));
        }

        let dependencies = DependencyCollector::default();
        let source_product_value = dependencies.read(source_product);
        super::product_domain::validate_product_snapshot(source_product_value)?;
        let source_frame_value = dependencies.read(source_frame);
        validate_frame(source_product_value, source_frame_value)?;
        let target_product_value = dependencies.read(target_product);
        super::product_domain::validate_product_snapshot(target_product_value)?;
        let target_frame_value = dependencies.read(target_frame);
        validate_frame(target_product_value, target_frame_value)?;
        let sensor_params = serde_json::to_value(&config).map_err(invalid)?;
        let params = serde_json::json!({
            "source_product": source_product.id(),
            "source_product_domain": &source_product_value.id,
            "source_product_version": &source_product_value.version,
            "source_frame": &source_frame_value.id,
            "source_frame_version": &source_frame_value.version,
            "source_movement": source_movement.id(),
            "source_left": &source_product_value.left,
            "source_right": &source_product_value.right,
            "target_product": target_product.id(),
            "target_product_domain": &target_product_value.id,
            "target_product_version": &target_product_value.version,
            "target_frame": &target_frame_value.id,
            "target_frame_version": &target_frame_value.version,
            "target_movement": target_movement.id(),
            "target_left": &target_product_value.left,
            "target_right": &target_product_value.right,
            "mappings": &config.mappings,
            "minimum_transitions": config.minimum_transitions,
        });
        let metadata = EmitMetadata {
            id: output_id,
            producer: descriptor.id.clone(),
            algorithm: "mapped_cross_product_movement_transfer".into(),
            version: descriptor.version.clone(),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp,
            domain_version: None,
            frame_version: None,
            model: None,
        };
        let ctx = CrossProductMeasureCtx::new(
            source_product,
            source_frame,
            source_movement,
            target_product,
            target_frame,
            target_movement,
            &sensor_params,
            dependencies,
        );
        sensor.measure(&ctx, ctx.calculation_token(metadata))
    }
}
