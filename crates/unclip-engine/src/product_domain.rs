//! Lazy, evidence-driven materialization of cross-domain interactions.

use std::collections::BTreeSet;

use unclip_domain::{
    DomainSnapshot, ProductDomainId, ProductDomainInput, ProductDomainSnapshot,
    ProductDomainVersion, ProductFrameAxis, ProductFrameId, ProductFrameVersion,
    ProductInteraction, ProductMeasurementFrame,
};
use unclip_epistemic::{
    hash_params, Calculated, CalculationToken, DependencyCollector, DerivedId, EmitMetadata,
    PluginId, Timestamp, Tracked,
};
use unclip_plugin::{PluginError, Result};

fn invalid(message: impl Into<String>) -> PluginError {
    PluginError::Message(message.into())
}

fn ordered_unique<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

impl crate::Engine {
    /// Materialize only explicitly observed or required interactions between two domains.
    ///
    /// The output retains immutable input identities and versions. It contains no
    /// copied units or relations, so it remains distinct from an ordinary domain union.
    #[allow(clippy::too_many_arguments)]
    pub fn materialize_product_domain(
        &self,
        left: &Tracked<DomainSnapshot>,
        right: &Tracked<DomainSnapshot>,
        interactions: &[Tracked<ProductInteraction>],
        id: ProductDomainId,
        version: ProductDomainVersion,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<ProductDomainSnapshot>> {
        if run_id.trim().is_empty() || id.0.trim().is_empty() || version.0.trim().is_empty() {
            return Err(invalid(
                "product-domain materialization requires nonempty product, version, and run identities",
            ));
        }
        let output_id = DerivedId::new(format!("{run_id}/product-domain"));
        if &output_id == left.id() || &output_id == right.id() {
            return Err(invalid(
                "product-domain output identity collides with a domain input",
            ));
        }

        let dependencies = DependencyCollector::default();
        let left_domain = dependencies.read(left);
        let right_domain = dependencies.read(right);
        super::domain_null::validate(left_domain)?;
        super::domain_null::validate(right_domain)?;
        if left.id().0.trim().is_empty()
            || right.id().0.trim().is_empty()
            || left.id() == right.id()
            || left_domain.id == right_domain.id
            || left_domain.id.0.trim().is_empty()
            || right_domain.id.0.trim().is_empty()
            || left_domain.version.0.trim().is_empty()
            || right_domain.version.0.trim().is_empty()
        {
            return Err(invalid(
                "product domains require two distinct immutable domain inputs",
            ));
        }

        let mut materialized = Vec::with_capacity(interactions.len());
        let mut pairs = BTreeSet::new();
        let mut evidence_ids = BTreeSet::new();
        for interaction in interactions {
            if interaction.id().0.trim().is_empty()
                || interaction.id() == left.id()
                || interaction.id() == right.id()
                || interaction.id() == &output_id
                || !evidence_ids.insert(interaction.id().clone())
            {
                return Err(invalid(
                    "product interaction evidence requires distinct nonempty identities",
                ));
            }
            let interaction = dependencies.read(interaction);
            let support_count = interaction.observations.len() + interaction.requirements.len();
            let unique_support = interaction
                .observations
                .iter()
                .chain(&interaction.requirements)
                .collect::<BTreeSet<_>>();
            if interaction.left.0.trim().is_empty()
                || interaction.right.0.trim().is_empty()
                || !left_domain.units.contains_key(&interaction.left)
                || !right_domain.units.contains_key(&interaction.right)
                || interaction.observations.is_empty() && interaction.requirements.is_empty()
                || !ordered_unique(&interaction.observations)
                || !ordered_unique(&interaction.requirements)
                || interaction
                    .observations
                    .iter()
                    .chain(&interaction.requirements)
                    .any(|evidence| evidence.0.trim().is_empty())
                || unique_support.len() != support_count
                || !pairs.insert((interaction.left.clone(), interaction.right.clone()))
            {
                return Err(invalid(
                    "product interactions require one unique existing left/right pair with ordered observed or required evidence",
                ));
            }
            materialized.push(interaction.clone());
        }
        materialized.sort_by(|a, b| {
            (&a.left, &a.right, &a.observations, &a.requirements).cmp(&(
                &b.left,
                &b.right,
                &b.observations,
                &b.requirements,
            ))
        });

        let left_input = ProductDomainInput {
            domain: left_domain.id.clone(),
            version: left_domain.version.clone(),
        };
        let right_input = ProductDomainInput {
            domain: right_domain.id.clone(),
            version: right_domain.version.clone(),
        };
        let params = serde_json::json!({
            "product_domain": &id,
            "product_version": &version,
            "left": {
                "derived": left.id(),
                "domain": &left_input.domain,
                "version": &left_input.version,
            },
            "right": {
                "derived": right.id(),
                "domain": &right_input.domain,
                "version": &right_input.version,
            },
            "materialized_interactions": materialized.len(),
        });
        let token = CalculationToken::from_harness(
            EmitMetadata {
                id: output_id,
                producer: PluginId::new("calculate.product-domain"),
                algorithm: "lazy_observation_driven_product".into(),
                version: semver::Version::new(0, 1, 0),
                params_hash: hash_params(&params),
                params,
                source: None,
                timestamp,
                domain_version: None,
                frame_version: None,
                model: None,
            },
            dependencies,
        );
        Ok(token.emit(ProductDomainSnapshot {
            id,
            version,
            left: left_input,
            right: right_input,
            interactions: materialized,
        }))
    }
}

fn validate_product_snapshot(product: &ProductDomainSnapshot) -> Result<()> {
    if product.id.0.trim().is_empty()
        || product.version.0.trim().is_empty()
        || product.left.domain.0.trim().is_empty()
        || product.right.domain.0.trim().is_empty()
        || product.left.version.0.trim().is_empty()
        || product.right.version.0.trim().is_empty()
        || product.left.domain == product.right.domain
    {
        return Err(invalid(
            "product frame requires a valid product with two explicit immutable inputs",
        ));
    }

    let mut previous = None;
    for interaction in &product.interactions {
        let support_count = interaction.observations.len() + interaction.requirements.len();
        let unique_support = interaction
            .observations
            .iter()
            .chain(&interaction.requirements)
            .collect::<BTreeSet<_>>();
        let coordinate = (interaction.left.clone(), interaction.right.clone());
        if interaction.left.0.trim().is_empty()
            || interaction.right.0.trim().is_empty()
            || interaction.observations.is_empty() && interaction.requirements.is_empty()
            || !ordered_unique(&interaction.observations)
            || !ordered_unique(&interaction.requirements)
            || unique_support.len() != support_count
            || interaction
                .observations
                .iter()
                .chain(&interaction.requirements)
                .any(|evidence| evidence.0.trim().is_empty())
            || previous.as_ref().is_some_and(|prior| prior >= &coordinate)
        {
            return Err(invalid(
                "product frame requires canonical materialized interaction evidence",
            ));
        }
        previous = Some(coordinate);
    }
    Ok(())
}

impl crate::Engine {
    /// Define an explicitly versioned coordinate frame over materialized product interactions.
    pub fn create_product_frame(
        &self,
        product: &Tracked<ProductDomainSnapshot>,
        axes: &[ProductFrameAxis],
        id: ProductFrameId,
        version: ProductFrameVersion,
        run_id: &str,
        timestamp: Timestamp,
    ) -> Result<Calculated<ProductMeasurementFrame>> {
        if run_id.trim().is_empty() || id.0.trim().is_empty() || version.0.trim().is_empty() {
            return Err(invalid(
                "product frame requires nonempty frame, version, and run identities",
            ));
        }
        let output_id = DerivedId::new(format!("{run_id}/product-frame"));
        if product.id().0.trim().is_empty() || product.id() == &output_id {
            return Err(invalid(
                "product frame requires a distinct nonempty product input identity",
            ));
        }
        super::require_calculated_evidence(product, "product domain")?;

        let dependencies = DependencyCollector::default();
        let product_snapshot = dependencies.read(product);
        validate_product_snapshot(product_snapshot)?;
        let materialized = product_snapshot
            .interactions
            .iter()
            .map(|interaction| (interaction.left.clone(), interaction.right.clone()))
            .collect::<BTreeSet<_>>();
        let mut selected = BTreeSet::new();
        for axis in axes {
            let coordinate = (axis.left.clone(), axis.right.clone());
            if axis.left.0.trim().is_empty()
                || axis.right.0.trim().is_empty()
                || axis
                    .label
                    .as_ref()
                    .is_some_and(|label| label.trim().is_empty())
                || !materialized.contains(&coordinate)
                || !selected.insert(coordinate)
            {
                return Err(invalid(
                    "product frame axes must uniquely reference materialized interactions",
                ));
            }
        }

        let params = serde_json::json!({
            "product": product.id(),
            "product_domain": &product_snapshot.id,
            "product_version": &product_snapshot.version,
            "left": &product_snapshot.left,
            "right": &product_snapshot.right,
            "product_frame": &id,
            "product_frame_version": &version,
            "axes": axes,
        });
        let token = CalculationToken::from_harness(
            EmitMetadata {
                id: output_id,
                producer: PluginId::new("calculate.product-frame"),
                algorithm: "explicit_product_interaction_frame".into(),
                version: semver::Version::new(0, 1, 0),
                params_hash: hash_params(&params),
                params,
                source: None,
                timestamp,
                domain_version: None,
                frame_version: None,
                model: None,
            },
            dependencies,
        );
        Ok(token.emit(ProductMeasurementFrame {
            id,
            version,
            product: product_snapshot.id.clone(),
            product_version: product_snapshot.version.clone(),
            left: product_snapshot.left.clone(),
            right: product_snapshot.right.clone(),
            axes: axes.to_vec(),
        }))
    }
}
