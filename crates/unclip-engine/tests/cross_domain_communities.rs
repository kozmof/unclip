use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use unclip_domain::{
    DomainId, DomainSnapshot, ProductDomainId, ProductDomainSnapshot, ProductDomainVersion,
    ProductFrameAxis, ProductFrameId, ProductFrameVersion, ProductInteraction,
    ProductMeasurementFrame, Unit, UnitId, UnitKind,
};
use unclip_engine::Engine;
use unclip_epistemic::{Calculated, DerivedId, DomainVersion, PluginId, Timestamp, Tracked};
use unclip_measure::{
    CrossDomainCommunityConfig, CrossDomainCommunityDetection, CrossDomainCommunityMember,
    CrossDomainMutualInformation, CrossDomainMutualInformationConfig, CrossDomainSample,
    MeasurementValue, ProductSide, Reading,
};
use unclip_observe::ObservationId;

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

fn interaction(id: &str, left: &str, right: &str) -> Tracked<ProductInteraction> {
    Tracked::from_recorded(
        DerivedId::new(id),
        ProductInteraction {
            left: UnitId::new(left),
            right: UnitId::new(right),
            observations: vec![DerivedId::new(format!("{id}-observation"))],
            requirements: vec![],
        },
    )
}

fn product_and_frame() -> (
    Engine,
    Calculated<ProductDomainSnapshot>,
    Calculated<ProductMeasurementFrame>,
) {
    let engine = Engine::with_builtins().unwrap();
    let product = engine
        .materialize_product_domain(
            &Tracked::from_recorded(DerivedId::new("left@3"), domain("left", "3", &["a", "b"])),
            &Tracked::from_recorded(DerivedId::new("right@4"), domain("right", "4", &["x", "y"])),
            &[
                interaction("a-x", "a", "x"),
                interaction("b-x", "b", "x"),
                interaction("b-y", "b", "y"),
            ],
            ProductDomainId::new("left-x-right"),
            ProductDomainVersion::new("6"),
            "product-run",
            Timestamp::new("2026-09-24T00:00:00Z"),
        )
        .unwrap();
    let frame = engine
        .create_product_frame(
            &Tracked::from(&product),
            &[
                ProductFrameAxis {
                    left: UnitId::new("a"),
                    right: UnitId::new("x"),
                    label: None,
                },
                ProductFrameAxis {
                    left: UnitId::new("b"),
                    right: UnitId::new("x"),
                    label: None,
                },
                ProductFrameAxis {
                    left: UnitId::new("b"),
                    right: UnitId::new("y"),
                    label: None,
                },
            ],
            ProductFrameId::new("left-right.interactions"),
            ProductFrameVersion::new("8"),
            "frame-run",
            Timestamp::new("2026-09-24T00:00:00Z"),
        )
        .unwrap();
    (engine, product, frame)
}

fn sample(id: &str, common: f64, independent: f64) -> Tracked<CrossDomainSample> {
    Tracked::from_recorded(
        DerivedId::new(format!("evidence-{id}")),
        CrossDomainSample {
            observation: ObservationId::new(id),
            left: BTreeMap::from([(UnitId::new("a"), common), (UnitId::new("b"), common)]),
            right: BTreeMap::from([(UnitId::new("x"), common), (UnitId::new("y"), independent)]),
        },
    )
}

fn mutual_information(
    engine: &Engine,
    product: &Calculated<ProductDomainSnapshot>,
    frame: &Calculated<ProductMeasurementFrame>,
) -> (
    Calculated<unclip_measure::Measurement>,
    CrossDomainMutualInformation,
) {
    let samples = [
        sample("one", 0.0, 0.0),
        sample("two", 0.0, 1.0),
        sample("three", 1.0, 0.0),
        sample("four", 1.0, 1.0),
    ];
    let result = engine
        .measure_cross_domain_mutual_information(
            &Tracked::from(product),
            &Tracked::from(frame),
            &samples,
            CrossDomainMutualInformationConfig {
                minimum_samples: NonZeroUsize::new(2).unwrap(),
                bins: NonZeroUsize::new(2).unwrap(),
            },
            "mi-run",
            Timestamp::new("2026-09-24T00:00:01Z"),
        )
        .unwrap();
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = &result.value().reading
    else {
        panic!("expected structured mutual information")
    };
    let profile = serde_json::from_value(value.clone()).unwrap();
    (result, profile)
}

