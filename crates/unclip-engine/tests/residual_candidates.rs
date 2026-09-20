use std::collections::BTreeMap;
use unclip_domain::{CandidateKind, DomainId, DomainSnapshot, FrameId, MeasurementFrame};
use unclip_engine::{CandidateInputs, Engine, MeasurementInputs, MeasurementRun};
use unclip_epistemic::{
    hash_params, Calculated, DependencyCollector, DerivedId, DomainVersion, EmitMetadata,
    FrameVersion, InferenceToken, PluginId, SourceRef, Timestamp, Tracked,
};
use unclip_measure::{Measurement, MeasurementValue, Reading};
use unclip_observe::{Alignment, Observation, ObservationId, ObservedUnit, ObservedUnitId};
use unclip_plugin::{EngineProfile, PluginSelection};
use unclip_store::{
    CandidateRepository, DomainReader, DomainWriter, ProvenanceRepository, SeaOrmDomainRepository,
    SeaOrmExperimentRepository, SeaOrmProvenanceRepository, StoredProvenance,
};

struct Fixture {
    domain: DomainSnapshot,
    observations: Vec<Tracked<Observation>>,
    measurements: Vec<Calculated<Measurement>>,
    provenance: Vec<StoredProvenance>,
}
fn metadata(id: &str) -> EmitMetadata {
    EmitMetadata {
        id: DerivedId::new(id),
        producer: PluginId::new("infer.fixture"),
        algorithm: "fixture".into(),
        version: "0.1.0".parse().unwrap(),
        params: serde_json::json!({}),
        params_hash: hash_params(&serde_json::json!({})),
        source: Some(SourceRef::new("fixture")),
        timestamp: Timestamp::new("now"),
        domain_version: None,
        frame_version: None,
        model: None,
    }
}
fn fixture() -> Fixture {
    let domain = DomainSnapshot {
        id: DomainId::new("test"),
        version: DomainVersion::new("1"),
        units: BTreeMap::new(),
        relations: BTreeMap::new(),
    };
    let mut observations = Vec::new();
    let mut alignments = Vec::new();
    let mut provenance = Vec::new();
    for (index, labels) in [
        vec!["echo", "echo", "one-off"],
        vec!["echo", "other"],
        vec!["Echo"],
    ]
    .into_iter()
    .enumerate()
    {
        let id = ObservationId::new(format!("obs-{index}"));
        let observation = InferenceToken::from_harness(
            metadata(&format!("observation-{index}")),
            DependencyCollector::default(),
        )
        .emit(Observation {
            id: id.clone(),
            source: SourceRef::new("fixture"),
            observed_at: None,
            relations: vec![],
            context: BTreeMap::new(),
            units: labels
                .into_iter()
                .enumerate()
                .map(|(i, label)| ObservedUnit {
                    id: ObservedUnitId::new(format!("unit-{i}")),
                    label: label.into(),
                    salience: None,
                    uncertainty: None,
                    context: BTreeMap::new(),
                })
                .collect(),
        });
        let alignment = InferenceToken::from_harness(
            metadata(&format!("alignment-{index}")),
            DependencyCollector::default(),
        )
        .emit(Alignment {
            observation: id,
            candidates: vec![],
        });
        provenance.push(StoredProvenance {
            id: observation.id().clone(),
            run_id: None,
            provenance: observation.provenance().clone(),
        });
        provenance.push(StoredProvenance {
            id: alignment.id().clone(),
            run_id: None,
            provenance: alignment.provenance().clone(),
        });
        observations.push(Tracked::from_derived(
            &observation,
            observation.value().clone(),
        ));
        alignments.push(Tracked::from_derived(&alignment, alignment.value().clone()));
    }
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            sensors: vec![PluginSelection::any("sensor.residual")],
            ..Default::default()
        })
        .unwrap();
    let frame = MeasurementFrame {
        id: FrameId::new("test.frame"),
        version: FrameVersion::new("1"),
        axes: vec![],
    };
    let measurements = engine
        .measure(
            &plan,
            MeasurementInputs {
                domain: &domain,
                frame: &frame,
                observations: &observations,
                alignments: &alignments,
                rankings: &[],
            },
            MeasurementRun {
                id: "residual",
                timestamp: Timestamp::new("now"),
                params: &BTreeMap::new(),
            },
        )
        .unwrap();
    Fixture {
        domain,
        observations,
        measurements,
        provenance,
    }
}
fn run(
    observations: &[Tracked<Observation>],
    measurements: &[Tracked<Measurement>],
    params: serde_json::Value,
) -> unclip_plugin::Result<Vec<Calculated<unclip_domain::CandidateProposal>>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            candidate_generators: vec![PluginSelection::any("generate.persistent-residual")],
            ..Default::default()
        })
        .unwrap();
    engine.generate_candidates(
        &plan,
        CandidateInputs {
            domain_version_id: r#"["test","1"]"#,
            observations,
            measurements,
        },
        MeasurementRun {
            id: "discover",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new("generate.persistent-residual"), params)]),
        },
    )
}
fn measurements(fixture: &Fixture) -> Vec<Tracked<Measurement>> {
    fixture
        .measurements
        .iter()
        .map(|m| Tracked::from_derived(m, m.value().clone()))
        .collect()
}

