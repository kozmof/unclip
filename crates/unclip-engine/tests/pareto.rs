use unclip_engine::{Engine, ObjectiveDirection, ParetoDimension, ParetoRelation};
use unclip_epistemic::{DerivedId, PluginId, Timestamp, Tracked};
use unclip_measure::{Measurement, MeasurementContext, MeasurementValue, Reading};
fn value(id: &str, reading: Reading) -> Tracked<Measurement> {
    Tracked::from_recorded(
        DerivedId::new(id),
        Measurement {
            sensor: PluginId::new("fixture"),
            sensor_version: semver::Version::new(1, 0, 0),
            reading,
            confidence: None,
            sample_count: Some(4),
            context: MeasurementContext::default(),
        },
    )
}
fn scalar(id: &str, number: f64) -> Tracked<Measurement> {
    value(
        id,
        Reading::Value {
            value: MeasurementValue::Scalar(number),
        },
    )
}
fn dimensions() -> Vec<ParetoDimension> {
    vec![
        ParetoDimension {
            name: "benefit".into(),
            left: DerivedId::new("a"),
            right: DerivedId::new("b"),
            direction: ObjectiveDirection::Maximize,
        },
        ParetoDimension {
            name: "cost".into(),
            left: DerivedId::new("c"),
            right: DerivedId::new("d"),
            direction: ObjectiveDirection::Minimize,
        },
    ]
}
#[test]
fn preserves_dominance_equality_tradeoffs_and_missing_dimensions() {
    let engine = Engine::with_builtins().unwrap();
    for (a, b, c, d, expected) in [
        (2., 1., 1., 2., ParetoRelation::LeftDominates),
        (1., 2., 2., 1., ParetoRelation::RightDominates),
        (1., 1., 2., 2., ParetoRelation::Equal),
        (2., 1., 2., 1., ParetoRelation::Tradeoff),
        (2., 1., 1., 1., ParetoRelation::LeftDominates),
    ] {
        let inputs = [
            scalar("a", a),
            scalar("b", b),
            scalar("c", c),
            scalar("d", d),
        ];
        let result = engine
            .compare_pareto(&inputs, &dimensions(), "run", Timestamp::new("now"))
            .unwrap();
        assert_eq!(result.value().relation, expected);
        assert_eq!(
            result.provenance().inputs,
            ["a", "b", "c", "d"].map(DerivedId::new)
        );
        let mut reversed = dimensions();
        reversed.reverse();
        assert_eq!(
            result,
            engine
                .compare_pareto(&inputs, &reversed, "run", Timestamp::new("now"))
                .unwrap()
        );
    }
    let inputs = [
        scalar("a", 2.),
        scalar("b", 1.),
        scalar("c", 1.),
        value("d", Reading::InsufficientEvidence { have: 1, need: 2 }),
    ];
    let result = engine
        .compare_pareto(&inputs, &dimensions(), "run", Timestamp::new("now"))
        .unwrap();
    assert_eq!(result.value().relation, ParetoRelation::Incomparable);
    assert_eq!(result.value().dimensions.len(), 2);
}
#[test]
fn rejects_ambiguous_missing_and_nonfinite_inputs() {
    let engine = Engine::with_builtins().unwrap();
    let inputs = [
        scalar("a", 1.),
        scalar("b", 2.),
        scalar("c", 1.),
        scalar("d", 2.),
    ];
    let mut duplicate = dimensions();
    duplicate.push(duplicate[0].clone());
    for dims in [vec![], duplicate] {
        assert!(engine
            .compare_pareto(&inputs, &dims, "run", Timestamp::new("now"))
            .is_err());
    }
    assert!(engine
        .compare_pareto(&inputs[..2], &dimensions(), "run", Timestamp::new("now"))
        .is_err());
    assert!(engine
        .compare_pareto(
            &[scalar("a", f64::NAN), scalar("b", 2.)],
            &dimensions()[..1],
            "run",
            Timestamp::new("now")
        )
        .is_err());
}
