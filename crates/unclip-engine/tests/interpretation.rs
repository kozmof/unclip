use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{json, Value};
use unclip_engine::{Engine, InterpretationRun};
use unclip_epistemic::{
    hash_params, DependencyCollector, DerivedId, EmitMetadata, InterpretationToken, ModelRef,
    Operation, PluginId, Provenance, Timestamp, Tracked,
};
use unclip_measure::EmpiricalStructure;
use unclip_plugin::{
    EngineProfile, InterpretationIo, InterpretationRequest, PluginSelection, Result,
};
use unclip_store::{
    connect_and_migrate, EmpiricalStructureRecord, MeasurementRepository, ProvenanceRepository,
    SeaOrmMeasurementRepository, SeaOrmProvenanceRepository, StoredProvenance,
};

struct FixtureIo;

#[async_trait]
impl InterpretationIo for FixtureIo {
    async fn request(&self, request: &InterpretationRequest) -> Result<Value> {
        assert_eq!(request.model, "fixture/semantic-labeler");
        assert_eq!(request.model_version, "2026-09-23");
        assert_eq!(request.parameters, json!({"temperature": 0, "seed": 7}));
        assert_eq!(request.structure.kind, "communities");
        assert_eq!(request.structure.value["members"], json!(["a", "b"]));
        Ok(json!({
            "label": "shared ritual",
            "explanation": "the detected community repeats a preparation pattern"
        }))
    }
}

fn source_provenance(id: &DerivedId) -> StoredProvenance {
    let params = json!({"threshold": 0.6});
    StoredProvenance {
        id: id.clone(),
        run_id: None,
        provenance: Provenance {
            operation: Operation::Calculated,
            producer: PluginId::new("empirical.communities"),
            algorithm: "empirical.communities".into(),
            version: semver::Version::new(0, 1, 0),
            params_hash: hash_params(&params),
            params,
            inputs: vec![],
            source: None,
            timestamp: Timestamp::new("2026-09-23T00:00:00Z"),
            domain_version: None,
            frame_version: None,
            model: None,
        },
    }
}

fn interpreter_params() -> Value {
    json!({
        "model": "fixture/semantic-labeler",
        "model_version": "2026-09-23",
        "context": "coffee preparation",
        "generation": {"temperature": 0, "seed": 7}
    })
}

#[tokio::test]
async fn versioned_model_parameters_and_stored_source_dependency_round_trip() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let provenance = SeaOrmProvenanceRepository::new(db.clone());
    let measurements = SeaOrmMeasurementRepository::new(db);
    let source_id = DerivedId::new("structure/communities/1");
    let structure = EmpiricalStructure {
        kind: "communities".into(),
        value: json!({"members": ["a", "b"]}),
    };
    provenance
        .insert_provenance(source_provenance(&source_id))
        .await
        .unwrap();
    measurements
        .insert_empirical_structure(EmpiricalStructureRecord {
            id: source_id.0.clone(),
            profile_id: None,
            provenance: source_id.clone(),
            created_at: "2026-09-23T00:00:00Z".into(),
            structure: structure.clone(),
        })
        .await
        .unwrap();

    let stored_source = measurements
        .get_empirical_structure(&source_id.0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored_source.structure, structure);
    let tracked = Tracked::from_recorded(stored_source.provenance, stored_source.structure);
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            interpreters: vec![PluginSelection::any("interpret.llm-label")],
            ..EngineProfile::default()
        })
        .unwrap();
    let params = [(PluginId::new("interpret.llm-label"), interpreter_params())]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let outputs = engine
        .interpret(
            &plan,
            &[tracked],
            InterpretationRun {
                id: "interpret-run",
                timestamp: Timestamp::new("2026-09-23T01:02:03Z"),
                params: &params,
                io: &FixtureIo,
            },
        )
        .await
        .unwrap();

    assert_eq!(outputs.len(), 1);
    let output = &outputs[0];
    assert_eq!(
        output.value()["structure"],
        serde_json::to_value(&structure).unwrap()
    );
    assert_eq!(output.value()["interpretation"]["label"], "shared ritual");
    assert_eq!(output.provenance().operation, Operation::Interpreted);
    assert_eq!(
        output.provenance().producer,
        PluginId::new("interpret.llm-label")
    );
    assert_eq!(output.provenance().version, semver::Version::new(1, 0, 0));
    assert_eq!(output.provenance().params, interpreter_params());
    assert_eq!(
        output.provenance().params_hash,
        hash_params(&interpreter_params())
    );
    assert_eq!(output.provenance().inputs, vec![source_id.clone()]);
    assert_eq!(
        output.provenance().model,
        Some(ModelRef::versioned(
            "fixture/semantic-labeler",
            "2026-09-23"
        ))
    );

    provenance
        .insert_provenance(StoredProvenance {
            id: output.id().clone(),
            run_id: None,
            provenance: output.provenance().clone(),
        })
        .await
        .unwrap();
    let reloaded = provenance
        .get_provenance(output.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.provenance, output.provenance().clone());
    assert_eq!(
        provenance.direct_inputs(output.id()).await.unwrap(),
        vec![source_id]
    );
}

