use serde_json::json;
use std::collections::BTreeMap;
use unclip_engine::{Engine, MeasurementRun, ScalarDifference};
use unclip_epistemic::{Calculated, DerivedId, PluginId, Timestamp, Tracked};
use unclip_measure::{Delta, Measurement, MeasurementContext, MeasurementValue, Reading};
use unclip_plugin::{EngineProfile, PluginSelection};
fn measurement(reading: Reading) -> Measurement {
    Measurement {
        sensor: PluginId::new("sensor.fixture"),
        sensor_version: "1.0.0".parse().unwrap(),
        reading,
        confidence: None,
        sample_count: Some(4),
        context: MeasurementContext::default(),
    }
}
fn scalar(v: f64) -> Reading {
    Reading::Value {
        value: MeasurementValue::Scalar(v),
    }
}
fn compare(
    before: Measurement,
    after: Measurement,
    params: serde_json::Value,
) -> unclip_plugin::Result<Vec<Calculated<Delta>>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            comparators: vec![PluginSelection::any("compare.scalar-difference")],
            ..Default::default()
        })
        .unwrap();
    engine.compare_measurements(
        &plan,
        &Tracked::from_recorded(DerivedId::new("before"), before),
        &Tracked::from_recorded(DerivedId::new("after"), after),
        MeasurementRun {
            id: "compare",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new("compare.scalar-difference"), params)]),
        },
    )
}
fn payload(result: &[Calculated<Delta>]) -> ScalarDifference {
    let MeasurementValue::Structured(value) = &result[0].value().value else {
        panic!()
    };
    serde_json::from_value(value.clone()).unwrap()
}
#[test]
fn signed_differences_and_zero_replay_with_both_dependencies() {
    for (a, b) in [(3.0, 1.0), (0.0, 0.0), (-2.0, 3.0)] {
        let results = compare(measurement(scalar(a)), measurement(scalar(b)), json!({})).unwrap();
        assert_eq!(
            payload(&results),
            ScalarDifference::Value {
                before: a,
                after: b,
                difference: b - a
            }
        );
        assert_eq!(
            results[0].value().comparator,
            PluginId::new("compare.scalar-difference")
        );
        assert_eq!(
            results[0].provenance().inputs,
            vec![DerivedId::new("after"), DerivedId::new("before")]
        );
        assert_eq!(
            compare(measurement(scalar(a)), measurement(scalar(b)), json!({})).unwrap(),
            results
        );
    }
}
#[test]
fn unavailable_readings_are_preserved_and_structures_are_not_subtracted() {
    for sparse in [
        Reading::NotMeasured,
        Reading::NotApplicable {
            reason: "no axis".into(),
        },
        Reading::InsufficientEvidence { have: 1, need: 3 },
    ] {
        let result = compare(
            measurement(sparse.clone()),
            measurement(scalar(0.0)),
            json!({}),
        )
        .unwrap();
        assert_eq!(
            payload(&result),
            ScalarDifference::Unavailable {
                before: sparse,
                after: scalar(0.0)
            }
        );
    }
    let result = compare(
        measurement(Reading::Value {
            value: MeasurementValue::Structured(json!({"score":1})),
        }),
        measurement(scalar(2.0)),
        json!({}),
    )
    .unwrap();
    assert!(matches!(
        payload(&result),
        ScalarDifference::NotApplicable { .. }
    ));
}
#[test]
fn mismatched_semantics_invalid_configuration_and_nonfinite_results_fail() {
    let before = measurement(scalar(1.0));
    let mut sensor = before.clone();
    sensor.sensor = PluginId::new("other");
    let mut version = before.clone();
    version.sensor_version = "2.0.0".parse().unwrap();
    let mut context = before.clone();
    context.context.values.insert("unit".into(), json!("other"));
    for after in [sensor, version, context] {
        assert!(compare(before.clone(), after, json!({})).is_err());
    }
    for invalid in [f64::NAN, f64::INFINITY] {
        assert!(compare(
            measurement(scalar(invalid)),
            measurement(Reading::NotMeasured),
            json!({})
        )
        .is_err());
    }
    assert!(compare(
        measurement(scalar(-f64::MAX)),
        measurement(scalar(f64::MAX)),
        json!({})
    )
    .is_err());
    assert!(compare(before.clone(), before, json!({"weight":1})).is_err());
}
#[test]
fn explicit_selection_enforces_versions_and_records_comparator_configuration() {
    let engine = Engine::with_builtins().unwrap();
    let empty = engine.plan(&EngineProfile::default()).unwrap();
    assert!(empty.comparators.is_empty());
    let plan = engine
        .plan(&EngineProfile {
            comparators: vec![PluginSelection::any("compare.scalar-difference")],
            ..Default::default()
        })
        .unwrap();
    let record = engine.run_record(
        &plan,
        &BTreeMap::new(),
        "comparison",
        Timestamp::new("now"),
        json!({}),
    );
    assert_eq!(
        record.resolved_plan["comparators"][0]["id"],
        "compare.scalar-difference"
    );
    assert_eq!(record.resolved_plan["comparators"][0]["version"], "0.1.0");
    assert_eq!(record.resolved_plan["comparators"][0]["params"], json!({}));
    let mut selection = PluginSelection::any("compare.scalar-difference");
    selection.version = ">=1".parse().unwrap();
    assert!(engine
        .plan(&EngineProfile {
            comparators: vec![selection],
            ..Default::default()
        })
        .is_err());
}

