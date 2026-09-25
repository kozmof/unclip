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
    CrossDomainMutualInformation, CrossDomainMutualInformationConfig, CrossDomainSample,
    MeasurementValue, Reading,
};
use unclip_observe::ObservationId;
use unclip_plugin::conformance;

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
            &Tracked::from_recorded(
                DerivedId::new("coffee@7"),
                domain("coffee", "7", &["presentation", "social"]),
            ),
            &Tracked::from_recorded(
                DerivedId::new("photo@3"),
                domain("photo", "3", &["composition", "sharing"]),
            ),
            &[
                interaction("presentation-composition", "presentation", "composition"),
                interaction("social-sharing", "social", "sharing"),
            ],
            ProductDomainId::new("coffee-x-photo"),
            ProductDomainVersion::new("2"),
            "product-run",
            Timestamp::new("2026-09-24T00:00:00Z"),
        )
        .unwrap();
    let frame = engine
        .create_product_frame(
            &Tracked::from(&product),
            &[
                ProductFrameAxis {
                    left: UnitId::new("presentation"),
                    right: UnitId::new("composition"),
                    label: Some("visual form".into()),
                },
                ProductFrameAxis {
                    left: UnitId::new("social"),
                    right: UnitId::new("sharing"),
                    label: Some("social exchange".into()),
                },
            ],
            ProductFrameId::new("coffee-photo.general"),
            ProductFrameVersion::new("5"),
            "frame-run",
            Timestamp::new("2026-09-24T00:00:00Z"),
        )
        .unwrap();
    (engine, product, frame)
}

fn sample(id: &str, left: &[(&str, f64)], right: &[(&str, f64)]) -> Tracked<CrossDomainSample> {
    Tracked::from_recorded(
        DerivedId::new(format!("evidence-{id}")),
        CrossDomainSample {
            observation: ObservationId::new(id),
            left: left
                .iter()
                .map(|(unit, value)| (UnitId::new(*unit), *value))
                .collect(),
            right: right
                .iter()
                .map(|(unit, value)| (UnitId::new(*unit), *value))
                .collect(),
        },
    )
}

fn config() -> CrossDomainMutualInformationConfig {
    CrossDomainMutualInformationConfig {
        minimum_samples: NonZeroUsize::new(2).unwrap(),
        bins: NonZeroUsize::new(2).unwrap(),
    }
}

#[test]
fn engine_cross_domain_mi_preserves_axes_versions_and_dependencies() {
    let (engine, product, frame) = product_and_frame();
    let samples = vec![
        sample(
            "a",
            &[("presentation", 0.0), ("social", 0.0)],
            &[("composition", 0.0), ("sharing", 0.0)],
        ),
        sample(
            "b",
            &[("presentation", 0.0), ("social", 0.0)],
            &[("composition", 0.0), ("sharing", 1.0)],
        ),
        sample(
            "c",
            &[("presentation", 1.0), ("social", 1.0)],
            &[("composition", 1.0), ("sharing", 0.0)],
        ),
        sample(
            "d",
            &[("presentation", 1.0), ("social", 1.0)],
            &[("composition", 1.0), ("sharing", 1.0)],
        ),
    ];
    let measure = |samples: &[Tracked<CrossDomainSample>]| {
        engine.measure_cross_domain_mutual_information(
            &Tracked::from(&product),
            &Tracked::from(&frame),
            samples,
            config(),
            "mi-run",
            Timestamp::new("2026-09-24T00:00:01Z"),
        )
    };
    let sensor = engine
        .registry()
        .product_sensor(&PluginId::new("sensor.cross-domain-mutual-information"))
        .unwrap();
    conformance::assert_product_sensor(sensor.as_ref(), || measure(&samples));
    let result = measure(&samples).unwrap();
    let reversed = samples.iter().cloned().rev().collect::<Vec<_>>();
    assert_eq!(measure(&reversed).unwrap(), result);

    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = &result.value().reading
    else {
        panic!("expected structured cross-domain MI")
    };
    let analysis: CrossDomainMutualInformation = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(analysis.axes.len(), 2);
    assert_eq!(analysis.axes[0].left, UnitId::new("presentation"));
    assert_eq!(analysis.axes[0].right, UnitId::new("composition"));
    assert_eq!(analysis.axes[0].mutual_information_bits, 1.0);
    assert_eq!(analysis.axes[1].mutual_information_bits, 0.0);
    assert!(analysis.unassessed_axes.is_empty());
    assert_eq!(analysis.observation_count, 4);
    assert_eq!(analysis.requested_bins, 2);

    assert_eq!(
        result.value().sensor,
        PluginId::new("sensor.cross-domain-mutual-information")
    );
    assert_eq!(result.value().sample_count, None);
    assert_eq!(result.provenance().producer, result.value().sensor);
    assert_eq!(
        result.provenance().algorithm,
        "equal_width_cross_domain_mutual_information"
    );
    assert_eq!(result.provenance().params["product_version"], "2");
    assert_eq!(result.provenance().params["product_frame_version"], "5");
    assert_eq!(result.provenance().params["left"]["version"], "7");
    assert_eq!(result.provenance().params["right"]["version"], "3");
    assert_eq!(result.provenance().params["bins"], 2);

    let expected_inputs = samples
        .iter()
        .map(|sample| sample.id().clone())
        .chain([product.id().clone(), frame.id().clone()])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    assert_eq!(result.provenance().inputs, expected_inputs);
}

#[test]
fn engine_cross_domain_mi_keeps_assessed_and_sparse_axes_separate() {
    let (engine, product, frame) = product_and_frame();
    let samples = [
        sample(
            "a",
            &[("presentation", 0.0), ("social", 0.0)],
            &[("composition", 0.0), ("sharing", 0.0)],
        ),
        sample(
            "b",
            &[("presentation", 1.0)],
            &[("composition", 1.0), ("sharing", 1.0)],
        ),
    ];
    let result = engine
        .measure_cross_domain_mutual_information(
            &Tracked::from(&product),
            &Tracked::from(&frame),
            &samples,
            config(),
            "sparse-mi",
            Timestamp::new("now"),
        )
        .unwrap();
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = &result.value().reading
    else {
        panic!("one assessed axis must retain the structured value")
    };
    let analysis: CrossDomainMutualInformation = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(analysis.axes.len(), 1);
    assert_eq!(analysis.unassessed_axes.len(), 1);
    assert_eq!(analysis.unassessed_axes[0].have, 1);
    assert_eq!(
        result.value().context.values["unassessed_axes"][0]["left"],
        "social"
    );
}

#[test]
fn engine_cross_domain_mi_reports_an_all_sparse_profile() {
    let (engine, product, frame) = product_and_frame();
    let result = engine
        .measure_cross_domain_mutual_information(
            &Tracked::from(&product),
            &Tracked::from(&frame),
            &[sample(
                "only",
                &[("presentation", 0.0), ("social", 0.0)],
                &[("composition", 0.0), ("sharing", 0.0)],
            )],
            config(),
            "insufficient-mi",
            Timestamp::new("now"),
        )
        .unwrap();
    assert_eq!(
        result.value().reading,
        Reading::InsufficientEvidence { have: 1, need: 2 }
    );
    assert_eq!(result.value().sample_count, Some(1));
}