fn community_config(samples: usize) -> CrossDomainCommunityConfig {
    CrossDomainCommunityConfig {
        minimum_mutual_information_bits: 0.5,
        minimum_samples: NonZeroUsize::new(samples).unwrap(),
    }
}

fn member(side: ProductSide, unit: &str) -> CrossDomainCommunityMember {
    CrossDomainCommunityMember {
        side,
        unit: UnitId::new(unit),
    }
}

#[test]
fn engine_communities_preserve_bipartite_identity_versions_and_provenance() {
    let (engine, product, frame) = product_and_frame();
    let (mi, profile) = mutual_information(&engine, &product, &frame);
    let tracked_mi = Tracked::from_derived(&mi, profile);
    let result = engine
        .measure_cross_domain_communities(
            &Tracked::from(&product),
            &Tracked::from(&frame),
            &tracked_mi,
            community_config(2),
            "community-run",
            Timestamp::new("2026-09-24T00:00:02Z"),
        )
        .unwrap();

    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = &result.value().reading
    else {
        panic!("expected structured communities")
    };
    let detection: CrossDomainCommunityDetection = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        detection.communities,
        vec![
            vec![
                member(ProductSide::Left, "a"),
                member(ProductSide::Left, "b"),
                member(ProductSide::Right, "x"),
            ],
            vec![member(ProductSide::Right, "y")],
        ]
    );
    assert_eq!(detection.interaction_count, 3);
    assert_eq!(detection.assessed_interactions, 3);
    assert_eq!(detection.qualifying_interactions, 2);
    assert_eq!(detection.binding.product_version.0, "6");
    assert_eq!(detection.binding.frame_version.0, "8");
    assert_eq!(detection.binding.left.version.0, "3");
    assert_eq!(detection.binding.right.version.0, "4");

    assert_eq!(
        result.value().sensor,
        PluginId::new("sensor.cross-domain-communities")
    );
    assert_eq!(result.value().sample_count, None);
    assert_eq!(result.provenance().producer, result.value().sensor);
    assert_eq!(
        result.provenance().algorithm,
        "thresholded_bipartite_mutual_information_communities"
    );
    assert_eq!(result.provenance().params["product_version"], "6");
    assert_eq!(result.provenance().params["product_frame_version"], "8");
    assert_eq!(result.provenance().params["mutual_information"], mi.id().0);
    assert_eq!(
        result.provenance().inputs,
        [product.id().clone(), frame.id().clone(), mi.id().clone()]
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    );
}

#[test]
fn engine_communities_keep_a_higher_sample_floor_typed() {
    let (engine, product, frame) = product_and_frame();
    let (mi, profile) = mutual_information(&engine, &product, &frame);
    let result = engine
        .measure_cross_domain_communities(
            &Tracked::from(&product),
            &Tracked::from(&frame),
            &Tracked::from_derived(&mi, profile),
            community_config(5),
            "sparse-community-run",
            Timestamp::new("2026-09-24T00:00:02Z"),
        )
        .unwrap();
    assert_eq!(
        result.value().reading,
        Reading::InsufficientEvidence { have: 4, need: 5 }
    );
    assert_eq!(result.value().sample_count, Some(4));
    assert_eq!(
        result.value().context.values["unassessed_interactions"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn engine_communities_reject_mismatched_or_incomplete_mi_profiles() {
    let (engine, product, frame) = product_and_frame();
    let (mi, mut profile) = mutual_information(&engine, &product, &frame);
    profile.binding.frame_version = ProductFrameVersion::new("wrong");
    let error = engine
        .measure_cross_domain_communities(
            &Tracked::from(&product),
            &Tracked::from(&frame),
            &Tracked::from_derived(&mi, profile),
            community_config(2),
            "wrong-binding",
            Timestamp::new("2026-09-24T00:00:02Z"),
        )
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("exact product and frame versions"));

    let (_, mut profile) = mutual_information(&engine, &product, &frame);
    profile.axes.pop();
    let error = engine
        .measure_cross_domain_communities(
            &Tracked::from(&product),
            &Tracked::from(&frame),
            &Tracked::from_derived(&mi, profile),
            community_config(2),
            "missing-axis",
            Timestamp::new("2026-09-24T00:00:02Z"),
        )
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("every and only product-frame axis"));
}
