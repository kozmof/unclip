use serde_json::json;
use unclip_epistemic::{
    hash_params, DerivedId, DomainVersion, FrameVersion, ModelRef, Operation, PluginId, Provenance,
    SourceRef, Timestamp,
};
use unclip_store::{
    connect_and_migrate, ProvenanceRepository, SeaOrmProvenanceRepository, StoreError,
    StoredProvenance,
};

fn stored(id: &str, inputs: &[&str]) -> StoredProvenance {
    let params = json!({"threshold": 0.5});
    StoredProvenance {
        id: DerivedId::new(id),
        run_id: None,
        provenance: Provenance {
            operation: Operation::Calculated,
            producer: PluginId::new("test.producer"),
            algorithm: "fixture".into(),
            version: semver::Version::new(1, 2, 3),
            params_hash: hash_params(&params),
            params,
            inputs: inputs.iter().copied().map(DerivedId::new).collect(),
            source: Some(SourceRef::new("fixture")),
            timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
            domain_version: Some(DomainVersion::new("domain-v1")),
            frame_version: Some(FrameVersion::new("frame-v2")),
            model: Some(ModelRef::new("model-v3")),
        },
    }
}

#[tokio::test]
async fn metadata_and_dependency_graph_round_trip() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let repo = SeaOrmProvenanceRepository::new(db);

    let a = stored("a", &[]);
    repo.insert_provenance(a.clone()).await.unwrap();
    repo.insert_provenance(stored("b", &["a"])).await.unwrap();
    repo.insert_provenance(stored("c", &["a"])).await.unwrap();
    let d = stored("d", &["b", "c"]);
    repo.insert_provenance(d.clone()).await.unwrap();

    assert_eq!(
        repo.get_provenance(&DerivedId::new("a"))
            .await
            .unwrap()
            .unwrap(),
        a
    );
    assert_eq!(
        repo.get_provenance(&DerivedId::new("d"))
            .await
            .unwrap()
            .unwrap(),
        d
    );
    assert_eq!(
        repo.direct_inputs(&DerivedId::new("d")).await.unwrap(),
        vec![DerivedId::new("b"), DerivedId::new("c")]
    );
    assert_eq!(
        repo.ancestors(&DerivedId::new("d")).await.unwrap(),
        vec![
            DerivedId::new("b"),
            DerivedId::new("c"),
            DerivedId::new("a")
        ]
    );
}

#[tokio::test]
async fn missing_dependency_rolls_back_node_and_duplicate_is_typed() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let repo = SeaOrmProvenanceRepository::new(db);

    let missing = stored("derived", &["absent"]);
    let error = repo.insert_provenance(missing.clone()).await.unwrap_err();
    assert!(matches!(error, StoreError::NotFound { .. }));
    assert!(repo.get_provenance(&missing.id).await.unwrap().is_none());

    repo.insert_provenance(stored("input", &[])).await.unwrap();
    let value = stored("derived", &["input"]);
    repo.insert_provenance(value.clone()).await.unwrap();
    let error = repo.insert_provenance(value).await.unwrap_err();
    assert!(matches!(error, StoreError::AlreadyExists { .. }));
}

#[tokio::test]
async fn rejects_duplicate_inputs_before_writing() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let repo = SeaOrmProvenanceRepository::new(db);
    repo.insert_provenance(stored("input", &[])).await.unwrap();
    let duplicate = stored("derived", &["input", "input"]);

    let error = repo.insert_provenance(duplicate.clone()).await.unwrap_err();
    assert!(matches!(error, StoreError::InvalidRequest { .. }));
    assert!(repo.get_provenance(&duplicate.id).await.unwrap().is_none());
}
