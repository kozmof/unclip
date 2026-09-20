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
fn run_generator(
    generator: &str,
    observations: &[Tracked<Observation>],
    measurements: &[Tracked<Measurement>],
    params: serde_json::Value,
) -> unclip_plugin::Result<Vec<Calculated<unclip_domain::CandidateProposal>>> {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            candidate_generators: vec![PluginSelection::any(generator)],
            ..Default::default()
        })
        .unwrap();
    engine.generate_candidates(
        &plan,
        CandidateInputs {
            structures: &[],
            domain_version_id: r#"["test","1"]"#,
            observations,
            measurements,
        },
        MeasurementRun {
            id: "discover",
            timestamp: Timestamp::new("now"),
            params: &BTreeMap::from([(PluginId::new(generator), params)]),
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
async fn assert_stored(fixture: Fixture, generator: &str) {
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
    let candidates = run_generator(
        generator,
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
        run_generator(
            generator,
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

fn run(
    observations: &[Tracked<Observation>],
    measurements: &[Tracked<Measurement>],
    params: serde_json::Value,
) -> unclip_plugin::Result<Vec<Calculated<unclip_domain::CandidateProposal>>> {
    run_generator(
        "generate.persistent-residual",
        observations,
        measurements,
        params,
    )
}

fn relation_fixture() -> Fixture {
    use unclip_observe::{ObservedRelation, ObservedRelationId};
    let mut fixture = fixture();
    let reader = DependencyCollector::default();
    let mut alignments = Vec::new();
    for (index, tracked) in fixture.observations.iter_mut().enumerate() {
        let mut observation = reader.read(tracked).clone();
        observation.units = ["source", "target"]
            .into_iter()
            .enumerate()
            .map(|(i, label)| ObservedUnit {
                id: ObservedUnitId::new(format!("unit-{i}")),
                label: label.into(),
                salience: None,
                uncertainty: None,
                context: BTreeMap::new(),
            })
            .collect();
        let (source, target) = if index == 2 { (1, 0) } else { (0, 1) };
        observation.relations = vec![ObservedRelation {
            id: ObservedRelationId::new("r"),
            source: ObservedUnitId::new(format!("unit-{source}")),
            target: ObservedUnitId::new(format!("unit-{target}")),
            kind: "supports".into(),
            uncertainty: Some(0.25),
        }];
        if index == 0 {
            let mut duplicate = observation.relations[0].clone();
            duplicate.id = ObservedRelationId::new("r-copy");
            observation.relations.push(duplicate);
        }
        if index == 2 {
            let mut other = observation.relations[0].clone();
            other.id = ObservedRelationId::new("other-kind");
            other.kind = "opposes".into();
            observation.relations.push(other);
        }
        alignments.push(Tracked::from_recorded(
            DerivedId::new(format!("alignment-{index}")),
            Alignment {
                observation: observation.id.clone(),
                candidates: vec![],
            },
        ));
        *tracked = Tracked::from_recorded(tracked.id().clone(), observation);
    }
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            sensors: vec![PluginSelection::any("sensor.residual")],
            ..Default::default()
        })
        .unwrap();
    let frame = MeasurementFrame {
        id: FrameId::new("frame"),
        version: FrameVersion::new("1"),
        axes: vec![],
    };
    fixture.measurements = engine
        .measure(
            &plan,
            MeasurementInputs {
                domain: &fixture.domain,
                frame: &frame,
                observations: &fixture.observations,
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
    fixture
}

#[tokio::test]
async fn generated_proposals_and_evidence_persist_without_changing_domain() {
    assert_stored(fixture(), "generate.persistent-residual").await;
    assert_stored(relation_fixture(), "generate.missing-relation").await;
    assert_stored(motif_fixture(), "generate.recurring-motif").await;
}

#[test]
fn missing_relation_candidates_preserve_direction_kind_and_distinct_observation_support() {
    let mut fixture = relation_fixture();
    let mut measured = measurements(&fixture);
    measured.push(Tracked::from_recorded(
        DerivedId::new("another-profile"),
        fixture.measurements[1].value().clone(),
    ));
    let params = serde_json::json!({"minimum_observations":2});
    let candidates = run_generator(
        "generate.missing-relation",
        &fixture.observations,
        &measured,
        params.clone(),
    )
    .unwrap();
    assert_eq!(candidates.len(), 1);
    let candidate = &candidates[0];
    assert_eq!(candidate.value().kind, CandidateKind::Relation);
    assert_eq!(
        candidate.value().value["pattern"],
        serde_json::json!({"matching":"exact_directed_observed_relation","source_label":"source","relation_kind":"supports","target_label":"target"})
    );
    assert_eq!(candidate.value().value["observation_count"], 2);
    assert_eq!(
        candidate.value().value["examples"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(candidate.value().value["examples"][0]["uncertainty"], 0.25);
    assert!(!candidate.value().value.contains_key("label"));
    fixture.observations.reverse();
    measured.reverse();
    assert_eq!(
        run_generator(
            "generate.missing-relation",
            &fixture.observations,
            &measured,
            params
        )
        .unwrap(),
        candidates
    );
    assert!(run_generator(
        "generate.missing-relation",
        &fixture.observations,
        &measured,
        serde_json::json!({"minimum_observations":3})
    )
    .unwrap()
    .is_empty());
}

#[test]
fn missing_relation_generation_rejects_invalid_graphs_and_unselected_residuals() {
    let fixture = relation_fixture();
    let measured = measurements(&fixture);
    let params = serde_json::json!({"minimum_observations":2});
    let reader = DependencyCollector::default();
    for change in 0..4 {
        let mut observations = fixture.observations.clone();
        let mut value = reader.read(&observations[0]).clone();
        match change {
            0 => value.relations[0].source = ObservedUnitId::new("absent"),
            1 => value.units.push(value.units[0].clone()),
            2 => value.relations.push(value.relations[0].clone()),
            _ => value.relations[0].uncertainty = Some(f64::NAN),
        }
        observations[0] = Tracked::from_recorded(observations[0].id().clone(), value);
        assert!(run_generator(
            "generate.missing-relation",
            &observations,
            &measured,
            params.clone()
        )
        .is_err());
    }
    let mut measurement = fixture.measurements[1].value().clone();
    measurement.reading = Reading::Value {
        value: MeasurementValue::Structured(
            serde_json::json!({"count":1,"ids":["absent/relation"]}),
        ),
    };
    assert!(run_generator(
        "generate.missing-relation",
        &fixture.observations,
        &[Tracked::from_recorded(DerivedId::new("bad"), measurement)],
        params.clone()
    )
    .is_err());
    for reading in [
        Reading::NotMeasured,
        Reading::InsufficientEvidence { have: 1, need: 2 },
        Reading::Value {
            value: MeasurementValue::Structured(serde_json::json!({"count":0,"ids":[]})),
        },
    ] {
        let mut measurement = fixture.measurements[1].value().clone();
        measurement.reading = reading;
        assert!(run_generator(
            "generate.missing-relation",
            &fixture.observations,
            &[Tracked::from_recorded(
                DerivedId::new("sparse"),
                measurement
            )],
            params.clone()
        )
        .unwrap()
        .is_empty());
    }
}

fn remeasure_relations(fixture: &mut Fixture) {
    let reader = DependencyCollector::default();
    let alignments = fixture
        .observations
        .iter()
        .enumerate()
        .map(|(index, tracked)| {
            Tracked::from_recorded(
                DerivedId::new(format!("alignment-{index}")),
                Alignment {
                    observation: reader.read(tracked).id.clone(),
                    candidates: vec![],
                },
            )
        })
        .collect::<Vec<_>>();
    let engine = Engine::with_builtins().unwrap();
    let plan = engine
        .plan(&EngineProfile {
            sensors: vec![PluginSelection::any("sensor.residual")],
            ..Default::default()
        })
        .unwrap();
    let frame = MeasurementFrame {
        id: FrameId::new("frame"),
        version: FrameVersion::new("1"),
        axes: vec![],
    };
    fixture.measurements = engine
        .measure(
            &plan,
            MeasurementInputs {
                domain: &fixture.domain,
                frame: &frame,
                observations: &fixture.observations,
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
}
fn motif_fixture() -> Fixture {
    use unclip_observe::{ObservedRelation, ObservedRelationId};
    let mut fixture = relation_fixture();
    let reader = DependencyCollector::default();
    for (index, tracked) in fixture.observations.iter_mut().enumerate() {
        let mut observation = reader.read(tracked).clone();
        observation.units = ["source", "middle", "target"]
            .into_iter()
            .enumerate()
            .map(|(i, label)| ObservedUnit {
                id: ObservedUnitId::new(format!("unit-{i}")),
                label: label.into(),
                salience: None,
                uncertainty: None,
                context: BTreeMap::new(),
            })
            .collect();
        let (a, b, c) = if index == 2 { (2, 1, 0) } else { (0, 1, 2) };
        observation.relations = [("first", a, b, "supports"), ("second", b, c, "enables")]
            .into_iter()
            .map(|(id, source, target, kind)| ObservedRelation {
                id: ObservedRelationId::new(id),
                source: ObservedUnitId::new(format!("unit-{source}")),
                target: ObservedUnitId::new(format!("unit-{target}")),
                kind: kind.into(),
                uncertainty: Some(0.2),
            })
            .collect();
        if index == 0 {
            let mut duplicate = observation.relations[0].clone();
            duplicate.id = ObservedRelationId::new("first-copy");
            observation.relations.push(duplicate);
        }
        *tracked = Tracked::from_recorded(tracked.id().clone(), observation);
    }
    remeasure_relations(&mut fixture);
    fixture
}

#[test]
fn motifs_preserve_graph_structure_and_count_observations_instead_of_occurrences() {
    let mut fixture = motif_fixture();
    let mut measured = measurements(&fixture);
    measured.push(Tracked::from_recorded(
        DerivedId::new("repeated-profile"),
        fixture.measurements[1].value().clone(),
    ));
    let params = serde_json::json!({"minimum_observations":2});
    let candidates = run_generator(
        "generate.recurring-motif",
        &fixture.observations,
        &measured,
        params.clone(),
    )
    .unwrap();
    assert_eq!(candidates.len(), 1);
    let proposal = candidates[0].value();
    assert_eq!(proposal.kind, CandidateKind::GraphMotif);
    assert_eq!(proposal.value["observation_count"], 2);
    assert_eq!(proposal.value["examples"].as_array().unwrap().len(), 3);
    assert_eq!(
        proposal.value["pattern"]["nodes"][1]["observed_label"],
        "middle"
    );
    assert_eq!(
        proposal.value["pattern"]["edges"],
        serde_json::json!([{"source":0,"target":1,"kind":"supports"},{"source":1,"target":2,"kind":"enables"}])
    );
    assert_eq!(
        proposal.value["examples"][0]["edges"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        proposal.value["examples"][0]["edges"][0]["uncertainty"],
        0.2
    );
    assert!(!proposal.value.contains_key("label"));
    fixture.observations.reverse();
    measured.reverse();
    let reader = DependencyCollector::default();
    for tracked in &mut fixture.observations {
        let mut observation = reader.read(tracked).clone();
        observation.units.reverse();
        observation.relations.reverse();
        *tracked = Tracked::from_recorded(tracked.id().clone(), observation);
    }
    assert_eq!(
        run_generator(
            "generate.recurring-motif",
            &fixture.observations,
            &measured,
            params
        )
        .unwrap(),
        candidates
    );
    assert!(run_generator(
        "generate.recurring-motif",
        &fixture.observations,
        &measured,
        serde_json::json!({"minimum_observations":3})
    )
    .unwrap()
    .is_empty());
}

#[test]
fn motifs_require_connected_distinct_units_and_two_residual_edges() {
    let params = serde_json::json!({"minimum_observations":2});
    let reader = DependencyCollector::default();
    for change in 0..3 {
        let mut fixture = motif_fixture();
        for tracked in &mut fixture.observations {
            let mut observation = reader.read(tracked).clone();
            match change {
                0 => {
                    // Equal labels on different units are not connectivity.
                    let mut separate = observation.units[1].clone();
                    separate.id = ObservedUnitId::new("separate-middle");
                    observation.units.push(separate);
                    observation.relations[1].source = ObservedUnitId::new("separate-middle");
                }
                1 => observation.relations[1].target = observation.relations[0].source.clone(),
                _ => {
                    // Keep only one edge per observation in residual evidence below.
                    observation
                        .relations
                        .retain(|relation| relation.id.0 == "first");
                }
            }
            *tracked = Tracked::from_recorded(tracked.id().clone(), observation);
        }
        remeasure_relations(&mut fixture);
        assert!(run_generator(
            "generate.recurring-motif",
            &fixture.observations,
            &measurements(&fixture),
            params.clone()
        )
        .unwrap()
        .is_empty());
    }
    let fixture = motif_fixture();
    let mut residual = fixture.measurements[1].value().clone();
    residual.reading = Reading::InsufficientEvidence { have: 1, need: 2 };
    assert!(run_generator(
        "generate.recurring-motif",
        &fixture.observations,
        &[Tracked::from_recorded(DerivedId::new("sparse"), residual)],
        params
    )
    .unwrap()
    .is_empty());
}

#[test]
fn motif_matching_keeps_relation_kinds_separate_and_rejects_dangling_edges() {
    let mut fixture = motif_fixture();
    let reader = DependencyCollector::default();
    let mut observation = reader.read(&fixture.observations[1]).clone();
    observation.relations[1].kind = "different".into();
    fixture.observations[1] =
        Tracked::from_recorded(fixture.observations[1].id().clone(), observation.clone());
    remeasure_relations(&mut fixture);
    let params = serde_json::json!({"minimum_observations":2});
    assert!(run_generator(
        "generate.recurring-motif",
        &fixture.observations,
        &measurements(&fixture),
        params.clone()
    )
    .unwrap()
    .is_empty());
    observation.relations[1].target = ObservedUnitId::new("absent");
    fixture.observations[1] =
        Tracked::from_recorded(fixture.observations[1].id().clone(), observation);
    assert!(run_generator(
        "generate.recurring-motif",
        &fixture.observations,
        &measurements(&fixture),
        params
    )
    .is_err());
}
