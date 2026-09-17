use std::collections::BTreeMap;

use sea_orm::{ActiveValue::Set, DatabaseConnection, EntityTrait};
use serde_json::json;
use unclip_domain::{DomainId, DomainSnapshot, Unit, UnitId, UnitKind};
use unclip_entity::provenance;
use unclip_epistemic::{DerivedId, DomainVersion, SourceRef};
use unclip_observe::{
    Alignment, AlignmentCandidate, Observation, ObservationId, ObservedRelation,
    ObservedRelationId, ObservedUnit, ObservedUnitId, PartialRanking, RankTier,
};
use unclip_store::{
    connect_and_migrate, DomainWriter, ObservationRepository, SeaOrmDomainRepository,
    SeaOrmObservationRepository, StoreError,
};

async fn add_provenance(db: &DatabaseConnection, id: &str) {
    provenance::Entity::insert(provenance::ActiveModel {
        derived_id: Set(id.into()),
        run_id: Set(None),
        operation: Set("inferred".into()),
        producer: Set("test".into()),
        algorithm: Set("fixture".into()),
        version: Set("1.0.0".into()),
        params_json: Set("{}".into()),
        params_hash: Set("fixture".into()),
        source: Set(None),
        timestamp: Set("2026-09-17T00:00:00Z".into()),
        domain_version: Set(None),
        frame_version: Set(None),
        model: Set(None),
    })
    .exec(db)
    .await
    .unwrap();
}

fn domain() -> DomainSnapshot {
    let units = ["d1", "d2"]
        .into_iter()
        .map(|id| {
            let id = UnitId::new(id);
            (
                id.clone(),
                Unit {
                    id,
                    kind: UnitKind::AtomicMeaning,
                    label: None,
                    properties: BTreeMap::new(),
                },
            )
        })
        .collect();
    DomainSnapshot {
        id: DomainId::new("domain"),
        version: DomainVersion::new("1"),
        units,
        relations: BTreeMap::new(),
    }
}

fn observation() -> Observation {
    let units = ["o1", "o2", "o3"]
        .into_iter()
        .map(|id| ObservedUnit {
            id: ObservedUnitId::new(id),
            label: id.to_uppercase(),
            salience: Some(0.5),
            uncertainty: Some(0.1),
            context: [("detail".into(), json!(id))].into_iter().collect(),
        })
        .collect();
    Observation {
        id: ObservationId::new("observation"),
        source: SourceRef::new("fixture"),
        observed_at: Some("2026-09-17T00:00:00Z".into()),
        units,
        relations: vec![ObservedRelation {
            id: ObservedRelationId::new("r1"),
            source: ObservedUnitId::new("o1"),
            target: ObservedUnitId::new("o2"),
            kind: "before".into(),
            uncertainty: Some(0.2),
        }],
        context: [("locale".into(), json!("en"))].into_iter().collect(),
    }
}

#[tokio::test]
async fn observation_alignment_and_partial_ranking_round_trip() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let provenance = DerivedId::new("derived");
    add_provenance(&db, &provenance.0).await;
    SeaOrmDomainRepository::new(db.clone())
        .insert_domain_version(domain())
        .await
        .unwrap();
    let repo = SeaOrmObservationRepository::new(db);
    let expected_observation = observation();

    repo.insert_observation(expected_observation.clone(), &provenance)
        .await
        .unwrap();
    assert_eq!(
        repo.get_observation(&expected_observation.id)
            .await
            .unwrap()
            .unwrap(),
        expected_observation
    );

    let alignment = Alignment {
        observation: expected_observation.id.clone(),
        candidates: vec![
            AlignmentCandidate {
                observed: ObservedUnitId::new("o1"),
                domain: UnitId::new("d1"),
                confidence: 0.8,
                evidence: vec!["lexical".into()],
            },
            AlignmentCandidate {
                observed: ObservedUnitId::new("o1"),
                domain: UnitId::new("d2"),
                confidence: 0.8,
                evidence: vec!["context".into()],
            },
        ],
    };
    repo.insert_alignment(
        "alignment",
        alignment.clone(),
        &DomainId::new("domain"),
        &DomainVersion::new("1"),
        &provenance,
    )
    .await
    .unwrap();
    assert_eq!(
        repo.get_alignment("alignment").await.unwrap().unwrap(),
        alignment
    );

    let ranking = PartialRanking {
        observation: expected_observation.id,
        tiers: vec![RankTier {
            units: vec![ObservedUnitId::new("o1"), ObservedUnitId::new("o2")],
        }],
        unknown: vec![ObservedUnitId::new("o3")],
    };
    repo.insert_ranking("ranking", ranking.clone(), &provenance)
        .await
        .unwrap();
    assert_eq!(repo.get_ranking("ranking").await.unwrap().unwrap(), ranking);
}

#[tokio::test]
async fn invalid_observation_is_rolled_back() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let provenance = DerivedId::new("derived");
    add_provenance(&db, &provenance.0).await;
    let repo = SeaOrmObservationRepository::new(db);
    let mut invalid = observation();
    invalid.units[1].uncertainty = Some(2.0);

    let error = repo
        .insert_observation(invalid.clone(), &provenance)
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::InvalidRequest { .. }));
    assert!(repo.get_observation(&invalid.id).await.unwrap().is_none());
}

#[tokio::test]
async fn missing_provenance_prevents_any_observation_rows() {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let repo = SeaOrmObservationRepository::new(db);
    let value = observation();

    repo.insert_observation(value.clone(), &DerivedId::new("missing"))
        .await
        .unwrap_err();

    assert!(repo.get_observation(&value.id).await.unwrap().is_none());
}
