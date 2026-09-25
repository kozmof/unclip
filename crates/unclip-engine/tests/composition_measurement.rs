use std::collections::{BTreeMap, BTreeSet};

use unclip_domain::{
    CandidateKind, DomainId, DomainSnapshot, FrameAxis, FrameId, MeasurementFrame, ProductDomainId,
    ProductDomainSnapshot, ProductDomainVersion, ProductFrameAxis, ProductFrameId,
    ProductFrameVersion, ProductInteraction, ProductMeasurementFrame, Unit, UnitId, UnitKind,
};
use unclip_engine::{
    CandidateInputs, CompositionMeasurementInputs, Engine, IndependenceDefinition,
    MeasurementInputs, MeasurementRun,
};
use unclip_epistemic::{Calculated, DerivedId, DomainVersion, FrameVersion, Timestamp, Tracked};
use unclip_measure::{
    CrossDomainMutualInformationConfig, CrossDomainSample, ExpectedIndependentBehavior,
    Measurement, MeasurementKind, MeasurementValue, Reading,
};
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

fn independence_definition(
    composition: &Calculated<unclip_engine::CompositionMeasurementProfile>,
    expected: ExpectedIndependentBehavior,
) -> IndependenceDefinition {
    IndependenceDefinition {
        product_measurement: composition.value().product.measurements[0].id.clone(),
        left_measurements: vec![composition.value().left.measurements[0].id.clone()],
        right_measurements: vec![composition.value().right.measurements[0].id.clone()],
        rule: "fixture.explicit-independent-association".into(),
        parameters: serde_json::json!({"assumption": "factorized inputs"}),
        expected,
    }
}

#[test]
fn independence_rules_are_typed_versioned_and_bound_to_both_input_profiles() {
    let fixture = fixture("expectation");
    let composition = compose(&fixture, "expectation-composition").unwrap();
    let definition = independence_definition(
        &composition,
        ExpectedIndependentBehavior::Structured {
            value: serde_json::json!({
                "status": "independent",
                "assumption": "factorized inputs"
            }),
        },
    );
    let expectations = fixture
        .engine
        .define_independent_behavior(
            &composition,
            &[definition],
            "expectation-run",
            Timestamp::new("2026-09-25T00:00:00Z"),
        )
        .unwrap();
    let value = expectations.value();

    assert_eq!(value.composition_profile, *composition.id());
    assert_eq!(value.expectations.len(), 1);
    assert_eq!(
        value.expectations[0].measurement_kind,
        MeasurementKind::Structured
    );
    assert_eq!(
        value.expectations[0].left_measurements,
        vec![fixture.left_measurements[0].id().clone()]
    );
    assert_eq!(
        value.expectations[0].right_measurements,
        vec![fixture.right_measurements[0].id().clone()]
    );
    assert_eq!(
        value.expectations[0].expected.reading(),
        Reading::Value {
            value: unclip_measure::MeasurementValue::Structured(serde_json::json!({
                "status": "independent",
                "assumption": "factorized inputs"
            }))
        }
    );
    assert_eq!(
        expectations.provenance().inputs,
        vec![composition.id().clone()]
    );
    assert_eq!(
        expectations.provenance().params["definitions"][0]["rule"],
        "fixture.explicit-independent-association"
    );
    assert_eq!(
        fixture
            .engine
            .define_independent_behavior(
                &composition,
                &[independence_definition(
                    &composition,
                    ExpectedIndependentBehavior::Structured {
                        value: serde_json::json!({
                            "status": "independent",
                            "assumption": "factorized inputs"
                        })
                    }
                )],
                "expectation-run",
                Timestamp::new("2026-09-25T00:00:00Z"),
            )
            .unwrap(),
        expectations
    );
    let encoded = serde_json::to_string(value).unwrap();
    let decoded: unclip_engine::IndependenceExpectationProfile =
        serde_json::from_str(&encoded).unwrap();
    assert_eq!(&decoded, value);
}

