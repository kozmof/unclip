use serde_json::{json, Value};
use std::collections::BTreeMap;
use unclip_domain::CandidateKind;
use unclip_engine::{CandidateInputs, Engine, MeasurementRun};
use unclip_epistemic::{DerivedId, PluginId, Timestamp, Tracked};
use unclip_measure::EmpiricalStructure;
use unclip_plugin::{EngineProfile, PluginSelection};

fn generate(
    id: &str,
    values: &[Tracked<EmpiricalStructure>],
    params: Value,
) -> unclip_plugin::Result<Vec<unclip_epistemic::Calculated<unclip_domain::CandidateProposal>>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            candidate_generators: vec![PluginSelection::any(id)],
            ..Default::default()
        })
        .unwrap();
    engine.generate_candidates(
        &plan,
        CandidateInputs {
            domain_version_id: "d1",
            structures: values,
            measurements: &[],
            observations: &[],
        },
        MeasurementRun {
            id: "discovery",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new(id), params)]),
        },
    )
}
fn input(id: &str, kind: &str, value: Value) -> Tracked<EmpiricalStructure> {
    Tracked::from_recorded(
        DerivedId::new(id),
        EmpiricalStructure {
            kind: kind.into(),
            value,
        },
    )
}
fn community() -> Value {
    json!({"metric":"spearman","threshold":0.8,"minimum_samples":2,"communities":[["a","b"],["c"]],"assessed_pairs":2,"qualifying_pairs":1,"unassessed":[{"left":"a","right":"c","evidence":{"status":"undefined","sample_count":4}}]})
}
fn spectral() -> Value {
    let q = std::f64::consts::FRAC_1_SQRT_2;
    json!({"metric":"relative_rank_variance","units":["a","b"],"eigenpairs":[{"eigenvalue":1.0,"loadings":[q,q]},{"eigenvalue":-1.0,"loadings":[q,-q]}],"minimum_cell_samples":4,"tolerance":1e-12,"sweeps":1})
}
fn community_params() -> Value {
    json!({"metric":"spearman","minimum_samples":2,"minimum_members":2})
}
fn latent_params() -> Value {
    json!({"metric":"relative_rank_variance","minimum_samples":2,"minimum_absolute_eigenvalue":0.8})
}
#[test]
fn community_preserves_partition_and_missing_evidence() {
    let values = [input("g", "communities", community())];
    let candidates = generate("generate.community", &values, community_params()).unwrap();
    assert_eq!(candidates.len(), 1);
    let candidate = &candidates[0];
    assert_eq!(candidate.value().kind, CandidateKind::CompositeMeaning);
    assert_eq!(
        candidate.value().value["pattern"]["members"],
        json!(["a", "b"])
    );
    assert_eq!(candidate.value().value["evidence"]["result"], community());
    assert_eq!(candidate.provenance().inputs, vec![DerivedId::new("g")]);
    assert!(!candidate.value().value.contains_key("label"));
    for (key, value) in [
        ("minimum_members", json!(3)),
        ("minimum_samples", json!(3)),
        ("metric", json!("kendall")),
    ] {
        let mut params = community_params();
        params[key] = value;
        assert!(generate("generate.community", &values, params)
            .unwrap()
            .is_empty());
    }
}
#[test]
fn latent_axes_preserve_negative_eigenvalues_and_signed_loadings() {
    let values = [
        input("z", "spectral", spectral()),
        input("a", "spectral", spectral()),
    ];
    let candidates = generate("generate.latent-axis", &values, latent_params()).unwrap();
    assert_eq!(candidates.len(), 4);
    assert_eq!(candidates[1].value().kind, CandidateKind::LatentAxis);
    assert_eq!(candidates[1].value().value["pattern"]["eigenvalue"], -1.0);
    assert_eq!(
        candidates[1].value().value["pattern"]["loadings"],
        spectral()["eigenpairs"][1]["loadings"]
    );
    assert_eq!(
        candidates[0].provenance().inputs,
        vec![DerivedId::new("a"), DerivedId::new("z")]
    );
    let reversed = [
        input("a", "spectral", spectral()),
        input("z", "spectral", spectral()),
    ];
    assert_eq!(
        generate("generate.latent-axis", &reversed, latent_params()).unwrap(),
        candidates
    );
    for (key, value) in [
        ("minimum_absolute_eigenvalue", json!(2)),
        ("minimum_samples", json!(5)),
        ("metric", json!("kendall")),
    ] {
        let mut params = latent_params();
        params[key] = value;
        assert!(generate("generate.latent-axis", &values, params)
            .unwrap()
            .is_empty());
    }
}
#[test]
fn malformed_evidence_and_duplicate_sources_are_rejected() {
    for (key, value) in [
        ("communities", json!([["a", "a"], ["c"]])),
        ("assessed_pairs", json!(3)),
        ("qualifying_pairs", json!(0)),
        ("label", json!("invented")),
    ] {
        let mut value0 = community();
        value0[key] = value;
        assert!(generate(
            "generate.community",
            &[input("g", "communities", value0)],
            community_params()
        )
        .is_err());
    }
    for (key, value) in [
        ("units", json!(["a"])),
        ("minimum_cell_samples", json!(1)),
        ("tolerance", json!(0)),
        (
            "eigenpairs",
            json!([{"eigenvalue":1,"loadings":[1,1]},{"eigenvalue":-1,"loadings":[1,1]}]),
        ),
    ] {
        let mut value0 = spectral();
        value0[key] = value;
        assert!(generate(
            "generate.latent-axis",
            &[input("g", "spectral", value0)],
            latent_params()
        )
        .is_err());
    }
    assert!(generate(
        "generate.community",
        &[
            input("g", "communities", community()),
            input("g", "communities", community())
        ],
        community_params()
    )
    .is_err());
    for (id, params) in [
        ("generate.community", community_params()),
        ("generate.latent-axis", latent_params()),
    ] {
        assert!(generate(id, &[], params).unwrap().is_empty());
    }
}
