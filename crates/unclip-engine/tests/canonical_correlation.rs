use std::collections::{BTreeMap, BTreeSet};

use unclip_domain::{
    DomainId, DomainSnapshot, ProductDomainId, ProductDomainSnapshot, ProductDomainVersion,
    ProductFrameAxis, ProductFrameId, ProductFrameVersion, ProductInteraction,
    ProductMeasurementFrame, Unit, UnitId, UnitKind,
};
use unclip_engine::{create_product_frame, materialize_product_domain, CrossDomainRun, Engine};
use unclip_epistemic::{Calculated, DerivedId, DomainVersion, PluginId, Timestamp, Tracked};
use unclip_measure::{
    CanonicalCorrelationAnalysis, CanonicalCorrelationConfig, CrossDomainSample, MeasurementValue,
    Reading,
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
    let product = materialize_product_domain(
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
    let frame = create_product_frame(
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

fn coupled_samples() -> Vec<Tracked<CrossDomainSample>> {
    let mut samples = (-3..=3)
        .map(|x| {
            let value = f64::from(x);
            let alternating = if x % 2 == 0 { 1.0 } else { -1.0 };
            sample(
                &format!("sample-{x:+02}"),
                &[("presentation", value), ("social", alternating)],
                &[
                    ("composition", 2.0 * value + alternating),
                    ("sharing", -value + 3.0 * alternating),
                ],
            )
        })
        .collect::<Vec<_>>();
    samples.push(sample(
        "sparse",
        &[("presentation", 4.0)],
        &[("composition", 8.0), ("sharing", -4.0)],
    ));
    samples
}

#[test]
fn engine_cca_binds_product_versions_and_tracks_every_input() {
    let (engine, product, frame) = product_and_frame();
    let samples = coupled_samples();
    let config = CanonicalCorrelationConfig {
        regularization: 0.0,
        ..CanonicalCorrelationConfig::default()
    };
    let measure = |samples: &[Tracked<CrossDomainSample>]| {
        engine.measure_canonical_correlation(
            CrossDomainRun {
                product: &Tracked::from(&product),
                frame: &Tracked::from(&frame),
                run_id: "cca-run",
                timestamp: Timestamp::new("2026-09-24T00:00:01Z"),
            },
            samples,
            config,
        )
    };
    let sensor = engine
        .registry()
        .product_sensor(&PluginId::new("sensor.canonical-correlation"))
        .unwrap();
    conformance::assert_product_sensor(sensor.as_ref(), || measure(&samples));
    let result = measure(&samples).unwrap();
    let reversed = samples.iter().cloned().rev().collect::<Vec<_>>();
    assert_eq!(measure(&reversed).unwrap(), result);

    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = &result.value().reading
    else {
        panic!("expected structured CCA measurement")
    };
    let analysis: CanonicalCorrelationAnalysis = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        analysis.left_units,
        vec![UnitId::new("presentation"), UnitId::new("social")]
    );
    assert_eq!(
        analysis.right_units,
        vec![UnitId::new("composition"), UnitId::new("sharing")]
    );
    assert_eq!(analysis.sample_count, 7);
    assert_eq!(
        analysis.excluded_observations,
        vec![ObservationId::new("sparse")]
    );
    assert_eq!(analysis.modes.len(), 2);
    assert!(analysis
        .modes
        .iter()
        .all(|mode| (mode.correlation - 1.0).abs() < 1e-10));

    assert_eq!(
        result.value().sensor,
        PluginId::new("sensor.canonical-correlation")
    );
    assert_eq!(result.value().sample_count, Some(7));
    assert_eq!(result.provenance().producer, result.value().sensor);
    assert_eq!(
        result.provenance().algorithm.as_ref(),
        "regularized_canonical_correlation"
    );
    assert_eq!(result.provenance().params["product_version"], "2");
    assert_eq!(result.provenance().params["product_frame_version"], "5");
    assert_eq!(result.provenance().params["left"]["version"], "7");
    assert_eq!(result.provenance().params["right"]["version"], "3");
    assert_eq!(
        result.value().context.values["excluded_observations"],
        serde_json::json!(["sparse"])
    );

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
fn engine_cca_preserves_sparse_and_no_variation_states() {
    let (engine, product, frame) = product_and_frame();
    let calculate = |samples: &[Tracked<CrossDomainSample>]| {
        engine
            .measure_canonical_correlation(
                CrossDomainRun {
                    product: &Tracked::from(&product),
                    frame: &Tracked::from(&frame),
                    run_id: "sparse-cca",
                    timestamp: Timestamp::new("now"),
                },
                samples,
                CanonicalCorrelationConfig::default(),
            )
            .unwrap()
    };

    let incomplete = [sample(
        "incomplete",
        &[("presentation", 1.0)],
        &[("composition", 1.0)],
    )];
    let insufficient = calculate(&incomplete);
    assert_eq!(
        insufficient.value().reading,
        Reading::InsufficientEvidence { have: 0, need: 3 }
    );
    assert_eq!(insufficient.value().sample_count, Some(0));

    let constant = [
        sample(
            "a",
            &[("presentation", 1.0), ("social", 1.0)],
            &[("composition", 1.0), ("sharing", 1.0)],
        ),
        sample(
            "b",
            &[("presentation", 1.0), ("social", 1.0)],
            &[("composition", 2.0), ("sharing", 2.0)],
        ),
        sample(
            "c",
            &[("presentation", 1.0), ("social", 1.0)],
            &[("composition", 3.0), ("sharing", 3.0)],
        ),
    ];
    assert!(matches!(
        calculate(&constant).value().reading,
        Reading::NotApplicable { ref reason } if reason.contains("no variation")
    ));
}

#[test]
fn engine_cca_rejects_a_frame_from_another_product_version() {
    let (engine, product, frame) = product_and_frame();
    let mut wrong_frame = frame.value().clone();
    wrong_frame.product_version = ProductDomainVersion::new("other");
    assert!(engine
        .measure_canonical_correlation(
            CrossDomainRun {
                product: &Tracked::from(&product),
                frame: &Tracked::from_recorded(DerivedId::new("wrong-frame"), wrong_frame),
                run_id: "invalid-cca",
                timestamp: Timestamp::new("now"),
            },
            &coupled_samples(),
            CanonicalCorrelationConfig::default(),
        )
        .is_err());
}

#[test]
fn engine_cca_holds_the_sensor_to_its_declared_params_schema() {
    // Product sensors are dispatched by fixed id and never reach
    // `classify_sensor`, so the descriptor's schema is enforced on this path
    // explicitly. `tolerance` is declared `exclusiveMaximum: 1`; before that
    // keyword was implemented the bound was declared and never applied.
    let (engine, product, frame) = product_and_frame();
    let out_of_range = CanonicalCorrelationConfig {
        tolerance: 1.0,
        ..CanonicalCorrelationConfig::default()
    };
    let error = engine
        .measure_canonical_correlation(
            CrossDomainRun {
                product: &Tracked::from(&product),
                frame: &Tracked::from(&frame),
                run_id: "schema-cca",
                timestamp: Timestamp::new("now"),
            },
            &coupled_samples(),
            out_of_range,
        )
        .expect_err("a config the declared schema rejects must not reach the sensor");
    let message = error.to_string();
    assert!(message.contains("tolerance"), "got: {message}");
    assert!(message.contains("must be less than 1"), "got: {message}");

    // The same call with an in-range tolerance still succeeds, so the gate
    // rejects the violation rather than the path.
    assert!(engine
        .measure_canonical_correlation(
            CrossDomainRun {
                product: &Tracked::from(&product),
                frame: &Tracked::from(&frame),
                run_id: "schema-cca-ok",
                timestamp: Timestamp::new("now"),
            },
            &coupled_samples(),
            CanonicalCorrelationConfig::default(),
        )
        .is_ok());
}

#[test]
fn empty_frame_still_records_every_selected_sample() {
    let (engine, product, _) = product_and_frame();
    let product = Tracked::from(&product);
    let frame = create_product_frame(
        &product,
        &[],
        ProductFrameId::new("empty"),
        ProductFrameVersion::new("1"),
        "empty-frame",
        Timestamp::new("2026-09-24T00:00:00Z"),
    )
    .unwrap();
    let frame = Tracked::from(&frame);
    let samples = vec![sample("unused", &[], &[])];
    let result = engine
        .measure_canonical_correlation(
            CrossDomainRun {
                product: &product,
                frame: &frame,
                run_id: "empty-cca",
                timestamp: Timestamp::new("2026-09-24T00:00:01Z"),
            },
            &samples,
            CanonicalCorrelationConfig::default(),
        )
        .unwrap();
    assert!(matches!(
        result.value().reading,
        Reading::NotApplicable { .. }
    ));
    assert!(result.provenance().inputs.contains(samples[0].id()));
    assert!(result.provenance().inputs.contains(product.id()));
    assert!(result.provenance().inputs.contains(frame.id()));
}
