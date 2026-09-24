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
    CrossDomainInteractionMovement, CrossDomainInteractionMovementConfig, CrossDomainSample,
    CrossProductAxisMapping, CrossProductTransfer, CrossProductTransferConfig, Measurement,
    MeasurementValue, ObservationSequence, OrderedObservation, Reading,
};
use unclip_observe::ObservationId;

fn domain(id: &str, units: &[&str]) -> DomainSnapshot {
    DomainSnapshot {
        id: DomainId::new(id),
        version: DomainVersion::new("1"),
        units: units
            .iter()
            .map(|id| {
                (
                    UnitId::new(*id),
                    Unit {
                        id: UnitId::new(*id),
                        kind: UnitKind::AtomicMeaning,
                        label: None,
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

fn product_and_frame(
    engine: &Engine,
    name: &str,
    left: [&str; 2],
    right: [&str; 2],
) -> (
    Calculated<ProductDomainSnapshot>,
    Calculated<ProductMeasurementFrame>,
) {
    let product = engine
        .materialize_product_domain(
            &Tracked::from_recorded(
                DerivedId::new(format!("{name}-left@1")),
                domain(&format!("{name}-left"), &left),
            ),
            &Tracked::from_recorded(
                DerivedId::new(format!("{name}-right@1")),
                domain(&format!("{name}-right"), &right),
            ),
            &[
                interaction(&format!("{name}-first"), left[0], right[0]),
                interaction(&format!("{name}-second"), left[1], right[1]),
            ],
            ProductDomainId::new(format!("{name}-product")),
            ProductDomainVersion::new("2"),
            &format!("{name}-product-run"),
            Timestamp::new("2026-09-24T00:00:00Z"),
        )
        .unwrap();
    let frame = engine
        .create_product_frame(
            &Tracked::from(&product),
            &[
                ProductFrameAxis {
                    left: UnitId::new(left[0]),
                    right: UnitId::new(right[0]),
                    label: None,
                },
                ProductFrameAxis {
                    left: UnitId::new(left[1]),
                    right: UnitId::new(right[1]),
                    label: None,
                },
            ],
            ProductFrameId::new(format!("{name}-frame")),
            ProductFrameVersion::new("3"),
            &format!("{name}-frame-run"),
            Timestamp::new("2026-09-24T00:00:00Z"),
        )
        .unwrap();
    (product, frame)
}

fn samples(
    prefix: &str,
    left: [&str; 2],
    right: [&str; 2],
    second_target_reversed: bool,
) -> Vec<Tracked<CrossDomainSample>> {
    (0..5)
        .map(|index| {
            let value = index as f64;
            Tracked::from_recorded(
                DerivedId::new(format!("{prefix}-evidence-{index}")),
                CrossDomainSample {
                    observation: ObservationId::new(format!("{prefix}-{index}")),
                    left: BTreeMap::from([
                        (UnitId::new(left[0]), value),
                        (UnitId::new(left[1]), value),
                    ]),
                    right: BTreeMap::from([
                        (UnitId::new(right[0]), value),
                        (
                            UnitId::new(right[1]),
                            if second_target_reversed {
                                -value
                            } else {
                                value
                            },
                        ),
                    ]),
                },
            )
        })
        .collect()
}

fn sequence(prefix: &str) -> ObservationSequence {
    ObservationSequence::new(
        (0..5)
            .map(|position| OrderedObservation {
                observation: ObservationId::new(format!("{prefix}-{position}")),
                position,
            })
            .collect(),
    )
    .unwrap()
}

fn movement(
    engine: &Engine,
    product: &Calculated<ProductDomainSnapshot>,
    frame: &Calculated<ProductMeasurementFrame>,
    samples: &[Tracked<CrossDomainSample>],
    prefix: &str,
) -> (Calculated<Measurement>, CrossDomainInteractionMovement) {
    let result = engine
        .measure_cross_domain_interaction_movement(
            &Tracked::from(product),
            &Tracked::from(frame),
            samples,
            CrossDomainInteractionMovementConfig {
                minimum_transitions: NonZeroUsize::new(2).unwrap(),
                sequence: sequence(prefix),
            },
            &format!("{prefix}-movement-run"),
            Timestamp::new("2026-09-24T00:00:01Z"),
        )
        .unwrap();
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = &result.value().reading
    else {
        panic!("expected structured movement")
    };
    let movement = serde_json::from_value(value.clone()).unwrap();
    (result, movement)
}

fn mapping(source: (&str, &str), target: (&str, &str)) -> CrossProductAxisMapping {
    CrossProductAxisMapping {
        source_left: UnitId::new(source.0),
        source_right: UnitId::new(source.1),
        target_left: UnitId::new(target.0),
        target_right: UnitId::new(target.1),
    }
}

fn config(minimum: usize) -> CrossProductTransferConfig {
    CrossProductTransferConfig {
        mappings: vec![
            mapping(("b", "y"), ("q", "v")),
            mapping(("a", "x"), ("p", "u")),
        ],
        minimum_transitions: NonZeroUsize::new(minimum).unwrap(),
    }
}

struct Fixture {
    engine: Engine,
    source_product: Calculated<ProductDomainSnapshot>,
    source_frame: Calculated<ProductMeasurementFrame>,
    source_measurement: Calculated<Measurement>,
    source_movement: CrossDomainInteractionMovement,
    target_product: Calculated<ProductDomainSnapshot>,
    target_frame: Calculated<ProductMeasurementFrame>,
    target_measurement: Calculated<Measurement>,
    target_movement: CrossDomainInteractionMovement,
}

fn fixture() -> Fixture {
    let engine = Engine::with_builtins().unwrap();
    let (source_product, source_frame) =
        product_and_frame(&engine, "source", ["a", "b"], ["x", "y"]);
    let (target_product, target_frame) =
        product_and_frame(&engine, "target", ["p", "q"], ["u", "v"]);
    let source_samples = samples("source", ["a", "b"], ["x", "y"], false);
    let target_samples = samples("target", ["p", "q"], ["u", "v"], true);
    let (source_measurement, source_movement) = movement(
        &engine,
        &source_product,
        &source_frame,
        &source_samples,
        "source",
    );
    let (target_measurement, target_movement) = movement(
        &engine,
        &target_product,
        &target_frame,
        &target_samples,
        "target",
    );
    Fixture {
        engine,
        source_product,
        source_frame,
        source_measurement,
        source_movement,
        target_product,
        target_frame,
        target_measurement,
        target_movement,
    }
}

#[test]
fn engine_cross_product_transfer_preserves_signed_zero_versions_and_dependencies() {
    let fixture = fixture();
    let result = fixture
        .engine
        .measure_cross_product_transfer(
            &Tracked::from(&fixture.source_product),
            &Tracked::from(&fixture.source_frame),
            &Tracked::from_derived(&fixture.source_measurement, fixture.source_movement),
            &Tracked::from(&fixture.target_product),
            &Tracked::from(&fixture.target_frame),
            &Tracked::from_derived(&fixture.target_measurement, fixture.target_movement),
            config(2),
            "transfer-run",
            Timestamp::new("2026-09-24T00:00:02Z"),
        )
        .unwrap();
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = &result.value().reading
    else {
        panic!("expected structured transfer")
    };
    let transfer: CrossProductTransfer = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(transfer.transfers.len(), 2);
    assert_eq!(transfer.transfers[0].absolute_change, 0.0);
    assert_eq!(transfer.transfers[1].directional_concordance_change, -2.0);
    assert_eq!(transfer.transfers[1].absolute_change, 2.0);
    assert_eq!(transfer.source_binding.product_version.0, "2");
    assert_eq!(transfer.target_binding.frame_version.0, "3");

    assert_eq!(
        result.value().sensor,
        PluginId::new("sensor.cross-product-transfer")
    );
    assert_eq!(
        result.provenance().algorithm,
        "mapped_cross_product_movement_transfer"
    );
    assert_eq!(result.provenance().params["minimum_transitions"], 2);
    assert_eq!(
        result.provenance().params["mappings"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let expected = [
        fixture.source_product.id().clone(),
        fixture.source_frame.id().clone(),
        fixture.source_measurement.id().clone(),
        fixture.target_product.id().clone(),
        fixture.target_frame.id().clone(),
        fixture.target_measurement.id().clone(),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>()
    .into_iter()
    .collect::<Vec<_>>();
    assert_eq!(result.provenance().inputs, expected);
}

#[test]
fn engine_cross_product_transfer_keeps_shortfalls_typed() {
    let fixture = fixture();
    let result = fixture
        .engine
        .measure_cross_product_transfer(
            &Tracked::from(&fixture.source_product),
            &Tracked::from(&fixture.source_frame),
            &Tracked::from_derived(&fixture.source_measurement, fixture.source_movement),
            &Tracked::from(&fixture.target_product),
            &Tracked::from(&fixture.target_frame),
            &Tracked::from_derived(&fixture.target_measurement, fixture.target_movement),
            config(5),
            "sparse-transfer-run",
            Timestamp::new("2026-09-24T00:00:02Z"),
        )
        .unwrap();
    assert_eq!(
        result.value().reading,
        Reading::InsufficientEvidence { have: 4, need: 5 }
    );
    assert_eq!(result.value().sample_count, Some(4));
    assert_eq!(
        result.value().context.values["unassessed_transfers"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn engine_cross_product_transfer_rejects_stale_or_incomplete_movement() {
    let stale_fixture = fixture();
    let mut target_movement = stale_fixture.target_movement;
    target_movement.binding.frame_version = ProductFrameVersion::new("stale");
    let error = stale_fixture
        .engine
        .measure_cross_product_transfer(
            &Tracked::from(&stale_fixture.source_product),
            &Tracked::from(&stale_fixture.source_frame),
            &Tracked::from_derived(
                &stale_fixture.source_measurement,
                stale_fixture.source_movement,
            ),
            &Tracked::from(&stale_fixture.target_product),
            &Tracked::from(&stale_fixture.target_frame),
            &Tracked::from_derived(&stale_fixture.target_measurement, target_movement),
            config(2),
            "stale-transfer-run",
            Timestamp::new("2026-09-24T00:00:02Z"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("exact source and target"));
    let fixture = fixture();
    let mut target_movement = fixture.target_movement;
    target_movement.axes.pop();
    let error = fixture
        .engine
        .measure_cross_product_transfer(
            &Tracked::from(&fixture.source_product),
            &Tracked::from(&fixture.source_frame),
            &Tracked::from_derived(&fixture.source_measurement, fixture.source_movement),
            &Tracked::from(&fixture.target_product),
            &Tracked::from(&fixture.target_frame),
            &Tracked::from_derived(&fixture.target_measurement, target_movement),
            config(2),
            "incomplete-transfer-run",
            Timestamp::new("2026-09-24T00:00:02Z"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("complete movement evidence"));
}