fn ranking(units: &[&str], unknown: &[&str]) -> Reading {
    Reading::Value {
        value: MeasurementValue::Ranking(unclip_measure::RankedState {
            tiers: units
                .iter()
                .map(|id| vec![unclip_domain::UnitId::new(*id)])
                .collect(),
            unknown: unknown
                .iter()
                .map(|id| unclip_domain::UnitId::new(*id))
                .collect(),
            unresolved: vec![],
        }),
    }
}
fn compare_rank(
    id: &str,
    a: Reading,
    b: Reading,
    params: serde_json::Value,
) -> unclip_plugin::Result<Vec<Calculated<Delta>>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            comparators: vec![PluginSelection::any(id)],
            ..Default::default()
        })
        .unwrap();
    engine.compare_measurements(
        &plan,
        &Tracked::from_recorded(DerivedId::new("before"), measurement(a)),
        &Tracked::from_recorded(DerivedId::new("after"), measurement(b)),
        MeasurementRun {
            id: "ranks",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new(id), params)]),
        },
    )
}
fn ranking_payload(results: &[Calculated<Delta>]) -> unclip_engine::RankingComparison {
    let MeasurementValue::Structured(value) = &results[0].value().value else {
        panic!()
    };
    serde_json::from_value(value.clone()).unwrap()
}
#[test]
fn kendall_counts_inversions_while_rbo_measures_prefix_agreement() {
    use unclip_engine::RankingComparison;
    for (order, expected) in [
        (vec!["a", "b", "c"], 0),
        (vec!["b", "a", "c"], 1),
        (vec!["c", "b", "a"], 3),
    ] {
        let a = ranking(&["a", "b", "c"], &[]);
        let b = ranking(&order, &[]);
        let results = compare_rank("compare.kendall", a.clone(), b.clone(), json!({})).unwrap();
        let RankingComparison::Kendall {
            distance,
            discordant_pairs,
            pairs,
            ..
        } = ranking_payload(&results)
        else {
            panic!()
        };
        assert_eq!(discordant_pairs, expected);
        assert_eq!(pairs, 3);
        assert_eq!(distance, expected as f64 / 3.0);
        assert_eq!(
            results[0].provenance().inputs,
            vec![DerivedId::new("after"), DerivedId::new("before")]
        );
        assert_eq!(
            compare_rank("compare.kendall", a, b, json!({})).unwrap(),
            results
        );
    }
    for (right, expected) in [
        (vec!["a", "b"], 1.0),
        (vec!["b", "a"], 0.5),
        (vec!["c", "d"], 0.0),
    ] {
        let results = compare_rank(
            "compare.rbo",
            ranking(&["a", "b"], &["x"]),
            ranking(&right, &["y"]),
            json!({"p":0.5}),
        )
        .unwrap();
        let RankingComparison::Rbo {
            similarity,
            p,
            depth,
            before,
            after,
        } = ranking_payload(&results)
        else {
            panic!()
        };
        assert_eq!(similarity, expected);
        assert_eq!(p, 0.5);
        assert_eq!(depth, 2);
        assert_eq!(before.unknown, vec![unclip_domain::UnitId::new("x")]);
        assert_eq!(after.unknown, vec![unclip_domain::UnitId::new("y")]);
        assert_eq!(
            compare_rank(
                "compare.rbo",
                ranking(&["a", "b"], &["x"]),
                ranking(&right, &["y"]),
                json!({"p":0.5})
            )
            .unwrap(),
            results
        );
    }
}
#[test]
fn ranking_comparators_preserve_sparse_and_unsupported_shapes() {
    use unclip_engine::RankingComparison;
    let tied = Reading::Value {
        value: MeasurementValue::Ranking(unclip_measure::RankedState {
            tiers: vec![vec![
                unclip_domain::UnitId::new("a"),
                unclip_domain::UnitId::new("b"),
            ]],
            unknown: vec![],
            unresolved: vec![],
        }),
    };
    for (id, params) in [
        ("compare.kendall", json!({})),
        ("compare.rbo", json!({"p":0.9})),
    ] {
        let results = compare_rank(
            id,
            Reading::NotMeasured,
            ranking(&["a", "b"], &[]),
            params.clone(),
        )
        .unwrap();
        assert!(matches!(
            ranking_payload(&results),
            RankingComparison::Unavailable {
                before: Reading::NotMeasured,
                ..
            }
        ));
        let results =
            compare_rank(id, tied.clone(), ranking(&["a", "b"], &[]), params.clone()).unwrap();
        assert!(matches!(
            ranking_payload(&results),
            RankingComparison::NotApplicable { .. }
        ));
        assert!(compare_rank(
            id,
            ranking(&["a", "a"], &[]),
            ranking(&["a", "b"], &[]),
            params.clone()
        )
        .is_err());
        assert!(compare_rank(
            id,
            ranking(&["a"], &["a"]),
            ranking(&["a", "b"], &[]),
            params
        )
        .is_err());
    }
    for (id, a, b, params) in [
        (
            "compare.kendall",
            ranking(&["a"], &["b"]),
            ranking(&["a", "b"], &[]),
            json!({}),
        ),
        (
            "compare.kendall",
            ranking(&["a", "b"], &[]),
            ranking(&["a", "c"], &[]),
            json!({}),
        ),
        (
            "compare.rbo",
            ranking(&["a"], &[]),
            ranking(&["a", "b"], &[]),
            json!({"p":0.9}),
        ),
    ] {
        assert!(matches!(
            ranking_payload(&compare_rank(id, a, b, params).unwrap()),
            RankingComparison::NotApplicable { .. }
        ));
    }
    for params in [
        json!({}),
        json!({"p":0}),
        json!({"p":1}),
        json!({"p":0.9,"weighted":true}),
    ] {
        assert!(compare_rank(
            "compare.rbo",
            ranking(&["a"], &[]),
            ranking(&["a"], &[]),
            params
        )
        .is_err());
    }
}

