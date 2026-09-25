use std::collections::{BTreeMap, BTreeSet};

use unclip_domain::{
    DomainId, DomainSnapshot, FrameAxis, FrameId, MeasurementFrame, ProductDomainId,
    ProductDomainSnapshot, ProductDomainVersion, ProductFrameAxis, ProductFrameId,
    ProductFrameVersion, ProductInteraction, ProductMeasurementFrame, Unit, UnitId, UnitKind,
};
use unclip_engine::{CompositionMeasurementInputs, Engine, MeasurementInputs, MeasurementRun};
use unclip_epistemic::{Calculated, DerivedId, DomainVersion, FrameVersion, Timestamp, Tracked};
use unclip_measure::{CrossDomainMutualInformationConfig, CrossDomainSample, Measurement};
use unclip_observe::ObservationId;
use unclip_plugin::{EngineProfile, PluginSelection};

fn domain(id: &str, version: &str, unit: &str) -> DomainSnapshot {
    DomainSnapshot {
        id: DomainId::new(id),
        version: DomainVersion::new(version),
        units: BTreeMap::from([(
            UnitId::new(unit),
            Unit {
                id: UnitId::new(unit),
                kind: UnitKind::AtomicMeaning,
                label: None,
                properties: BTreeMap::new(),
            },
        )]),
        relations: BTreeMap::new(),
    }
}

fn frame(id: &str, version: &str, unit: &str) -> MeasurementFrame {
    MeasurementFrame {
        id: FrameId::new(id),
        version: FrameVersion::new(version),
        axes: vec![FrameAxis {
            unit: UnitId::new(unit),
            label: None,
        }],
    }
}

struct Fixture {
    engine: Engine,
    left: Tracked<DomainSnapshot>,
    left_frame: Tracked<MeasurementFrame>,
    right: Tracked<DomainSnapshot>,
    right_frame: Tracked<MeasurementFrame>,
    product: Calculated<ProductDomainSnapshot>,
    product_frame: Calculated<ProductMeasurementFrame>,
    left_measurements: Vec<Calculated<Measurement>>,
    right_measurements: Vec<Calculated<Measurement>>,
    product_measurements: Vec<Calculated<Measurement>>,
}

fn fixture(prefix: &str) -> Fixture {
    let engine = Engine::with_builtins().unwrap();
    let left = Tracked::from_recorded(
        DerivedId::new(format!("{prefix}-left-domain")),
        domain(&format!("{prefix}-left"), "left-v2", "a"),
    );
    let left_frame = Tracked::from_recorded(
        DerivedId::new(format!("{prefix}-left-frame")),
        frame(&format!("{prefix}-left-frame"), "left-f3", "a"),
    );
    let right = Tracked::from_recorded(
        DerivedId::new(format!("{prefix}-right-domain")),
        domain(&format!("{prefix}-right"), "right-v4", "b"),
    );
    let right_frame = Tracked::from_recorded(
        DerivedId::new(format!("{prefix}-right-frame")),
        frame(&format!("{prefix}-right-frame"), "right-f5", "b"),
    );
    let interaction = Tracked::from_recorded(
        DerivedId::new(format!("{prefix}-interaction")),
        ProductInteraction {
            left: UnitId::new("a"),
            right: UnitId::new("b"),
            observations: vec![DerivedId::new(format!("{prefix}-support"))],
            requirements: vec![],
        },
    );
    let product = engine
        .materialize_product_domain(
            &left,
            &right,
            &[interaction],
            ProductDomainId::new(format!("{prefix}-product")),
            ProductDomainVersion::new("product-v6"),
            &format!("{prefix}-materialize"),
            Timestamp::new("2026-09-24T00:00:00Z"),
        )
        .unwrap();
    let product_frame = engine
        .create_product_frame(
            &Tracked::from(&product),
            &[ProductFrameAxis {
                left: UnitId::new("a"),
                right: UnitId::new("b"),
                label: None,
            }],
            ProductFrameId::new(format!("{prefix}-product-frame")),
            ProductFrameVersion::new("product-f7"),
            &format!("{prefix}-product-frame-run"),
            Timestamp::new("2026-09-24T00:00:00Z"),
        )
        .unwrap();

    let plan = engine
        .plan(&EngineProfile {
            sensors: vec![PluginSelection::any("sensor.coverage")],
            ..EngineProfile::default()
        })
        .unwrap();
    let params = BTreeMap::new();
    let measure =
        |domain: &Tracked<DomainSnapshot>, frame: &Tracked<MeasurementFrame>, run_id: &str| {
            engine
                .measure(
                    &plan,
                    MeasurementInputs {
                        domain: unclip_epistemic::DependencyCollector::default().read(domain),
                        frame: unclip_epistemic::DependencyCollector::default().read(frame),
                        observations: &[],
                        alignments: &[],
                        rankings: &[],
                    },
                    MeasurementRun {
                        id: run_id,
                        timestamp: Timestamp::new("2026-09-24T00:00:00Z"),
                        params: &params,
                    },
                )
                .unwrap()
        };
    let left_measurements = measure(&left, &left_frame, &format!("{prefix}-measure-left"));
    let right_measurements = measure(&right, &right_frame, &format!("{prefix}-measure-right"));
    let samples = [
        Tracked::from_recorded(
            DerivedId::new(format!("{prefix}-sample-1")),
            CrossDomainSample {
                observation: ObservationId::new(format!("{prefix}-observation-1")),
                left: BTreeMap::from([(UnitId::new("a"), 0.0)]),
                right: BTreeMap::from([(UnitId::new("b"), 0.0)]),
            },
        ),
        Tracked::from_recorded(
            DerivedId::new(format!("{prefix}-sample-2")),
            CrossDomainSample {
                observation: ObservationId::new(format!("{prefix}-observation-2")),
                left: BTreeMap::from([(UnitId::new("a"), 1.0)]),
                right: BTreeMap::from([(UnitId::new("b"), 1.0)]),
            },
        ),
    ];
    let product_measurements = vec![engine
        .measure_cross_domain_mutual_information(
            &Tracked::from(&product),
            &Tracked::from(&product_frame),
            &samples,
            CrossDomainMutualInformationConfig::default(),
            &format!("{prefix}-measure-product"),
            Timestamp::new("2026-09-24T00:00:00Z"),
        )
        .unwrap()];

    Fixture {
        engine,
        left,
        left_frame,
        right,
        right_frame,
        product,
        product_frame,
        left_measurements,
        right_measurements,
        product_measurements,
    }
}

