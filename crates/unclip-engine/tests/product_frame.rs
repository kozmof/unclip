use std::collections::BTreeMap;

use serde_json::json;
use unclip_domain::{
    DomainId, DomainSnapshot, ProductDomainId, ProductDomainSnapshot, ProductDomainVersion,
    ProductFrameAxis, ProductFrameId, ProductFrameVersion, ProductInteraction, Unit, UnitId,
    UnitKind,
};
use unclip_engine::Engine;
use unclip_epistemic::{
    hash_params, Calculated, DependencyCollector, DerivedId, DomainVersion, EmitMetadata,
    InferenceToken, PluginId, Timestamp, Tracked,
};

fn domain(id: &str, version: &str, units: &[&str]) -> DomainSnapshot {
    DomainSnapshot {
        id: DomainId::new(id),
        version: DomainVersion::new(version),
        units: units
            .iter()
            .map(|id| {
                (
                    UnitId::new(*id),
                    Unit {
                        id: UnitId::new(*id),
                        kind: UnitKind::AtomicMeaning,
                        label: Some((*id).into()),
                        properties: BTreeMap::new(),
                    },
                )
            })
            .collect(),
        relations: BTreeMap::new(),
    }
}

fn interaction(
    id: &str,
    left: &str,
    right: &str,
    observations: &[&str],
    requirements: &[&str],
) -> Tracked<ProductInteraction> {
    Tracked::from_recorded(
        DerivedId::new(id),
        ProductInteraction {
            left: UnitId::new(left),
            right: UnitId::new(right),
            observations: observations.iter().map(|id| DerivedId::new(*id)).collect(),
            requirements: requirements.iter().map(|id| DerivedId::new(*id)).collect(),
        },
    )
}

fn product() -> (Engine, Calculated<ProductDomainSnapshot>) {
    let engine = Engine::with_builtins().unwrap();
    let product = engine
        .materialize_product_domain(
            &Tracked::from_recorded(
                DerivedId::new("coffee@7"),
                domain("coffee", "7", &["presentation", "social"]),
            ),
            &Tracked::from_recorded(
                DerivedId::new("photo@3"),
                domain("photo", "3", &["composition", "sharing"]),
            ),
            &[
                interaction(
                    "presentation-composition",
                    "presentation",
                    "composition",
                    &["observation-1", "observation-2"],
                    &[],
                ),
                interaction(
                    "social-sharing",
                    "social",
                    "sharing",
                    &[],
                    &["measurement-requirement"],
                ),
            ],
            ProductDomainId::new("coffee-x-photo"),
            ProductDomainVersion::new("1"),
            "coffee-photo",
            Timestamp::new("now"),
        )
        .unwrap();
    (engine, product)
}

fn axis(left: &str, right: &str, label: Option<&str>) -> ProductFrameAxis {
    ProductFrameAxis {
        left: UnitId::new(left),
        right: UnitId::new(right),
        label: label.map(str::to_owned),
    }
}

#[test]
fn product_frame_pins_product_inputs_versions_and_axis_order() {
    let (engine, product) = product();
    let product_input = Tracked::from(&product);
    let axes = [
        axis("social", "sharing", Some("sharing context")),
        axis("presentation", "composition", None),
    ];
    let frame = engine
        .create_product_frame(
            &product_input,
            &axes,
            ProductFrameId::new("coffee-photo.general"),
            ProductFrameVersion::new("4"),
            "coffee-photo-frame",
            Timestamp::new("now"),
        )
        .unwrap();
    let replay = engine
        .create_product_frame(
            &product_input,
            &axes,
            ProductFrameId::new("coffee-photo.general"),
            ProductFrameVersion::new("4"),
            "coffee-photo-frame",
            Timestamp::new("now"),
        )
        .unwrap();

    assert_eq!(frame, replay);
    assert_eq!(
        frame.value().id,
        ProductFrameId::new("coffee-photo.general")
    );
    assert_eq!(frame.value().version, ProductFrameVersion::new("4"));
    assert_eq!(
        frame.value().product,
        ProductDomainId::new("coffee-x-photo")
    );
    assert_eq!(
        frame.value().product_version,
        ProductDomainVersion::new("1")
    );
    assert_eq!(frame.value().left.domain, DomainId::new("coffee"));
    assert_eq!(frame.value().left.version, DomainVersion::new("7"));
    assert_eq!(frame.value().right.domain, DomainId::new("photo"));
    assert_eq!(frame.value().right.version, DomainVersion::new("3"));
    assert_eq!(frame.value().axes, axes);
    assert_eq!(frame.provenance().inputs, vec![product.id().clone()]);
    assert_eq!(
        frame.provenance().producer,
        PluginId::new("calculate.product-frame")
    );
    assert_eq!(
        frame.provenance().params["product_frame_version"],
        json!("4")
    );
    assert_eq!(frame.provenance().params["left"]["version"], json!("7"));
    assert_eq!(frame.provenance().params["right"]["version"], json!("3"));

    let encoded = serde_json::to_value(frame.value()).unwrap();
    assert_eq!(encoded["product"], "coffee-x-photo");
    assert_eq!(encoded["product_version"], "1");
    assert!(encoded.get("domain").is_none());
    assert_eq!(
        serde_json::from_value::<unclip_domain::ProductMeasurementFrame>(encoded).unwrap(),
        frame.value().clone()
    );
}

#[test]
fn product_frame_rejects_unmaterialized_duplicate_or_malformed_axes() {
    let (engine, product) = product();
    let product = Tracked::from(&product);
    let create = |axes: &[ProductFrameAxis]| {
        engine.create_product_frame(
            &product,
            axes,
            ProductFrameId::new("frame"),
            ProductFrameVersion::new("1"),
            "frame-run",
            Timestamp::new("now"),
        )
    };

    assert!(create(&[axis("presentation", "sharing", None)]).is_err());
    assert!(create(&[
        axis("social", "sharing", None),
        axis("social", "sharing", Some("duplicate")),
    ])
    .is_err());
    assert!(create(&[axis("social", "sharing", Some("  "))]).is_err());
    assert!(engine
        .create_product_frame(
            &product,
            &[],
            ProductFrameId::new(""),
            ProductFrameVersion::new("1"),
            "frame-run",
            Timestamp::new("now"),
        )
        .is_err());
}

#[test]
fn product_frame_requires_valid_calculated_product_evidence() {
    let (engine, product) = product();
    let mut malformed = product.value().clone();
    malformed.version = ProductDomainVersion::new("");
    assert!(engine
        .create_product_frame(
            &Tracked::from_recorded(DerivedId::new("stored-malformed"), malformed),
            &[],
            ProductFrameId::new("frame"),
            ProductFrameVersion::new("1"),
            "stored-frame",
            Timestamp::new("now"),
        )
        .is_err());

    let params = json!({"fixture":"inferred-product"});
    let inferred = InferenceToken::from_harness(
        EmitMetadata {
            id: DerivedId::new("inferred-product"),
            producer: PluginId::new("infer.fixture"),
            algorithm: "fixture".into(),
            version: "0.1.0".parse().unwrap(),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp: Timestamp::new("now"),
            domain_version: None,
            frame_version: None,
            model: None,
        },
        DependencyCollector::default(),
    )
    .emit(product.value().clone());
    assert!(engine
        .create_product_frame(
            &Tracked::from(&inferred),
            &[],
            ProductFrameId::new("frame"),
            ProductFrameVersion::new("1"),
            "inferred-frame",
            Timestamp::new("now"),
        )
        .is_err());
}