#[test]
fn exact_label_recurrence_counts_distinct_observations_and_is_reproducible() {
    let mut fixture = fixture();
    let mut measured = measurements(&fixture);
    let candidates = run(
        &fixture.observations,
        &measured,
        serde_json::json!({"minimum_observations":2}),
    )
    .unwrap();
    assert_eq!(candidates.len(), 1);
    let candidate = &candidates[0];
    assert_eq!(candidate.value().kind, CandidateKind::AtomicMeaning);
    assert_eq!(candidate.value().value["observation_count"], 2);
    assert_eq!(
        candidate.value().value["examples"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(candidate.value().value["pattern"]["observed_label"], "echo");
    assert!(!candidate.value().value.contains_key("label"));
    assert_eq!(candidate.provenance().inputs.len(), 5);
    assert_eq!(
        candidate.provenance().params_hash,
        hash_params(&serde_json::json!({"minimum_observations":2}))
    );
    fixture.observations.reverse();
    measured.reverse();
    assert_eq!(
        run(
            &fixture.observations,
            &measured,
            serde_json::json!({"minimum_observations":2})
        )
        .unwrap(),
        candidates
    );
    assert!(run(
        &fixture.observations,
        &measured,
        serde_json::json!({"minimum_observations":3})
    )
    .unwrap()
    .is_empty());
}
#[test]
fn duplicate_profiles_do_not_inflate_support_and_sparse_evidence_emits_nothing() {
    let fixture = fixture();
    let mut measured = measurements(&fixture);
    measured.push(Tracked::from_recorded(
        DerivedId::new("duplicate-profile"),
        fixture.measurements[0].value().clone(),
    ));
    let result = run(
        &fixture.observations,
        &measured,
        serde_json::json!({"minimum_observations":2}),
    )
    .unwrap();
    assert_eq!(result[0].value().value["observation_count"], 2);
    for reading in [
        Reading::NotMeasured,
        Reading::InsufficientEvidence { have: 1, need: 2 },
        Reading::NotApplicable {
            reason: "unsupported".into(),
        },
        Reading::Value {
            value: MeasurementValue::Structured(serde_json::json!({"count":0,"ids":[]})),
        },
    ] {
        let mut value = fixture.measurements[0].value().clone();
        value.reading = reading;
        assert!(run(
            &fixture.observations,
            &[Tracked::from_recorded(DerivedId::new("sparse"), value)],
            serde_json::json!({"minimum_observations":2})
        )
        .unwrap()
        .is_empty());
    }
}
#[test]
fn rejects_invalid_parameters_and_inconsistent_or_unselected_evidence() {
    let fixture = fixture();
    let measured = measurements(&fixture);
    for params in [
        serde_json::json!({}),
        serde_json::json!({"minimum_observations":1}),
        serde_json::json!({"minimum_observations":2,"label":"invented"}),
    ] {
        assert!(run(&fixture.observations, &measured, params).is_err());
    }
    for value in [
        serde_json::json!({"count":1,"ids":[]}),
        serde_json::json!({"count":1,"ids":["absent/unit"]}),
        serde_json::json!({"count":2,"ids":["obs-0/unit-0","obs-0/unit-0"]}),
    ] {
        let mut measurement = fixture.measurements[0].value().clone();
        measurement.reading = Reading::Value {
            value: MeasurementValue::Structured(value),
        };
        assert!(run(
            &fixture.observations,
            &[Tracked::from_recorded(DerivedId::new("bad"), measurement)],
            serde_json::json!({"minimum_observations":2})
        )
        .is_err());
    }
    let mut duplicate = fixture.observations.clone();
    duplicate.push(duplicate[0].clone());
    assert!(run(
        &duplicate,
        &measured,
        serde_json::json!({"minimum_observations":2})
    )
    .is_err());
    let mut duplicate = measured.clone();
    duplicate.push(duplicate[0].clone());
    assert!(run(
        &fixture.observations,
        &duplicate,
        serde_json::json!({"minimum_observations":2})
    )
    .is_err());
}
#[tokio::test]
async fn generated_proposal_and_its_evidence_persist_without_changing_domain() {
    let fixture = fixture();
    let db = unclip_store::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let domains = SeaOrmDomainRepository::new(db.clone());
    domains
        .insert_domain_version(fixture.domain.clone())
        .await
        .unwrap();
    let provenance = SeaOrmProvenanceRepository::new(db.clone());
    for value in fixture.provenance {
        provenance.insert_provenance(value).await.unwrap();
    }
    for measurement in &fixture.measurements {
        provenance
            .insert_provenance(StoredProvenance {
                id: measurement.id().clone(),
                run_id: None,
                provenance: measurement.provenance().clone(),
            })
            .await
            .unwrap();
    }
    let measured = fixture
        .measurements
        .iter()
        .map(|m| Tracked::from_derived(m, m.value().clone()))
        .collect::<Vec<_>>();
    let candidates = run(
        &fixture.observations,
        &measured,
        serde_json::json!({"minimum_observations":2}),
    )
    .unwrap();
    let repo = SeaOrmExperimentRepository::new(db);
    for candidate in &candidates {
        repo.insert_candidate(None, candidate.clone())
            .await
            .unwrap();
        assert_eq!(
            repo.get_candidate(candidate.id())
                .await
                .unwrap()
                .unwrap()
                .proposal,
            *candidate.value()
        );
        assert_eq!(
            provenance
                .get_provenance(candidate.id())
                .await
                .unwrap()
                .unwrap()
                .provenance,
            *candidate.provenance()
        );
        assert!(provenance
            .ancestors(candidate.id())
            .await
            .unwrap()
            .contains(&DerivedId::new("alignment-0")));
    }
    assert_eq!(
        domains
            .get_domain_version(&fixture.domain.id, &fixture.domain.version)
            .await
            .unwrap()
            .unwrap(),
        fixture.domain
    );
    assert_eq!(
        run(
            &fixture.observations,
            &measured,
            serde_json::json!({"minimum_observations":2})
        )
        .unwrap(),
        candidates
    );
}

#[test]
fn rejects_ambiguous_slash_qualified_identities() {
    let fixture = fixture();
    let reader = DependencyCollector::default();
    let mut first = reader.read(&fixture.observations[0]).clone();
    first.id = ObservationId::new("a/b");
    first.units.truncate(1);
    first.units[0].id = ObservedUnitId::new("c");
    let mut second = first.clone();
    second.id = ObservationId::new("a");
    second.units[0].id = ObservedUnitId::new("b/c");
    let observations = [
        Tracked::from_recorded(DerivedId::new("first"), first),
        Tracked::from_recorded(DerivedId::new("second"), second),
    ];
    assert!(run(
        &observations,
        &[],
        serde_json::json!({"minimum_observations":2})
    )
    .is_err());
    let registry = Engine::with_builtins().unwrap();
    let descriptor = registry
        .registry()
        .candidate_generators()
        .next()
        .unwrap()
        .descriptor();
    let _: serde_json::Value = serde_json::from_str(descriptor.params_schema).unwrap();
}