#[test]
fn independence_rules_reject_implicit_kinds_and_unselected_sources() {
    let fixture = fixture("bad-expectation");
    let composition = compose(&fixture, "bad-expectation-composition").unwrap();

    let wrong_kind = fixture.engine.define_independent_behavior(
        &composition,
        &[independence_definition(
            &composition,
            ExpectedIndependentBehavior::Scalar { value: 0.0 },
        )],
        "wrong-kind",
        Timestamp::new("2026-09-25T00:00:00Z"),
    );
    assert!(wrong_kind
        .unwrap_err()
        .to_string()
        .contains("kind must match"));

    let mut missing_side = independence_definition(
        &composition,
        ExpectedIndependentBehavior::Undefined {
            measurement_kind: MeasurementKind::Structured,
            reason: "no validated structured independence rule".into(),
        },
    );
    missing_side.left_measurements.clear();
    let missing_side = fixture.engine.define_independent_behavior(
        &composition,
        &[missing_side],
        "missing-side",
        Timestamp::new("2026-09-25T00:00:00Z"),
    );
    assert!(missing_side
        .unwrap_err()
        .to_string()
        .contains("selected left measurements"));

    let mut foreign_source = independence_definition(
        &composition,
        ExpectedIndependentBehavior::Undefined {
            measurement_kind: MeasurementKind::Structured,
            reason: "no validated structured independence rule".into(),
        },
    );
    foreign_source.right_measurements = vec![DerivedId::new("foreign")];
    let foreign_source = fixture.engine.define_independent_behavior(
        &composition,
        &[foreign_source],
        "foreign-source",
        Timestamp::new("2026-09-25T00:00:00Z"),
    );
    assert!(foreign_source
        .unwrap_err()
        .to_string()
        .contains("selected right profile"));

    let uncovered = fixture.engine.define_independent_behavior(
        &composition,
        &[],
        "uncovered",
        Timestamp::new("2026-09-25T00:00:00Z"),
    );
    assert!(uncovered
        .unwrap_err()
        .to_string()
        .contains("cover every product measurement"));
}

fn define_expectations(
    fixture: &Fixture,
    composition: &Calculated<unclip_engine::CompositionMeasurementProfile>,
    expected: ExpectedIndependentBehavior,
    run_id: &str,
) -> Calculated<unclip_engine::IndependenceExpectationProfile> {
    fixture
        .engine
        .define_independent_behavior(
            composition,
            &[independence_definition(composition, expected)],
            run_id,
            Timestamp::new("2026-09-25T00:00:00Z"),
        )
        .unwrap()
}

fn structured_comparison_plan(engine: &Engine) -> unclip_plugin::RunPlan {
    engine
        .plan(&EngineProfile {
            comparators: vec![PluginSelection::any("compare.structured-identity")],
            ..EngineProfile::default()
        })
        .unwrap()
}