fn distribution(values: &[(&str, f64)]) -> Reading {
    Reading::Value {
        value: MeasurementValue::Distribution(
            values
                .iter()
                .map(|(name, value)| ((*name).into(), *value))
                .collect(),
        ),
    }
}
fn distribution_payload(results: &[Calculated<Delta>]) -> unclip_engine::DistributionComparison {
    let MeasurementValue::Structured(value) = &results[0].value().value else {
        panic!()
    };
    serde_json::from_value(value.clone()).unwrap()
}
#[test]
fn jensen_shannon_retains_categories_and_explicit_normalization() {
    use unclip_engine::DistributionComparison;
    for (a, b, expected) in [
        (vec![("a", 1.0)], vec![("a", 1.0)], 0.0),
        (vec![("a", 1.0)], vec![("b", 1.0)], 1.0),
        (
            vec![("b", 0.5), ("a", 0.5)],
            vec![("a", 0.5), ("b", 0.5)],
            0.0,
        ),
    ] {
        let result = compare_rank(
            "compare.jensen-shannon",
            distribution(&a),
            distribution(&b),
            json!({"normalization":"probability"}),
        )
        .unwrap();
        let DistributionComparison::Value {
            divergence_bits, ..
        } = distribution_payload(&result)
        else {
            panic!()
        };
        assert_eq!(divergence_bits, expected);
        assert_eq!(
            result[0].provenance().inputs,
            vec![DerivedId::new("after"), DerivedId::new("before")]
        );
        assert_eq!(
            compare_rank(
                "compare.jensen-shannon",
                distribution(&a),
                distribution(&b),
                json!({"normalization":"probability"})
            )
            .unwrap(),
            result
        );
    }
    let result = compare_rank(
        "compare.jensen-shannon",
        distribution(&[("b", 3.0), ("a", 1.0), ("zero", 0.0)]),
        distribution(&[("a", 2.0), ("b", 6.0)]),
        json!({"normalization":"mass"}),
    )
    .unwrap();
    let DistributionComparison::Value {
        divergence_bits,
        categories,
        before_total,
        after_total,
        before_probabilities,
        after_probabilities,
        ..
    } = distribution_payload(&result)
    else {
        panic!()
    };
    assert_eq!(divergence_bits, 0.0);
    assert_eq!(categories, vec!["a", "b", "zero"]);
    assert_eq!(before_total, 4.0);
    assert_eq!(after_total, 8.0);
    assert_eq!(before_probabilities, vec![0.25, 0.75, 0.0]);
    assert_eq!(after_probabilities, before_probabilities);
    let reordered = compare_rank(
        "compare.jensen-shannon",
        distribution(&[("zero", 0.0), ("a", 1.0), ("b", 3.0)]),
        distribution(&[("b", 6.0), ("a", 2.0)]),
        json!({"normalization":"mass"}),
    )
    .unwrap();
    assert_eq!(result, reordered);
}
#[test]
fn distribution_divergence_is_symmetric_and_distinguishes_sparse_from_zero() {
    use unclip_engine::DistributionComparison;
    let a = distribution(&[("a", 0.25), ("b", 0.75)]);
    let b = distribution(&[("a", 0.75), ("b", 0.25)]);
    let score = |a: Reading, b: Reading| {
        let results = compare_rank(
            "compare.jensen-shannon",
            a,
            b,
            json!({"normalization":"probability"}),
        )
        .unwrap();
        let DistributionComparison::Value {
            divergence_bits, ..
        } = distribution_payload(&results)
        else {
            panic!()
        };
        divergence_bits
    };
    let forward = score(a.clone(), b.clone());
    assert!((forward - 0.18872187554086717).abs() < 1e-14);
    assert_eq!(forward, score(b, a));
    for a in [
        distribution(&[]),
        distribution(&[("a", 0.0)]),
        Reading::NotMeasured,
        Reading::InsufficientEvidence { have: 0, need: 2 },
    ] {
        let result = compare_rank(
            "compare.jensen-shannon",
            a,
            distribution(&[("a", 1.0)]),
            json!({"normalization":"mass"}),
        )
        .unwrap();
        assert!(matches!(
            distribution_payload(&result),
            DistributionComparison::Unavailable { .. }
        ));
    }
    let result = compare_rank(
        "compare.jensen-shannon",
        scalar(1.0),
        distribution(&[("a", 1.0)]),
        json!({"normalization":"mass"}),
    )
    .unwrap();
    assert!(matches!(
        distribution_payload(&result),
        DistributionComparison::NotApplicable { .. }
    ));
    assert_eq!(
        score(
            distribution(&[("tiny", f64::from_bits(1)), ("a", 1.0)]),
            distribution(&[("a", 1.0)])
        ),
        0.0
    );
}
#[test]
fn invalid_distribution_data_and_implicit_normalization_are_rejected() {
    for bad in [
        vec![("a", -1.0)],
        vec![("a", f64::NAN)],
        vec![("a", f64::INFINITY)],
        vec![("a", f64::MAX), ("b", f64::MAX)],
        vec![("a", 1.0), ("a", 0.0)],
        vec![("", 1.0)],
    ] {
        assert!(compare_rank(
            "compare.jensen-shannon",
            distribution(&bad),
            distribution(&[("a", 1.0)]),
            json!({"normalization":"mass"})
        )
        .is_err());
    }
    assert!(compare_rank(
        "compare.jensen-shannon",
        distribution(&[("a", 2.0)]),
        distribution(&[("a", 1.0)]),
        json!({"normalization":"probability"})
    )
    .is_err());
    for params in [
        json!({}),
        json!({"normalization":"automatic"}),
        json!({"normalization":"mass","smoothing":1}),
    ] {
        assert!(compare_rank(
            "compare.jensen-shannon",
            distribution(&[("a", 1.0)]),
            distribution(&[("a", 1.0)]),
            params
        )
        .is_err());
    }
}

