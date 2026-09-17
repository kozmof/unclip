use std::collections::BTreeMap;

use serde_json::json;
use unclip_domain::{
    DomainId, DomainSnapshot, PropertyValue, Relation, RelationId, Unit, UnitId, UnitKind,
};
use unclip_epistemic::DomainVersion;
use unclip_store::{
    connect_and_migrate, DomainReader, DomainWriter, SeaOrmDomainRepository, StoreError,
};

fn snapshot() -> DomainSnapshot {
    let source = UnitId::new("source");
    let target = UnitId::new("target");
    let mut source_properties = BTreeMap::new();
    source_properties.insert("enabled".into(), PropertyValue::Boolean(true));
    source_properties.insert("count".into(), PropertyValue::Integer(3));
    source_properties.insert("weight".into(), PropertyValue::Number(0.75));
    source_properties.insert("note".into(), PropertyValue::Text("stable".into()));
    source_properties.insert(
        "metadata".into(),
        PropertyValue::Structured(json!({"tags": ["a", "b"]})),
    );

    let units = [
        (
            source.clone(),
            Unit {
                id: source.clone(),
                kind: UnitKind::AtomicMeaning,
                label: Some("Source".into()),
                properties: source_properties,
            },
        ),
        (
            target.clone(),
            Unit {
                id: target.clone(),
                kind: UnitKind::CrossDomainStructure,
                label: None,
                properties: BTreeMap::new(),
            },
        ),
    ]
    .into_iter()
    .collect();

    let relation_id = RelationId::new("connects");
    let relations = [(
        relation_id.clone(),
        Relation {
            id: relation_id,
            source,
            target,
            kind: "association".into(),
            properties: [("confidence".into(), PropertyValue::Number(0.9))]
                .into_iter()
                .collect(),
        },
    )]
    .into_iter()
    .collect();

    DomainSnapshot {
        id: DomainId::new("example"),
        version: DomainVersion::new("1.0"),
        units,
        relations,
    }
}

#[tokio::test]
async fn domain_snapshot_round_trips_with_typed_properties() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let repo = SeaOrmDomainRepository::new(db);
    let expected = snapshot();

    repo.insert_domain_version(expected.clone()).await.unwrap();

    let actual = repo
        .get_domain_version(&expected.id, &expected.version)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn missing_and_duplicate_versions_are_explicit() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let repo = SeaOrmDomainRepository::new(db);
    let expected = snapshot();

    assert!(repo
        .get_domain_version(&expected.id, &expected.version)
        .await
        .unwrap()
        .is_none());

    repo.insert_domain_version(expected.clone()).await.unwrap();
    let error = repo.insert_domain_version(expected).await.unwrap_err();
    assert!(matches!(error, StoreError::AlreadyExists { .. }));
}

#[tokio::test]
async fn invalid_property_rolls_back_the_whole_version() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let repo = SeaOrmDomainRepository::new(db);
    let mut invalid = snapshot();
    invalid
        .units
        .get_mut(&UnitId::new("source"))
        .unwrap()
        .properties
        .insert("invalid".into(), PropertyValue::Number(f64::NAN));

    let error = repo
        .insert_domain_version(invalid.clone())
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::InvalidRequest { .. }));
    assert!(repo
        .get_domain_version(&invalid.id, &invalid.version)
        .await
        .unwrap()
        .is_none());
}