fn compose(
    subject: &Fixture,
    run_id: &str,
) -> unclip_plugin::Result<Calculated<unclip_engine::CompositionMeasurementProfile>> {
    subject.engine.measure_composition(
        &subject.left,
        &subject.left_frame,
        &subject.right,
        &subject.right_frame,
        &Tracked::from(&subject.product),
        &Tracked::from(&subject.product_frame),
        CompositionMeasurementInputs {
            left: &subject.left_measurements,
            right: &subject.right_measurements,
            product: &subject.product_measurements,
        },
        run_id,
        Timestamp::new("2026-09-24T00:00:00Z"),
    )
}

#[test]
fn retains_three_separate_versioned_profiles_and_exact_dependencies() {
    let fixture = fixture("composition");
    let profile = compose(&fixture, "composition-run").unwrap();
    let value = profile.value();

    assert_eq!(value.left.domain, DomainId::new("composition-left"));
    assert_eq!(value.left.domain_version, DomainVersion::new("left-v2"));
    assert_eq!(value.left.frame_version, FrameVersion::new("left-f3"));
    assert_eq!(value.right.domain, DomainId::new("composition-right"));
    assert_eq!(value.right.domain_version, DomainVersion::new("right-v4"));
    assert_eq!(value.right.frame_version, FrameVersion::new("right-f5"));
    assert_eq!(
        value.product.binding.product,
        ProductDomainId::new("composition-product")
    );
    assert_eq!(value.product.binding.left.domain, value.left.domain);
    assert_eq!(value.product.binding.right.domain, value.right.domain);
    assert_eq!(value.left.measurements.len(), 1);
    assert_eq!(value.right.measurements.len(), 1);
    assert_eq!(value.product.measurements.len(), 1);
    assert_eq!(
        &value.left.measurements[0].measurement,
        fixture.left_measurements[0].value()
    );
    assert_eq!(
        &value.product.measurements[0].measurement,
        fixture.product_measurements[0].value()
    );

    let expected = [
        fixture.left.id().clone(),
        fixture.left_frame.id().clone(),
        fixture.right.id().clone(),
        fixture.right_frame.id().clone(),
        fixture.product.id().clone(),
        fixture.product_frame.id().clone(),
        fixture.left_measurements[0].id().clone(),
        fixture.right_measurements[0].id().clone(),
        fixture.product_measurements[0].id().clone(),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>()
    .into_iter()
    .collect::<Vec<_>>();
    assert_eq!(profile.provenance().inputs, expected);
    assert_eq!(
        profile.provenance().params["product"]["binding"]["left"]["version"],
        "left-v2"
    );
    assert_eq!(profile.provenance().domain_version, None);
    assert_eq!(profile.provenance().frame_version, None);

    let encoded = serde_json::to_string(value).unwrap();
    let decoded: unclip_engine::CompositionMeasurementProfile =
        serde_json::from_str(&encoded).unwrap();
    assert_eq!(&decoded, value);
    assert_eq!(compose(&fixture, "composition-run").unwrap(), profile);
}

#[test]
fn rejects_stale_cross_bound_and_incomplete_measurement_profiles() {
    let subject = fixture("invalid");
    let cross_bound = subject.engine.measure_composition(
        &subject.left,
        &subject.left_frame,
        &subject.right,
        &subject.right_frame,
        &Tracked::from(&subject.product),
        &Tracked::from(&subject.product_frame),
        CompositionMeasurementInputs {
            left: &subject.right_measurements,
            right: &subject.left_measurements,
            product: &subject.product_measurements,
        },
        "invalid-cross-bound",
        Timestamp::new("2026-09-24T00:00:00Z"),
    );
    assert!(cross_bound
        .unwrap_err()
        .to_string()
        .contains("exact domain and frame versions"));

    let other = fixture("other");
    let stale_product = subject.engine.measure_composition(
        &subject.left,
        &subject.left_frame,
        &subject.right,
        &subject.right_frame,
        &Tracked::from(&subject.product),
        &Tracked::from(&subject.product_frame),
        CompositionMeasurementInputs {
            left: &subject.left_measurements,
            right: &subject.right_measurements,
            product: &other.product_measurements,
        },
        "invalid-product",
        Timestamp::new("2026-09-24T00:00:00Z"),
    );
    assert!(stale_product
        .unwrap_err()
        .to_string()
        .contains("exact product, frame, and input-domain versions"));

    let empty = subject.engine.measure_composition(
        &subject.left,
        &subject.left_frame,
        &subject.right,
        &subject.right_frame,
        &Tracked::from(&subject.product),
        &Tracked::from(&subject.product_frame),
        CompositionMeasurementInputs {
            left: &[],
            right: &subject.right_measurements,
            product: &subject.product_measurements,
        },
        "invalid-empty",
        Timestamp::new("2026-09-24T00:00:00Z"),
    );
    assert!(empty
        .unwrap_err()
        .to_string()
        .contains("at least one measurement"));
}