fn pairwise(metric: &str, units: &[&str], cell: serde_json::Value) -> Reading {
    let diagonal = json!({"status":"value","value":1.0,"sample_count":4});
    Reading::Value {value:MeasurementValue::PairwiseMatrix(serde_json::from_value(json!({"metric":metric,"units":units,"cells":[[diagonal.clone(),cell.clone()],[cell,diagonal]]})).unwrap())}
}
fn matrix_payload(results: &[Calculated<Delta>]) -> unclip_engine::MatrixComparison {
    let MeasurementValue::Structured(value) = &results[0].value().value else {
        panic!()
    };
    serde_json::from_value(value.clone()).unwrap()
}
#[test]
fn matrix_deltas_preserve_signed_cells_and_per_cell_evidence() {
    use unclip_engine::{MatrixCellDifference, MatrixComparison};
    let a = pairwise(
        "spearman",
        &["a", "b"],
        json!({"status":"value","value":0.5,"sample_count":4}),
    );
    let b = pairwise(
        "spearman",
        &["a", "b"],
        json!({"status":"value","value":-0.5,"sample_count":3}),
    );
    let result = compare_rank(
        "compare.pairwise-matrix",
        a.clone(),
        b.clone(),
        json!({"minimum_samples":3}),
    )
    .unwrap();
    let MatrixComparison::Value { cells, units, .. } = matrix_payload(&result) else {
        panic!()
    };
    assert_eq!(
        units,
        vec![
            unclip_domain::UnitId::new("a"),
            unclip_domain::UnitId::new("b")
        ]
    );
    assert!(matches!(
        cells[0][0],
        MatrixCellDifference::Value {
            difference: 0.0,
            ..
        }
    ));
    assert!(matches!(
        cells[0][1],
        MatrixCellDifference::Value {
            difference: -1.0,
            after: unclip_measure::MatrixCell::Value {
                sample_count: 3,
                ..
            },
            ..
        }
    ));
    assert_eq!(cells[0][1], cells[1][0]);
    assert_eq!(
        result[0].provenance().inputs,
        vec![DerivedId::new("after"), DerivedId::new("before")]
    );
    assert_eq!(
        compare_rank(
            "compare.pairwise-matrix",
            a.clone(),
            b.clone(),
            json!({"minimum_samples":3})
        )
        .unwrap(),
        result
    );
    let stricter = compare_rank(
        "compare.pairwise-matrix",
        a,
        b,
        json!({"minimum_samples":4}),
    )
    .unwrap();
    let MatrixComparison::Value { cells, .. } = matrix_payload(&stricter) else {
        panic!()
    };
    assert!(matches!(
        cells[0][1],
        MatrixCellDifference::Unavailable {
            after: unclip_measure::MatrixCell::Value {
                sample_count: 3,
                ..
            },
            ..
        }
    ));
}
#[test]
fn matrix_missing_cells_remain_distinct_and_incompatible_axes_are_rejected() {
    use unclip_engine::{MatrixCellDifference, MatrixComparison};
    let a = pairwise(
        "spearman",
        &["a", "b"],
        json!({"status":"undefined","sample_count":4}),
    );
    let b = pairwise(
        "spearman",
        &["a", "b"],
        json!({"status":"insufficient_evidence","have":1,"need":2}),
    );
    let result = compare_rank(
        "compare.pairwise-matrix",
        a.clone(),
        b,
        json!({"minimum_samples":2}),
    )
    .unwrap();
    let MatrixComparison::Value { cells, .. } = matrix_payload(&result) else {
        panic!()
    };
    assert!(matches!(
        cells[0][1],
        MatrixCellDifference::Unavailable {
            before: unclip_measure::MatrixCell::Undefined { sample_count: 4 },
            after: unclip_measure::MatrixCell::InsufficientEvidence { have: 1, need: 2 }
        }
    ));
    for (metric, units) in [("kendall", vec!["a", "b"]), ("spearman", vec!["a", "c"])] {
        let b = pairwise(
            metric,
            &units,
            json!({"status":"undefined","sample_count":4}),
        );
        assert!(compare_rank(
            "compare.pairwise-matrix",
            a.clone(),
            b,
            json!({"minimum_samples":2})
        )
        .is_err());
    }
    let raw = Reading::Value {
        value: MeasurementValue::Matrix(vec![vec![1.0]]),
    };
    let result = compare_rank(
        "compare.pairwise-matrix",
        a.clone(),
        raw,
        json!({"minimum_samples":2}),
    )
    .unwrap();
    assert!(matches!(
        matrix_payload(&result),
        MatrixComparison::NotApplicable { .. }
    ));
    let result = compare_rank(
        "compare.pairwise-matrix",
        a.clone(),
        Reading::NotMeasured,
        json!({"minimum_samples":2}),
    )
    .unwrap();
    assert!(matches!(
        matrix_payload(&result),
        MatrixComparison::Unavailable {
            after: Reading::NotMeasured,
            ..
        }
    ));
    for params in [
        json!({}),
        json!({"minimum_samples":1}),
        json!({"minimum_samples":2,"aggregate":true}),
    ] {
        assert!(compare_rank("compare.pairwise-matrix", a.clone(), a.clone(), params).is_err());
    }
}