struct UnexpectedIo;

#[async_trait]
impl InterpretationIo for UnexpectedIo {
    async fn request(&self, _: &InterpretationRequest) -> Result<Value> {
        panic!("invalid interpretation inputs must fail before model I/O")
    }
}

#[tokio::test]
async fn rejects_missing_run_or_sources_and_duplicate_source_ids() {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            interpreters: vec![PluginSelection::any("interpret.llm-label")],
            ..EngineProfile::default()
        })
        .unwrap();
    let params = [(PluginId::new("interpret.llm-label"), interpreter_params())]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let run = |id| InterpretationRun {
        id,
        timestamp: Timestamp::new("now"),
        params: &params,
        io: &UnexpectedIo,
    };
    assert!(engine.interpret(&plan, &[], run("run")).await.is_err());
    let source = Tracked::from_recorded(
        DerivedId::new("structure/1"),
        EmpiricalStructure {
            kind: "communities".into(),
            value: json!({}),
        },
    );
    assert!(engine
        .interpret(&plan, std::slice::from_ref(&source), run(""))
        .await
        .is_err());
    assert!(engine
        .interpret(&plan, &[source.clone(), source], run("run"))
        .await
        .is_err());

    let empty_plan = engine.plan(&EngineProfile::default()).unwrap();
    assert!(engine
        .interpret(&empty_plan, &[], run(""))
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn interpreted_structures_cannot_be_reused_as_measurement_evidence() {
    let params = interpreter_params();
    let interpreted = InterpretationToken::from_harness(
        EmitMetadata {
            id: DerivedId::new("interpreted/structure"),
            producer: PluginId::new("interpret.fixture"),
            algorithm: "interpret.fixture".into(),
            version: semver::Version::new(1, 0, 0),
            params_hash: hash_params(&params),
            params: params.clone(),
            source: None,
            timestamp: Timestamp::new("now"),
            domain_version: None,
            frame_version: None,
            model: Some(ModelRef::versioned(
                "fixture/semantic-labeler",
                "2026-09-23",
            )),
        },
        DependencyCollector::default(),
    )
    .emit(EmpiricalStructure {
        kind: "communities".into(),
        value: json!({"members": ["a", "b"]}),
    });
    let source = Tracked::from(&interpreted);
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            interpreters: vec![PluginSelection::any("interpret.llm-label")],
            ..EngineProfile::default()
        })
        .unwrap();
    let plugin_params = [(PluginId::new("interpret.llm-label"), params)]
        .into_iter()
        .collect::<BTreeMap<_, _>>();

    let error = engine
        .interpret(
            &plan,
            &[source],
            InterpretationRun {
                id: "run",
                timestamp: Timestamp::new("now"),
                params: &plugin_params,
                io: &UnexpectedIo,
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("must be calculated evidence"));
}
