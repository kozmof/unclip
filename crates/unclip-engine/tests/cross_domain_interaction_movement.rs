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
    MeasurementValue, ObservationSequence, OrderedObservation, Reading,
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
            &Tracked::from_recorded(DerivedId::new("left@2"), domain("left", "2", &["a", "b"])),
            &Tracked::from_recorded(DerivedId::new("right@4"), domain("right", "4", &["x", "y"])),
            &[interaction("a-x", "a", "x"), interaction("b-y", "b", "y")],
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
                    label: Some("primary".into()),
                },
                ProductFrameAxis {
                    left: UnitId::new("b"),
                    right: UnitId::new("y"),
                    label: Some("sparse".into()),
                },
            ],
            ProductFrameId::new("left-right.movement"),
            ProductFrameVersion::new("8"),
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

fn samples() -> Vec<Tracked<CrossDomainSample>> {
    vec![
        sample("one", &[("a", 0.0), ("b", 0.0)], &[("x", 10.0), ("y", 0.0)]),
        sample("two", &[("a", 1.0)], &[("x", 20.0), ("y", 1.0)]),
        sample(
            "three",
            &[("a", 2.0), ("b", 2.0)],
            &[("x", 15.0), ("y", 2.0)],
        ),
        sample(
            "four",
            &[("a", 2.0), ("b", 3.0)],
            &[("x", 15.0), ("y", 3.0)],
        ),
    ]
}

fn config(minimum: usize) -> CrossDomainInteractionMovementConfig {
    CrossDomainInteractionMovementConfig {
        minimum_transitions: NonZeroUsize::new(minimum).unwrap(),
        sequence: ObservationSequence::new(
            ["one", "two", "three", "four"]
                .into_iter()
                .enumerate()
                .map(|(position, observation)| OrderedObservation {
                    observation: ObservationId::new(observation),
                    position: position as i64,
                })
                .collect(),
        )
        .unwrap(),
    }
}

#[test]
fn engine_interaction_movement_is_order_stable_sparse_versioned_and_tracked() {
    let (engine, product, frame) = product_and_frame();
    let samples = samples();
    let measure = |samples: &[Tracked<CrossDomainSample>]| {
        engine.measure_cross_domain_interaction_movement(
            &Tracked::from(&product),
            &Tracked::from(&frame),
            samples,
            config(1),
            "movement-run",
            Timestamp::new("2026-09-24T00:00:01Z"),
        )
    };
    let result = measure(&samples).unwrap();
    let reversed = samples.iter().cloned().rev().collect::<Vec<_>>();
    assert_eq!(measure(&reversed).unwrap(), result);

    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = &result.value().reading
    else {
        panic!("expected structured interaction movement")
    };
    let movement: CrossDomainInteractionMovement = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(movement.axes.len(), 2);
    assert_eq!(movement.axes[0].directional_concordance, 0.0);
    assert_eq!(movement.axes[0].concordant_transitions, 1);
    assert_eq!(movement.axes[0].discordant_transitions, 1);
    assert_eq!(movement.axes[0].stationary_transitions, 1);
    assert_eq!(movement.axes[1].directional_concordance, 1.0);
    assert_eq!(movement.axes[1].transition_count, 1);
    assert_eq!(movement.axes[1].excluded_transitions.len(), 2);
    assert_eq!(movement.binding.product_version.0, "6");
    assert_eq!(movement.binding.frame_version.0, "8");

    assert_eq!(
        result.value().sensor,
        PluginId::new("sensor.cross-domain-interaction-movement")
    );
    assert_eq!(
        result.provenance().algorithm,
        "signed_consecutive_cross_domain_movement"
    );
    assert_eq!(result.provenance().params["minimum_transitions"], 1);
    assert_eq!(
        result.provenance().params["sequence"]
            .as_array()
            .unwrap()
            .len(),
        4
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
fn engine_interaction_movement_keeps_partial_and_total_shortfalls_typed() {
    let (engine, product, frame) = product_and_frame();
    let samples = samples();
    let partial = engine
        .measure_cross_domain_interaction_movement(
            &Tracked::from(&product),
            &Tracked::from(&frame),
            &samples,
            config(2),
            "partial-movement-run",
            Timestamp::new("2026-09-24T00:00:01Z"),
        )
        .unwrap();
    let Reading::Value {
        value: MeasurementValue::Structured(value),
    } = &partial.value().reading
    else {
        panic!("one sufficiently observed axis should retain a value")
    };
    let movement: CrossDomainInteractionMovement = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(movement.axes.len(), 1);
    assert_eq!(movement.unassessed_axes.len(), 1);
    assert_eq!(movement.unassessed_axes[0].have, 1);
    assert_eq!(movement.unassessed_axes[0].need, 2);

    let insufficient = engine
        .measure_cross_domain_interaction_movement(
            &Tracked::from(&product),
            &Tracked::from(&frame),
            &samples,
            config(4),
            "insufficient-movement-run",
            Timestamp::new("2026-09-24T00:00:01Z"),
        )
        .unwrap();
    assert_eq!(
        insufficient.value().reading,
        Reading::InsufficientEvidence { have: 3, need: 4 }
    );
    assert_eq!(insufficient.value().sample_count, Some(3));
    assert_eq!(
        insufficient.value().context.values["unassessed_axes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn engine_interaction_movement_rejects_sequence_mismatch() {
    let (engine, product, frame) = product_and_frame();
    let mut config = config(1);
    config.sequence = ObservationSequence::new(
        ["one", "two", "three", "other"]
            .into_iter()
            .enumerate()
            .map(|(position, observation)| OrderedObservation {
                observation: ObservationId::new(observation),
                position: position as i64,
            })
            .collect(),
    )
    .unwrap();
    let error = engine
        .measure_cross_domain_interaction_movement(
            &Tracked::from(&product),
            &Tracked::from(&frame),
            &samples(),
            config,
            "wrong-sequence-run",
            Timestamp::new("2026-09-24T00:00:01Z"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("exactly match"));
}