fn spectral_payload(results: &[Calculated<Delta>]) -> unclip_engine::SpectralComparison {
    let MeasurementValue::Structured(value) = &results[0].value().value else {
        panic!()
    };
    serde_json::from_value(value.clone()).unwrap()
}
fn spectral_params() -> serde_json::Value {
    json!({"minimum_samples":2,"tolerance":1e-12,"max_sweeps":100})
}
#[test]
fn spectrum_retains_signed_eigenvalues_and_compares_order_not_factor_identity() {
    use unclip_engine::SpectralComparison;
    let matrix = |v| {
        pairwise(
            "relative_rank_variance",
            &["a", "b"],
            json!({"status":"value","value":v,"sample_count":4}),
        )
    };
    let result = compare_rank(
        "compare.spectrum",
        matrix(2.0),
        matrix(3.0),
        spectral_params(),
    )
    .unwrap();
    let SpectralComparison::Value {
        before,
        after,
        eigenvalue_differences,
        ..
    } = spectral_payload(&result)
    else {
        panic!()
    };
    assert!((before.eigenpairs[0].eigenvalue - 3.0).abs() < 1e-12);
    assert!((before.eigenpairs[1].eigenvalue + 1.0).abs() < 1e-12);
    assert!((after.eigenpairs[1].eigenvalue + 2.0).abs() < 1e-12);
    assert!((eigenvalue_differences[0] - 1.0).abs() < 1e-12);
    assert!((eigenvalue_differences[1] + 1.0).abs() < 1e-12);
    assert_eq!(
        result[0].provenance().inputs,
        vec![DerivedId::new("after"), DerivedId::new("before")]
    );
    assert_eq!(
        compare_rank(
            "compare.spectrum",
            matrix(2.0),
            matrix(3.0),
            spectral_params()
        )
        .unwrap(),
        result
    );
    // Different loading directions can have the same spectrum.
    let positive = pairwise(
        "spearman",
        &["a", "b"],
        json!({"status":"value","value":0.5,"sample_count":4}),
    );
    let negative = pairwise(
        "spearman",
        &["a", "b"],
        json!({"status":"value","value":-0.5,"sample_count":4}),
    );
    let result = compare_rank("compare.spectrum", positive, negative, spectral_params()).unwrap();
    let SpectralComparison::Value {
        before,
        after,
        eigenvalue_differences,
        ..
    } = spectral_payload(&result)
    else {
        panic!()
    };
    assert!(eigenvalue_differences.iter().all(|v| v.abs() < 1e-12));
    assert_ne!(before.eigenpairs[0].loadings, after.eigenpairs[0].loadings);
}
#[test]
fn spectral_sparse_and_incompatible_evidence_is_not_completed() {
    use unclip_engine::SpectralComparison;
    let full = pairwise(
        "spearman",
        &["a", "b"],
        json!({"status":"value","value":0.5,"sample_count":4}),
    );
    let sparse = pairwise(
        "spearman",
        &["a", "b"],
        json!({"status":"undefined","sample_count":4}),
    );
    for after in [sparse, Reading::NotMeasured] {
        let result =
            compare_rank("compare.spectrum", full.clone(), after, spectral_params()).unwrap();
        assert!(matches!(
            spectral_payload(&result),
            SpectralComparison::Unavailable { .. }
        ));
    }
    let result = compare_rank(
        "compare.spectrum",
        full.clone(),
        full.clone(),
        json!({"minimum_samples":5,"tolerance":1e-12,"max_sweeps":100}),
    )
    .unwrap();
    assert!(matches!(
        spectral_payload(&result),
        SpectralComparison::Unavailable { .. }
    ));
    for (key, value) in [
        ("minimum_samples", json!(1)),
        ("tolerance", json!(0)),
        ("max_sweeps", json!(0)),
    ] {
        let mut params = spectral_params();
        params[key] = value;
        assert!(compare_rank("compare.spectrum", full.clone(), full.clone(), params).is_err());
    }
    let other = pairwise(
        "kendall",
        &["a", "b"],
        json!({"status":"value","value":0.5,"sample_count":4}),
    );
    assert!(compare_rank("compare.spectrum", full, other, spectral_params()).is_err());
}

#[test]
fn spectrum_never_returns_an_unconverged_partial_result() {
    let values = [
        [0.0, 1.0, 3.0, 2.0],
        [1.0, 0.0, 2.0, 4.0],
        [3.0, 2.0, 0.0, 1.0],
        [2.0, 4.0, 1.0, 0.0],
    ];
    let cells = values
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| json!({"status":"value","value":value,"sample_count":4}))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let matrix = Reading::Value {
        value: MeasurementValue::PairwiseMatrix(
            serde_json::from_value(
                json!({"metric":"relative_rank_variance","units":["a","b","c","d"],"cells":cells}),
            )
            .unwrap(),
        ),
    };
    let error = compare_rank(
        "compare.spectrum",
        matrix.clone(),
        matrix,
        json!({"minimum_samples":2,"tolerance":1e-15,"max_sweeps":1}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("did not converge"));
}