#[test]
fn product_behavior_is_compared_expected_to_observed_with_typed_provenance() {
    let fixture = fixture("compare-independence");
    let composition = compose(&fixture, "compare-independence-composition").unwrap();
    let Reading::Value {
        value: MeasurementValue::Structured(observed),
    } = &composition.value().product.measurements[0]
        .measurement
        .reading
    else {
        panic!("fixture product measurement must be structured")
    };
    let expectations = define_expectations(
        &fixture,
        &composition,
        ExpectedIndependentBehavior::Structured {
            value: observed.clone(),
        },
        "compare-independence-expectations",
    );
    let plan = structured_comparison_plan(&fixture.engine);
    let params = BTreeMap::new();
    let compare = || {
        fixture.engine.compare_product_with_independence(
            &plan,
            &composition,
            &expectations,
            MeasurementRun {
                id: "compare-independence-run",
                timestamp: Timestamp::new("2026-09-25T00:00:00Z"),
                params: &params,
            },
        )
    };
    let result = compare().unwrap();

    assert_eq!(result.expectations.len(), 1);
    assert_eq!(result.deltas.len(), 1);
    assert_eq!(result.profile.value().comparisons.len(), 1);
    let entry = &result.profile.value().comparisons[0];
    assert_eq!(
        entry.product_measurement,
        composition.value().product.measurements[0].id
    );
    assert_eq!(entry.expectation_measurement, *result.expectations[0].id());
    assert_eq!(entry.delta_id, *result.deltas[0].id());
    assert_eq!(
        entry.comparator,
        unclip_epistemic::PluginId::new("compare.structured-identity")
    );
    let MeasurementValue::Structured(delta) = &entry.delta.value else {
        panic!("expected structured delta")
    };
    let delta: unclip_engine::StructuredIdentityComparison =
        serde_json::from_value(delta.clone()).unwrap();
    assert!(matches!(
        delta,
        unclip_engine::StructuredIdentityComparison::Value {
            identical: true,
            ..
        }
    ));

    let expected_inputs = [
        composition.id().clone(),
        expectations.id().clone(),
        fixture.left_measurements[0].id().clone(),
        fixture.right_measurements[0].id().clone(),
        fixture.product_measurements[0].id().clone(),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>()
    .into_iter()
    .collect::<Vec<_>>();
    assert_eq!(result.expectations[0].provenance().inputs, expected_inputs);
    assert_eq!(
        result.deltas[0].provenance().inputs,
        [
            result.expectations[0].id().clone(),
            fixture.product_measurements[0].id().clone(),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );
    let profile_inputs = [
        composition.id().clone(),
        expectations.id().clone(),
        fixture.left_measurements[0].id().clone(),
        fixture.right_measurements[0].id().clone(),
        fixture.product_measurements[0].id().clone(),
        result.expectations[0].id().clone(),
        result.deltas[0].id().clone(),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>()
    .into_iter()
    .collect::<Vec<_>>();
    assert_eq!(result.profile.provenance().inputs, profile_inputs);
    assert_eq!(
        result.profile.provenance().params["binding"]["product_version"],
        "product-v6"
    );
    let replay = compare().unwrap();
    assert_eq!(replay.profile, result.profile);
    assert_eq!(replay.expectations, result.expectations);
    assert_eq!(replay.deltas, result.deltas);
    let encoded = serde_json::to_string(result.profile.value()).unwrap();
    let decoded: unclip_engine::IndependenceComparisonProfile =
        serde_json::from_str(&encoded).unwrap();
    assert_eq!(&decoded, result.profile.value());
}

#[test]
fn independence_comparison_preserves_undefined_rules_and_requires_typed_comparators() {
    let fixture = fixture("unavailable-independence");
    let composition = compose(&fixture, "unavailable-composition").unwrap();
    let expectations = define_expectations(
        &fixture,
        &composition,
        ExpectedIndependentBehavior::Undefined {
            measurement_kind: MeasurementKind::Structured,
            reason: "no validated structured independence rule".into(),
        },
        "unavailable-expectations",
    );
    let plan = structured_comparison_plan(&fixture.engine);
    let result = fixture
        .engine
        .compare_product_with_independence(
            &plan,
            &composition,
            &expectations,
            MeasurementRun {
                id: "unavailable-comparison",
                timestamp: Timestamp::new("2026-09-25T00:00:00Z"),
                params: &BTreeMap::new(),
            },
        )
        .unwrap();
    let MeasurementValue::Structured(delta) = &result.deltas[0].value().value else {
        panic!("expected structured delta")
    };
    let delta: unclip_engine::StructuredIdentityComparison =
        serde_json::from_value(delta.clone()).unwrap();
    assert!(matches!(
        delta,
        unclip_engine::StructuredIdentityComparison::Unavailable {
            expected: Reading::NotApplicable { .. },
            ..
        }
    ));

    let scalar_plan = fixture
        .engine
        .plan(&EngineProfile {
            comparators: vec![PluginSelection::any("compare.scalar-difference")],
            ..EngineProfile::default()
        })
        .unwrap();
    let wrong_comparator = fixture.engine.compare_product_with_independence(
        &scalar_plan,
        &composition,
        &expectations,
        MeasurementRun {
            id: "wrong-comparator",
            timestamp: Timestamp::new("2026-09-25T00:00:00Z"),
            params: &BTreeMap::new(),
        },
    );
    assert!(wrong_comparator
        .unwrap_err()
        .to_string()
        .contains("comparator for its measurement kind"));

    let other = self::fixture("other-comparison");
    let other_composition = compose(&other, "other-composition").unwrap();
    let foreign = fixture.engine.compare_product_with_independence(
        &plan,
        &other_composition,
        &expectations,
        MeasurementRun {
            id: "foreign-expectations",
            timestamp: Timestamp::new("2026-09-25T00:00:00Z"),
            params: &BTreeMap::new(),
        },
    );
    assert!(foreign
        .unwrap_err()
        .to_string()
        .contains("belongs to another composition"));
}

fn compare_for_candidates(
    fixture: &Fixture,
    composition: &Calculated<unclip_engine::CompositionMeasurementProfile>,
    expected: ExpectedIndependentBehavior,
    prefix: &str,
) -> unclip_engine::IndependenceComparisonResult {
    let expectations = define_expectations(
        fixture,
        composition,
        expected,
        &format!("{prefix}-expectations"),
    );
    fixture
        .engine
        .compare_product_with_independence(
            &structured_comparison_plan(&fixture.engine),
            composition,
            &expectations,
            MeasurementRun {
                id: &format!("{prefix}-comparison"),
                timestamp: Timestamp::new("2026-09-25T00:00:00Z"),
                params: &BTreeMap::new(),
            },
        )
        .unwrap()
}

#[test]
fn measured_product_deviations_generate_anonymous_cross_domain_candidates() {
    let fixture = fixture("cross-domain-candidate");
    let composition = compose(&fixture, "cross-domain-candidate-composition").unwrap();
    let comparison = compare_for_candidates(
        &fixture,
        &composition,
        ExpectedIndependentBehavior::Structured {
            value: serde_json::json!({"status":"independent","rule":"fixture"}),
        },
        "cross-domain-candidate",
    );
    let derive = || {
        fixture.engine.derive_cross_domain_deviations(
            &comparison.profile,
            "cross-domain-candidate-derive",
            Timestamp::new("2026-09-25T00:00:00Z"),
        )
    };
    let structures = derive().unwrap();
    assert_eq!(structures.len(), 1);
    assert_eq!(
        structures[0].value().kind,
        "cross_domain_independence_deviation"
    );
    assert_eq!(
        structures[0].provenance().inputs,
        vec![comparison.profile.id().clone()]
    );
    let evidence: unclip_engine::CrossDomainDeviationEvidence =
        serde_json::from_value(structures[0].value().value.clone()).unwrap();
    assert_eq!(evidence.comparison_profile, *comparison.profile.id());
    assert_eq!(evidence.binding, composition.value().product.binding);
    assert_eq!(
        evidence.comparison,
        comparison.profile.value().comparisons[0]
    );
    assert_eq!(derive().unwrap(), structures);

    let tracked = structures.iter().map(Tracked::from).collect::<Vec<_>>();
    let plan = fixture
        .engine
        .plan(&EngineProfile {
            candidate_generators: vec![PluginSelection::any("generate.cross-domain-structure")],
            ..EngineProfile::default()
        })
        .unwrap();
    let target = serde_json::to_string(&(
        &composition.value().product.binding.left.domain.0,
        &composition.value().product.binding.left.version.0,
    ))
    .unwrap();
    let generate = || {
        fixture.engine.generate_candidates(
            &plan,
            CandidateInputs {
                domain_version_id: &target,
                measurements: &[],
                observations: &[],
                structures: &tracked,
            },
            MeasurementRun {
                id: "cross-domain-candidate-generate",
                timestamp: Timestamp::new("2026-09-25T00:00:00Z"),
                params: &BTreeMap::new(),
            },
        )
    };
    let candidates = generate().unwrap();
    assert_eq!(candidates.len(), 1);
    let candidate = &candidates[0];
    assert_eq!(candidate.value().kind, CandidateKind::CrossDomainStructure);
    assert_eq!(candidate.value().domain_version_id, target);
    assert_eq!(
        candidate.value().value["pattern"]["matching"],
        "typed_product_deviation_from_independence"
    );
    assert_eq!(
        candidate.value().value["evidence"]["structure"],
        structures[0].id().0
    );
    assert_eq!(
        candidate.value().value["evidence"]["binding"],
        serde_json::to_value(&composition.value().product.binding).unwrap()
    );
    assert_eq!(
        candidate.value().value["evidence"]["typed_delta"],
        serde_json::to_value(&comparison.profile.value().comparisons[0].delta).unwrap()
    );
    assert!(!serde_json::to_string(candidate.value())
        .unwrap()
        .contains("\"label\""));
    assert_eq!(
        candidate.provenance().inputs,
        vec![structures[0].id().clone()]
    );
    assert_eq!(generate().unwrap(), candidates);

    let foreign = fixture.engine.generate_candidates(
        &plan,
        CandidateInputs {
            domain_version_id: "[\"foreign\",\"v1\"]",
            measurements: &[],
            observations: &[],
            structures: &tracked,
        },
        MeasurementRun {
            id: "cross-domain-candidate-foreign",
            timestamp: Timestamp::new("2026-09-25T00:00:00Z"),
            params: &BTreeMap::new(),
        },
    );
    assert!(foreign
        .unwrap_err()
        .to_string()
        .contains("one of the product source-domain versions"));
}

#[test]
fn equal_or_unavailable_product_comparisons_do_not_become_candidates() {
    let fixture = fixture("no-cross-domain-candidate");
    let composition = compose(&fixture, "no-cross-domain-candidate-composition").unwrap();
    let Reading::Value {
        value: MeasurementValue::Structured(observed),
    } = &composition.value().product.measurements[0]
        .measurement
        .reading
    else {
        panic!("fixture product measurement must be structured")
    };
    let equal = compare_for_candidates(
        &fixture,
        &composition,
        ExpectedIndependentBehavior::Structured {
            value: observed.clone(),
        },
        "equal-cross-domain-candidate",
    );
    assert!(fixture
        .engine
        .derive_cross_domain_deviations(
            &equal.profile,
            "equal-cross-domain-candidate-derive",
            Timestamp::new("2026-09-25T00:00:00Z"),
        )
        .unwrap()
        .is_empty());

    let unavailable = compare_for_candidates(
        &fixture,
        &composition,
        ExpectedIndependentBehavior::Undefined {
            measurement_kind: MeasurementKind::Structured,
            reason: "no validated rule".into(),
        },
        "unavailable-cross-domain-candidate",
    );
    assert!(fixture
        .engine
        .derive_cross_domain_deviations(
            &unavailable.profile,
            "unavailable-cross-domain-candidate-derive",
            Timestamp::new("2026-09-25T00:00:00Z"),
        )
        .unwrap()
        .is_empty());

    assert!(fixture
        .engine
        .registry()
        .candidate_generators()
        .any(|generator| generator.descriptor().id.0 == "generate.cross-domain-structure"));
}
