use std::collections::BTreeMap;

use unclip_domain::{
    DomainId, DomainSnapshot, FrameAxis, FrameId, MeasurementFrame, Unit, UnitId, UnitKind,
};
use unclip_engine::{Engine, MeasurementInputs, MeasurementRun};
use unclip_epistemic::{
    hash_params, DependencyCollector, DerivedId, DomainVersion, EmitMetadata, FrameVersion,
    InferenceToken, PluginId, SourceRef, Timestamp, Tracked,
};
use unclip_measure::{
    MatrixCell, MeasurementProfile, MeasurementValue, RankPosition, RankTrajectory, Reading,
};
use unclip_observe::{
    Alignment, AlignmentCandidate, Observation, ObservationId, ObservedUnit, ObservedUnitId,
    PartialRanking, RankTier,
};
use unclip_plugin::{conformance, EngineProfile, MeasureCtx, PluginSelection, SensorDecision};
use unclip_store::*;

const SENSORS: &[&str] = &[
    "sensor.trajectories",
    "sensor.spearman",
    "sensor.kendall-association",
    "sensor.relative-rank-variance",
    "sensor.mutual-information",
];

#[derive(Clone)]
struct Fixture {
    domain: DomainSnapshot,
    frame: MeasurementFrame,
    observations: Vec<Observation>,
    alignments: Vec<Alignment>,
    rankings: Vec<PartialRanking>,
}

fn fixture() -> Fixture {
    let json: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/multi_observation.json")).unwrap();
    fixture_document(json)
}

fn fixture_document(json: serde_json::Value) -> Fixture {
    let units = json["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| UnitId::new(id.as_str().unwrap()))
        .collect::<Vec<_>>();
    let domain = DomainSnapshot {
        id: DomainId::new("batch"),
        version: DomainVersion::new("1"),
        relations: BTreeMap::new(),
        units: units
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    Unit {
                        id: id.clone(),
                        kind: UnitKind::AtomicMeaning,
                        label: None,
                        properties: BTreeMap::new(),
                    },
                )
            })
            .collect(),
    };
    let frame = MeasurementFrame {
        id: FrameId::new("batch.general"),
        version: FrameVersion::new("1"),
        axes: units
            .iter()
            .map(|unit| FrameAxis {
                unit: unit.clone(),
                label: None,
            })
            .collect(),
    };
    let mut fixture = Fixture {
        domain,
        frame,
        observations: vec![],
        alignments: vec![],
        rankings: vec![],
    };
    for row in json["observations"].as_array().unwrap() {
        let id = ObservationId::new(row["id"].as_str().unwrap());
        fixture.observations.push(Observation {
            id: id.clone(),
            source: SourceRef::new("fixture"),
            observed_at: None,
            relations: vec![],
            context: BTreeMap::new(),
            units: units
                .iter()
                .map(|unit| ObservedUnit {
                    id: ObservedUnitId::new(&unit.0),
                    label: unit.0.clone(),
                    salience: None,
                    uncertainty: None,
                    context: BTreeMap::new(),
                })
                .collect(),
        });
        let mut candidates = units
            .iter()
            .map(|unit| AlignmentCandidate {
                observed: ObservedUnitId::new(&unit.0),
                domain: unit.clone(),
                confidence: 0.8,
                evidence: vec![],
            })
            .collect::<Vec<_>>();
        if row["ambiguous_a"] == true {
            candidates.push(AlignmentCandidate {
                observed: ObservedUnitId::new("a"),
                domain: UnitId::new("c"),
                confidence: 0.8,
                evidence: vec![],
            });
        }
        fixture.alignments.push(Alignment {
            observation: id.clone(),
            candidates,
        });
        if let Some(tiers) = row["tiers"].as_array() {
            fixture.rankings.push(PartialRanking {
                observation: id,
                tiers: tiers
                    .iter()
                    .map(|tier| RankTier {
                        units: tier
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|unit| ObservedUnitId::new(unit.as_str().unwrap()))
                            .collect(),
                    })
                    .collect(),
                unknown: serde_json::from_value(row["unknown"].clone()).unwrap(),
            });
        }
    }
    fixture
}

fn metadata(id: &str, producer: &str) -> EmitMetadata {
    let params = serde_json::json!({});
    EmitMetadata {
        id: DerivedId::new(id),
        producer: PluginId::new(producer),
        algorithm: producer.into(),
        version: semver::Version::new(0, 1, 0),
        params_hash: hash_params(&params),
        params,
        source: Some(SourceRef::new("fixture")),
        timestamp: Timestamp::new("2026-09-19T00:00:00Z"),
        domain_version: Some(DomainVersion::new("1")),
        frame_version: Some(FrameVersion::new("1")),
        model: None,
    }
}

fn profile() -> EngineProfile {
    EngineProfile {
        sensors: SENSORS.iter().copied().map(PluginSelection::any).collect(),
        ..Default::default()
    }
}

struct Inputs {
    observations: Vec<Tracked<Observation>>,
    alignments: Vec<Tracked<Alignment>>,
    rankings: Vec<Tracked<PartialRanking>>,
}
impl Inputs {
    fn new(fixture: &Fixture) -> Self {
        Self {
            observations: fixture
                .observations
                .iter()
                .map(|value| {
                    Tracked::from_recorded(
                        DerivedId::new(format!("observation/{}", value.id.0)),
                        value.clone(),
                    )
                })
                .collect(),
            alignments: fixture
                .alignments
                .iter()
                .map(|value| {
                    Tracked::from_recorded(
                        DerivedId::new(format!("alignment/{}", value.observation.0)),
                        value.clone(),
                    )
                })
                .collect(),
            rankings: fixture
                .rankings
                .iter()
                .map(|value| {
                    Tracked::from_recorded(
                        DerivedId::new(format!("ranking/{}", value.observation.0)),
                        value.clone(),
                    )
                })
                .collect(),
        }
    }
    fn ctx<'a>(&'a self, fixture: &'a Fixture, params: &'a serde_json::Value) -> MeasureCtx<'a> {
        MeasureCtx::new(
            &fixture.domain,
            &fixture.frame,
            &self.observations,
            &self.alignments,
            &self.rankings,
            params,
            DependencyCollector::default(),
        )
    }
    fn engine_inputs<'a>(&'a self, fixture: &'a Fixture) -> MeasurementInputs<'a> {
        MeasurementInputs {
            domain: &fixture.domain,
            frame: &fixture.frame,
            observations: &self.observations,
            alignments: &self.alignments,
            rankings: &self.rankings,
        }
    }
}

#[test]
fn batch_sensors_conform_track_every_input_and_preserve_sparse_states() {
    let fixture = fixture();
    let inputs = Inputs::new(&fixture);
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&profile()).unwrap();
    let params = serde_json::json!({});
    let mut expected = inputs
        .observations
        .iter()
        .map(|value| value.id().clone())
        .chain(inputs.alignments.iter().map(|value| value.id().clone()))
        .chain(inputs.rankings.iter().map(|value| value.id().clone()))
        .collect::<Vec<_>>();
    expected.sort();
    for sensor in &plan.sensors {
        conformance::assert_sensor(sensor.as_ref(), |sensor| {
            let ctx = inputs.ctx(&fixture, &params);
            let values = sensor.measure(
                &ctx,
                ctx.calculation_token(metadata("test", &sensor.descriptor().id.0)),
            )?;
            assert_eq!(values[0].provenance().inputs, expected);
            assert_eq!(
                values[0].provenance().domain_version,
                Some(fixture.domain.version.clone())
            );
            assert_eq!(
                values[0].provenance().frame_version,
                Some(fixture.frame.version.clone())
            );
            Ok(values)
        });
        let ctx = inputs.ctx(&fixture, &params);
        conformance::assert_planning(
            sensor.as_ref(),
            &ctx,
            false,
            SensorDecision::Record(Reading::NotMeasured),
        );
        let mut empty = fixture.clone();
        empty.observations.clear();
        empty.alignments.clear();
        empty.rankings.clear();
        let empty_inputs = Inputs::new(&empty);
        let need = if sensor.descriptor().id.0 == "sensor.trajectories" {
            1
        } else {
            2
        };
        conformance::assert_planning(
            sensor.as_ref(),
            &empty_inputs.ctx(&empty, &params),
            true,
            SensorDecision::Record(Reading::InsufficientEvidence { have: 0, need }),
        );
        empty.frame.axes.clear();
        assert!(matches!(
            unclip_plugin::classify_sensor(
                sensor.as_ref(),
                &empty_inputs.ctx(&empty, &params),
                true
            ),
            SensorDecision::Record(Reading::NotApplicable { .. })
        ));
    }
    let params = BTreeMap::new();
    let results = engine
        .measure(
            &plan,
            inputs.engine_inputs(&fixture),
            MeasurementRun {
                id: "batch",
                timestamp: Timestamp::new("now"),
                params: &params,
            },
        )
        .unwrap();
    for result in &results {
        match &result.value().reading {
            Reading::Value {
                value: MeasurementValue::PairwiseMatrix(matrix),
            } => {
                let MatrixCell::Value {
                    value,
                    sample_count,
                } = matrix.cells()[0][1]
                else {
                    panic!("known pair expected")
                };
                assert_eq!(sample_count, 4);
                match result.value().sensor.0.as_str() {
                    "sensor.spearman" | "sensor.kendall-association" => assert!(value < 0.0),
                    "sensor.mutual-information" => assert!(value > 0.0),
                    "sensor.relative-rank-variance" => assert_eq!(value, 0.6875),
                    _ => unreachable!(),
                }
                assert_eq!(
                    matrix.cells()[0][2],
                    MatrixCell::InsufficientEvidence { have: 0, need: 2 }
                );
            }
            Reading::Value {
                value: MeasurementValue::Structured(value),
            } => {
                let trajectories: Vec<RankTrajectory> =
                    serde_json::from_value(value["rank_trajectories"].clone()).unwrap();
                assert_eq!(trajectories[0].samples[4].position, RankPosition::Missing);
                assert_eq!(trajectories[0].samples[5].position, RankPosition::Unknown);
                assert_eq!(
                    trajectories[0].samples[2].position,
                    RankPosition::Ranked { rank: 1 }
                );
                assert_eq!(
                    trajectories[1].samples[2].position,
                    RankPosition::Ranked { rank: 1 }
                );
            }
            _ => panic!("unexpected reading"),
        }
    }
    let mut reversed = fixture.clone();
    reversed.observations.reverse();
    reversed.rankings.reverse();
    reversed.alignments.reverse();
    assert_eq!(
        results,
        engine
            .measure(
                &plan,
                Inputs::new(&reversed).engine_inputs(&reversed),
                MeasurementRun {
                    id: "batch",
                    timestamp: Timestamp::new("now"),
                    params: &params
                }
            )
            .unwrap()
    );
}

#[test]
fn batch_sensors_reject_duplicate_or_unselected_inputs_and_unknown_parameters() {
    let base = fixture();
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&profile()).unwrap();
    for case in ["observation", "ranking", "unselected", "params"] {
        let mut fixture = base.clone();
        match case {
            "observation" => fixture.observations.push(fixture.observations[0].clone()),
            "ranking" => fixture.rankings.push(fixture.rankings[0].clone()),
            "unselected" => fixture.rankings[0].observation = ObservationId::new("not-selected"),
            _ => {}
        }
        let params = if case == "params" {
            serde_json::json!({"weight":1})
        } else {
            serde_json::json!({})
        };
        let inputs = Inputs::new(&fixture);
        for sensor in &plan.sensors {
            let ctx = inputs.ctx(&fixture, &params);
            assert!(
                sensor
                    .measure(
                        &ctx,
                        ctx.calculation_token(metadata("invalid", &sensor.descriptor().id.0))
                    )
                    .is_err(),
                "{case}"
            );
        }
    }
}

#[tokio::test]
async fn persisted_batch_profiles_replay_calculations_bit_for_bit() {
    assert_persisted_batch(fixture(), profile(), BTreeMap::new()).await;
}

async fn assert_persisted_batch(
    fixture: Fixture,
    profile: EngineProfile,
    params: BTreeMap<PluginId, serde_json::Value>,
) {
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let domains = SeaOrmDomainRepository::new(db.clone());
    let provenance = SeaOrmProvenanceRepository::new(db.clone());
    let observations = SeaOrmObservationRepository::new(db.clone());
    let measurements = SeaOrmMeasurementRepository::new(db.clone());
    let candidates = unclip_store::SeaOrmExperimentRepository::new(db.clone());
    let runs = SeaOrmEngineRunRepository::new(db);
    domains
        .insert_domain_version(fixture.domain.clone())
        .await
        .unwrap();
    domains
        .insert_measurement_frame(
            &fixture.domain.id,
            &fixture.domain.version,
            fixture.frame.clone(),
        )
        .await
        .unwrap();
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&profile).unwrap();
    runs.insert_run(engine.run_record(
        &plan,
        &params,
        "batch",
        Timestamp::new("now"),
        serde_json::json!({}),
    ))
    .await
    .unwrap();
    runs.transition_run("batch", EngineRunStatus::Running, None)
        .await
        .unwrap();
    let inputs = Inputs::new(&fixture);
    for id in inputs
        .observations
        .iter()
        .map(|value| value.id())
        .chain(inputs.alignments.iter().map(|value| value.id()))
        .chain(inputs.rankings.iter().map(|value| value.id()))
    {
        let inferred = InferenceToken::from_harness(
            metadata(&id.0, "infer.fixture"),
            DependencyCollector::default(),
        )
        .emit(());
        provenance
            .insert_provenance(StoredProvenance {
                id: id.clone(),
                run_id: Some("batch".into()),
                provenance: inferred.provenance().clone(),
            })
            .await
            .unwrap();
    }
    for value in &fixture.observations {
        observations
            .insert_observation(
                value.clone(),
                &DerivedId::new(format!("observation/{}", value.id.0)),
            )
            .await
            .unwrap();
    }
    for value in &fixture.alignments {
        observations
            .insert_alignment(
                &format!("alignment/{}", value.observation.0),
                value.clone(),
                &fixture.domain.id,
                &fixture.domain.version,
                &DerivedId::new(format!("alignment/{}", value.observation.0)),
            )
            .await
            .unwrap();
    }
    for value in &fixture.rankings {
        observations
            .insert_ranking(
                &format!("ranking/{}", value.observation.0),
                value.clone(),
                &DerivedId::new(format!("ranking/{}", value.observation.0)),
            )
            .await
            .unwrap();
    }
    let results = engine
        .measure(
            &plan,
            inputs.engine_inputs(&fixture),
            MeasurementRun {
                id: "batch",
                timestamp: Timestamp::new("now"),
                params: &params,
            },
        )
        .unwrap();
    let mut records = Vec::new();
    for result in &results {
        provenance
            .insert_provenance(StoredProvenance {
                id: result.id().clone(),
                run_id: Some("batch".into()),
                provenance: result.provenance().clone(),
            })
            .await
            .unwrap();
        measurements
            .insert_sensor_run(SensorRunRecord {
                id: result.id().0.clone(),
                engine_run_id: "batch".into(),
                sensor: result.value().sensor.clone(),
                sensor_version: result.value().sensor_version.clone(),
                params: params
                    .get(&result.value().sensor)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
                params_hash: hash_params(
                    &params
                        .get(&result.value().sensor)
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                ),
                status: "completed".into(),
                started_at: "now".into(),
                completed_at: Some("now".into()),
            })
            .await
            .unwrap();
        let Reading::Value { value } = &result.value().reading else {
            panic!("fixture should measure")
        };
        records.push(MeasurementRecord {
            id: format!("{}/measurement", result.id().0),
            sensor_run_id: result.id().0.clone(),
            provenance: result.id().clone(),
            kind: value.kind(),
            measurement: result.value().clone(),
        });
    }
    measurements
        .insert_profile(
            MeasurementProfileHeader {
                id: "batch-profile".into(),
                engine_run_id: "batch".into(),
                observation_id: None,
                frame: fixture.frame.id.clone(),
                frame_version: fixture.frame.version.clone(),
                provenance: results[0].id().clone(),
                created_at: "now".into(),
            },
            records,
        )
        .await
        .unwrap();
    runs.transition_run("batch", EngineRunStatus::Completed, Some("now".into()))
        .await
        .unwrap();
    let stored = measurements
        .get_profile("batch-profile")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored,
        MeasurementProfile {
            measurements: results
                .iter()
                .map(|result| result.value().clone())
                .collect()
        }
    );
    let matrix_inputs = results
        .iter()
        .filter(|result| {
            matches!(
                result.value().reading,
                Reading::Value {
                    value: MeasurementValue::PairwiseMatrix(_)
                }
            )
        })
        .map(|result| Tracked::from_derived(result, result.value().clone()))
        .collect::<Vec<_>>();
    if !matrix_inputs.is_empty() {
        let discovery_plan = engine
            .plan(&EngineProfile {
                candidate_generators: vec![unclip_plugin::PluginSelection::any(
                    "generate.pairwise-coupling",
                )],
                ..Default::default()
            })
            .unwrap();
        let domain_key =
            serde_json::to_string(&(&fixture.domain.id.0, &fixture.domain.version.0)).unwrap();
        let mut generated = 0;
        for (metric, threshold) in [
            ("spearman", -1.0),
            ("kendall", -1.0),
            ("relative_rank_variance", 10.0),
            ("mutual_information", 0.0),
        ] {
            let candidate_params = BTreeMap::from([(
                PluginId::new("generate.pairwise-coupling"),
                serde_json::json!({"metric":metric,"threshold":threshold,"minimum_samples":2}),
            )]);
            let discovery_id = format!("batch-candidates/{metric}");
            let calculate = || {
                engine
                    .generate_candidates(
                        &discovery_plan,
                        unclip_engine::CandidateInputs {
                            structures: &[],
                            domain_version_id: &domain_key,
                            observations: &[],
                            measurements: &matrix_inputs,
                        },
                        MeasurementRun {
                            id: &discovery_id,
                            timestamp: Timestamp::new("now"),
                            params: &candidate_params,
                        },
                    )
                    .unwrap()
            };
            let outputs = calculate();
            assert_eq!(outputs, calculate());
            generated += outputs.len();
            for output in outputs {
                unclip_store::CandidateRepository::insert_candidate(
                    &candidates,
                    Some("batch".into()),
                    output.clone(),
                )
                .await
                .unwrap();
                let stored =
                    unclip_store::CandidateRepository::get_candidate(&candidates, output.id())
                        .await
                        .unwrap()
                        .unwrap();
                assert_eq!(stored.proposal, *output.value());
                assert_eq!(
                    provenance
                        .get_provenance(output.id())
                        .await
                        .unwrap()
                        .unwrap()
                        .provenance,
                    *output.provenance()
                );
            }
        }
        assert!(generated > 0);

        for method in [
            unclip_engine::EmpiricalMethod::Communities {
                threshold: 0.5,
                minimum_samples: std::num::NonZeroUsize::new(2).unwrap(),
            },
            unclip_engine::EmpiricalMethod::Spectral {
                minimum_samples: std::num::NonZeroUsize::new(2).unwrap(),
                tolerance: 1e-12,
                max_sweeps: std::num::NonZeroUsize::new(100).unwrap(),
            },
        ] {
            let outputs = engine
                .derive_empirical(&matrix_inputs, method, "batch-g", Timestamp::new("now"))
                .unwrap();
            for output in outputs {
                if let Some(structure) = output.structure {
                    measurements
                        .insert_calculated_structure(
                            Some("batch".into()),
                            Some("batch-profile".into()),
                            structure.clone(),
                        )
                        .await
                        .unwrap();
                    let generator = if structure.value().kind == "communities" {
                        "generate.community"
                    } else {
                        "generate.latent-axis"
                    };
                    let metric = &structure.value().value["metric"];
                    let params = if generator == "generate.community" {
                        serde_json::json!({"metric":metric,"minimum_samples":2,"minimum_members":2})
                    } else {
                        serde_json::json!({"metric":metric,"minimum_samples":2,"minimum_absolute_eigenvalue":1e-10})
                    };
                    let plan = engine
                        .plan(&EngineProfile {
                            candidate_generators: vec![PluginSelection::any(generator)],
                            ..Default::default()
                        })
                        .unwrap();
                    let inputs = [Tracked::from_derived(&structure, structure.value().clone())];
                    let params = BTreeMap::from([(PluginId::new(generator), params)]);
                    let id = format!("structure-candidates/{}", structure.id().0);
                    let calculate = || {
                        engine
                            .generate_candidates(
                                &plan,
                                unclip_engine::CandidateInputs {
                                    domain_version_id: &domain_key,
                                    structures: &inputs,
                                    observations: &[],
                                    measurements: &[],
                                },
                                MeasurementRun {
                                    id: &id,
                                    timestamp: Timestamp::new("now"),
                                    params: &params,
                                },
                            )
                            .unwrap()
                    };
                    let proposals = calculate();
                    assert_eq!(proposals, calculate());
                    for proposal in proposals {
                        candidates
                            .insert_candidate(Some("batch".into()), proposal.clone())
                            .await
                            .unwrap();
                        assert_eq!(
                            provenance.direct_inputs(proposal.id()).await.unwrap(),
                            vec![structure.id().clone()]
                        );
                        assert_eq!(
                            provenance
                                .get_provenance(proposal.id())
                                .await
                                .unwrap()
                                .unwrap()
                                .provenance,
                            *proposal.provenance()
                        );
                    }
                    let stored = measurements
                        .get_empirical_structure(&structure.id().0)
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(
                        serde_json::to_vec(&stored.structure).unwrap(),
                        serde_json::to_vec(structure.value()).unwrap()
                    );
                    assert_eq!(
                        provenance.direct_inputs(structure.id()).await.unwrap(),
                        vec![output.measurement]
                    );
                    assert_eq!(
                        provenance
                            .get_provenance(structure.id())
                            .await
                            .unwrap()
                            .unwrap()
                            .provenance,
                        *structure.provenance()
                    );
                }
            }
        }
    }
    if results
        .iter()
        .any(|result| result.value().sensor.0 == "sensor.lagged-dependency")
    {
        let discovery_plan = engine
            .plan(&EngineProfile {
                candidate_generators: vec![unclip_plugin::PluginSelection::any(
                    "generate.temporal-coupling",
                )],
                ..Default::default()
            })
            .unwrap();
        let domain_key =
            serde_json::to_string(&(&fixture.domain.id.0, &fixture.domain.version.0)).unwrap();
        let candidate_params = BTreeMap::from([(
            PluginId::new("generate.temporal-coupling"),
            serde_json::json!({"threshold":0.5,"minimum_samples":2}),
        )]);
        let selected = results
            .iter()
            .map(|result| Tracked::from_derived(result, result.value().clone()))
            .collect::<Vec<_>>();
        let calculate = || {
            engine
                .generate_candidates(
                    &discovery_plan,
                    unclip_engine::CandidateInputs {
                        structures: &[],
                        domain_version_id: &domain_key,
                        observations: &[],
                        measurements: &selected,
                    },
                    MeasurementRun {
                        id: "temporal-candidates",
                        timestamp: Timestamp::new("now"),
                        params: &candidate_params,
                    },
                )
                .unwrap()
        };
        let outputs = calculate();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs, calculate());
        for output in outputs {
            unclip_store::CandidateRepository::insert_candidate(
                &candidates,
                Some("batch".into()),
                output.clone(),
            )
            .await
            .unwrap();
            let stored = unclip_store::CandidateRepository::get_candidate(&candidates, output.id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stored.proposal, *output.value());
            assert_eq!(
                provenance
                    .get_provenance(output.id())
                    .await
                    .unwrap()
                    .unwrap()
                    .provenance,
                *output.provenance()
            );
        }
    }
    let replay = runs.replay_run("batch").await.unwrap().unwrap();
    assert_eq!(replay.observations.len(), fixture.observations.len());
    let verified = engine
        .verify(
            &plan,
            &fixture.domain,
            &fixture.frame,
            &replay,
            MeasurementRun {
                id: "batch",
                timestamp: Timestamp::new("now"),
                params: &params,
            },
        )
        .unwrap();
    assert_eq!(verified, results);
    assert_eq!(
        serde_json::to_vec(&stored).unwrap(),
        serde_json::to_vec(&MeasurementProfile {
            measurements: verified
                .iter()
                .map(|result| result.value().clone())
                .collect()
        })
        .unwrap()
    );
    for result in &verified {
        let stored_provenance = provenance
            .get_provenance(result.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_provenance.provenance, *result.provenance());
        assert_eq!(
            provenance.direct_inputs(result.id()).await.unwrap().len(),
            fixture.observations.len() + fixture.alignments.len() + fixture.rankings.len()
        );
    }
}

const SELECTED_SENSORS: &[&str] = &[
    "sensor.co-foreground",
    "sensor.conditional-mutual-information",
    "sensor.partial-correlation",
];

fn conditioned_fixture() -> Fixture {
    fixture_document(
        serde_json::from_str(include_str!("fixtures/conditioned_observations.json")).unwrap(),
    )
}

fn selected_profile() -> EngineProfile {
    EngineProfile {
        sensors: SELECTED_SENSORS
            .iter()
            .copied()
            .map(PluginSelection::any)
            .collect(),
        ..Default::default()
    }
}

fn selected_params() -> BTreeMap<PluginId, serde_json::Value> {
    SELECTED_SENSORS
        .iter()
        .map(|id| {
            (
                PluginId::new(*id),
                if *id == "sensor.co-foreground" {
                    serde_json::json!({"left":"a","right":"b","foreground_rank":2})
                } else {
                    serde_json::json!({"left":"a","right":"b","conditioning_variables":["c"]})
                },
            )
        })
        .collect()
}

#[test]
fn selected_pair_sensors_conform_and_record_parameters_and_complete_cases() {
    let fixture = conditioned_fixture();
    let inputs = Inputs::new(&fixture);
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&selected_profile()).unwrap();
    let params = selected_params();
    for sensor in &plan.sensors {
        conformance::assert_sensor(sensor.as_ref(), |plugin| {
            let configured = &params[&plugin.descriptor().id];
            let ctx = inputs.ctx(&fixture, configured);
            let mut meta = metadata("selected", &plugin.descriptor().id.0);
            meta.params = configured.clone();
            meta.params_hash = hash_params(configured);
            let results = plugin.measure(&ctx, ctx.calculation_token(meta))?;
            assert_eq!(results[0].provenance().inputs.len(), 12);
            assert_eq!(results[0].provenance().params, *configured);
            assert_eq!(results[0].value().sample_count, Some(4));
            let Reading::Value {
                value: MeasurementValue::Scalar(value),
            } = results[0].value().reading
            else {
                panic!("expected a measured scalar")
            };
            let expected = match plugin.descriptor().id.0.as_str() {
                "sensor.co-foreground" => 0.25,
                "sensor.conditional-mutual-information" => 0.0,
                "sensor.partial-correlation" => -1.0,
                _ => unreachable!(),
            };
            assert!((value - expected).abs() < 1e-12);
            Ok(results)
        });
        let mut sparse = fixture.clone();
        sparse.rankings.remove(0);
        let sparse_inputs = Inputs::new(&sparse);
        let configured = &params[&sensor.descriptor().id];
        let ctx = sparse_inputs.ctx(&sparse, configured);
        let results = sensor
            .measure(
                &ctx,
                ctx.calculation_token(metadata("sparse", &sensor.descriptor().id.0)),
            )
            .unwrap();
        assert_eq!(results[0].value().sample_count, Some(3));
        if sensor.descriptor().id.0 == "sensor.partial-correlation" {
            assert_eq!(
                results[0].value().reading,
                Reading::InsufficientEvidence { have: 3, need: 4 }
            );
        }
        let empty_params = serde_json::json!({"left":"a","right":"b"});
        if sensor.descriptor().id.0 != "sensor.co-foreground" {
            let ctx = inputs.ctx(&fixture, &empty_params);
            conformance::assert_planning(
                sensor.as_ref(),
                &ctx,
                true,
                SensorDecision::Record(Reading::InsufficientEvidence { have: 0, need: 1 }),
            );
            let result = sensor
                .measure(
                    &ctx,
                    ctx.calculation_token(metadata("missing-condition", &sensor.descriptor().id.0)),
                )
                .unwrap();
            assert_eq!(result[0].value().sample_count, Some(0));
        }
    }
}

#[test]
fn selected_pair_sensors_reject_invalid_parameters_and_preserve_undefined_variance() {
    let fixture = conditioned_fixture();
    let inputs = Inputs::new(&fixture);
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&selected_profile()).unwrap();
    let params = selected_params();
    for sensor in &plan.sensors {
        let base = &params[&sensor.descriptor().id];
        for field in ["left", "same", "extra", "invalid-evidence"] {
            let mut config = base.clone();
            match field {
                "left" => config["left"] = serde_json::json!("absent"),
                "same" => config["right"] = config["left"].clone(),
                "extra" => config["unexpected"] = serde_json::json!(true),
                _ if sensor.descriptor().id.0 == "sensor.co-foreground" => {
                    config["foreground_rank"] = serde_json::json!(0)
                }
                _ => config["conditioning_variables"] = serde_json::json!(["c", "f1"]),
            }
            let ctx = inputs.ctx(&fixture, &config);
            assert!(sensor
                .measure(
                    &ctx,
                    ctx.calculation_token(metadata("invalid", &sensor.descriptor().id.0))
                )
                .is_err());
        }
    }
    let constant = fixture_document(serde_json::json!({"units":["a","b","c"],"observations":[
        {"id":"1","tiers":[["c"],["a"],["b"]],"unknown":[]},
        {"id":"2","tiers":[["c"],["b"],["a"]],"unknown":[]},
        {"id":"3","tiers":[["c"],["a"],["b"]],"unknown":[]},
        {"id":"4","tiers":[["c"],["b"],["a"]],"unknown":[]}
    ]}));
    let results = engine
        .measure(
            &plan,
            Inputs::new(&constant).engine_inputs(&constant),
            MeasurementRun {
                id: "constant",
                timestamp: Timestamp::new("now"),
                params: &params,
            },
        )
        .unwrap();
    let partial = results
        .iter()
        .find(|value| value.value().sensor.0 == "sensor.partial-correlation")
        .unwrap();
    assert_eq!(partial.value().sample_count, Some(4));
    assert_eq!(
        partial.value().reading,
        Reading::InsufficientEvidence { have: 0, need: 1 }
    );
    assert!(partial
        .value()
        .context
        .values
        .contains_key("missing_evidence"));
}

#[test]
fn conditional_information_detects_xor_and_zero_foreground_is_measured() {
    let fixture = fixture_document(serde_json::json!({"units":["a","b","c"],"observations":[
        {"id":"1","tiers":[["a","b","c"]],"unknown":[]},
        {"id":"2","tiers":[["a"],["b","c"]],"unknown":[]},
        {"id":"3","tiers":[["b"],["a","c"]],"unknown":[]},
        {"id":"4","tiers":[["c"],["a","b"]],"unknown":[]}
    ]}));
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&selected_profile()).unwrap();
    let params = selected_params();
    let results = engine
        .measure(
            &plan,
            Inputs::new(&fixture).engine_inputs(&fixture),
            MeasurementRun {
                id: "xor",
                timestamp: Timestamp::new("now"),
                params: &params,
            },
        )
        .unwrap();
    let cmi = results
        .iter()
        .find(|value| value.value().sensor.0 == "sensor.conditional-mutual-information")
        .unwrap();
    assert_eq!(
        cmi.value().reading,
        Reading::Value {
            value: MeasurementValue::Scalar(1.0)
        }
    );
    let mut params = selected_params();
    params
        .get_mut(&PluginId::new("sensor.co-foreground"))
        .unwrap()["foreground_rank"] = serde_json::json!(1);
    let fixture = conditioned_fixture();
    let results = engine
        .measure(
            &plan,
            Inputs::new(&fixture).engine_inputs(&fixture),
            MeasurementRun {
                id: "zero",
                timestamp: Timestamp::new("now"),
                params: &params,
            },
        )
        .unwrap();
    assert_eq!(
        results
            .iter()
            .find(|value| value.value().sensor.0 == "sensor.co-foreground")
            .unwrap()
            .value()
            .reading,
        Reading::Value {
            value: MeasurementValue::Scalar(0.0)
        }
    );
}

#[tokio::test]
async fn conditioned_profiles_persist_parameters_and_replay_bit_for_bit() {
    assert_persisted_batch(conditioned_fixture(), selected_profile(), selected_params()).await;
}

const TEMPORAL_SENSORS: &[&str] = &[
    "sensor.lagged-dependency",
    "sensor.dtw",
    "sensor.change-points",
];

fn temporal_fixture() -> Fixture {
    fixture_document(
        serde_json::from_str(include_str!("fixtures/temporal_observations.json")).unwrap(),
    )
}
fn temporal_profile() -> EngineProfile {
    EngineProfile {
        sensors: TEMPORAL_SENSORS
            .iter()
            .copied()
            .map(PluginSelection::any)
            .collect(),
        ..Default::default()
    }
}
fn temporal_params() -> BTreeMap<PluginId, serde_json::Value> {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/temporal_observations.json")).unwrap();
    TEMPORAL_SENSORS
        .iter()
        .map(|id| {
            let mut value = match *id {
                "sensor.lagged-dependency" => {
                    serde_json::json!({"source":"a","target":"b","lag":1})
                }
                "sensor.dtw" => serde_json::json!({"left":"a","right":"b"}),
                _ => serde_json::json!({"unit":"a","window":2,"minimum_shift":2.0}),
            };
            value["sequence"] = fixture["sequence"].clone();
            (PluginId::new(*id), value)
        })
        .collect()
}

#[test]
fn temporal_sensors_conform_use_explicit_order_and_keep_noncausal_evidence() {
    let fixture = temporal_fixture();
    let inputs = Inputs::new(&fixture);
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&temporal_profile()).unwrap();
    let params = temporal_params();
    for sensor in &plan.sensors {
        serde_json::from_str::<serde_json::Value>(sensor.descriptor().params_schema)
            .expect("valid parameter schema");
        conformance::assert_sensor(sensor.as_ref(), |plugin| {
            let configured = &params[&plugin.descriptor().id];
            let ctx = inputs.ctx(&fixture, configured);
            let mut meta = metadata("temporal", &plugin.descriptor().id.0);
            meta.params = configured.clone();
            meta.params_hash = hash_params(configured);
            let results = plugin.measure(&ctx, ctx.calculation_token(meta))?;
            assert_eq!(results[0].provenance().inputs.len(), 12);
            assert_eq!(results[0].provenance().params, *configured);
            let value = results[0].value();
            match plugin.descriptor().id.0.as_str() {
                "sensor.lagged-dependency" => {
                    assert_eq!(
                        value.reading,
                        Reading::Value {
                            value: MeasurementValue::Scalar(1.0)
                        }
                    );
                    assert_eq!(value.sample_count, Some(3));
                    assert_eq!(
                        value.context.values["evidence"],
                        "directional association, not causality"
                    );
                }
                "sensor.dtw" => {
                    assert_eq!(
                        value.reading,
                        Reading::Value {
                            value: MeasurementValue::Scalar(2.0)
                        }
                    );
                    assert_eq!(value.sample_count, Some(4));
                }
                _ => {
                    let Reading::Value {
                        value: MeasurementValue::Events(events),
                    } = &value.reading
                    else {
                        panic!("expected change events")
                    };
                    assert_eq!(events.len(), 1);
                    assert_eq!(events[0]["observation"], "t-3");
                    assert_eq!(events[0]["index"], 2);
                    assert_eq!(value.context.values["evaluated_boundaries"], 1);
                }
            }
            Ok(results)
        });
    }
    let calculate = |fixture: &Fixture| {
        engine
            .measure(
                &plan,
                Inputs::new(fixture).engine_inputs(fixture),
                MeasurementRun {
                    id: "stable",
                    timestamp: Timestamp::new("now"),
                    params: &params,
                },
            )
            .unwrap()
    };
    let expected = calculate(&fixture);
    let mut shuffled = fixture.clone();
    shuffled.observations.reverse();
    shuffled.rankings.reverse();
    shuffled.alignments.reverse();
    assert_eq!(calculate(&shuffled), expected);
}

#[test]
fn temporal_sensors_preserve_gaps_and_distinguish_no_event_from_missing_evidence() {
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&temporal_profile()).unwrap();
    let params = temporal_params();
    let mut fixture = temporal_fixture();
    fixture
        .rankings
        .retain(|ranking| ranking.observation.0 != "t-1");
    let results = engine
        .measure(
            &plan,
            Inputs::new(&fixture).engine_inputs(&fixture),
            MeasurementRun {
                id: "sparse-time",
                timestamp: Timestamp::new("now"),
                params: &params,
            },
        )
        .unwrap();
    for result in &results {
        let expected = match result.value().sensor.0.as_str() {
            "sensor.lagged-dependency" => Reading::InsufficientEvidence { have: 1, need: 2 },
            "sensor.dtw" => Reading::InsufficientEvidence { have: 3, need: 4 },
            _ => Reading::InsufficientEvidence { have: 2, need: 4 },
        };
        assert_eq!(result.value().reading, expected);
    }
    let fixture = temporal_fixture();
    let mut params = temporal_params();
    params
        .get_mut(&PluginId::new("sensor.change-points"))
        .unwrap()["minimum_shift"] = serde_json::json!(3.0);
    params.get_mut(&PluginId::new("sensor.dtw")).unwrap()["right"] = serde_json::json!("a");
    let results = engine
        .measure(
            &plan,
            Inputs::new(&fixture).engine_inputs(&fixture),
            MeasurementRun {
                id: "zero-time",
                timestamp: Timestamp::new("now"),
                params: &params,
            },
        )
        .unwrap();
    assert_eq!(
        results
            .iter()
            .find(|r| r.value().sensor.0 == "sensor.dtw")
            .unwrap()
            .value()
            .reading,
        Reading::Value {
            value: MeasurementValue::Scalar(0.0)
        }
    );
    assert_eq!(
        results
            .iter()
            .find(|r| r.value().sensor.0 == "sensor.change-points")
            .unwrap()
            .value()
            .reading,
        Reading::Value {
            value: MeasurementValue::Events(vec![])
        }
    );
    let mut constant = fixture.clone();
    for ranking in &mut constant.rankings {
        ranking.tiers = vec![RankTier {
            units: vec![ObservedUnitId::new("a"), ObservedUnitId::new("b")],
        }];
    }
    let results = engine
        .measure(
            &plan,
            Inputs::new(&constant).engine_inputs(&constant),
            MeasurementRun {
                id: "constant-time",
                timestamp: Timestamp::new("now"),
                params: &params,
            },
        )
        .unwrap();
    let lag = results
        .iter()
        .find(|r| r.value().sensor.0 == "sensor.lagged-dependency")
        .unwrap();
    assert_eq!(lag.value().sample_count, Some(3));
    assert_eq!(
        lag.value().reading,
        Reading::InsufficientEvidence { have: 0, need: 1 }
    );
    assert!(lag.value().context.values.contains_key("missing_evidence"));
}

#[test]
fn temporal_sensors_require_explicit_order_and_reject_invalid_selection_or_parameters() {
    let mut fixture = temporal_fixture();
    for observation in &mut fixture.observations {
        observation.observed_at = Some("2026-09-19T00:00:00Z".into());
    }
    let inputs = Inputs::new(&fixture);
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&temporal_profile()).unwrap();
    let params = temporal_params();
    for sensor in &plan.sensors {
        let base = &params[&sensor.descriptor().id];
        let mut missing = base.clone();
        missing.as_object_mut().unwrap().remove("sequence");
        let ctx = inputs.ctx(&fixture, &missing);
        conformance::assert_planning(
            sensor.as_ref(),
            &ctx,
            true,
            SensorDecision::Record(Reading::InsufficientEvidence { have: 0, need: 1 }),
        );
        assert_eq!(
            sensor
                .measure(
                    &ctx,
                    ctx.calculation_token(metadata("missing-time", &sensor.descriptor().id.0))
                )
                .unwrap()[0]
                .value()
                .reading,
            Reading::InsufficientEvidence { have: 0, need: 1 }
        );
        for case in [
            "duplicate",
            "positions",
            "unselected",
            "incomplete",
            "parameter",
            "unknown-unit",
            "unknown-field",
        ] {
            let mut config = base.clone();
            match case {
                "duplicate" => {
                    config["sequence"][1]["observation"] =
                        config["sequence"][0]["observation"].clone()
                }
                "positions" => config["sequence"][1]["position"] = serde_json::json!(0),
                "unselected" => config["sequence"][0]["observation"] = serde_json::json!("absent"),
                "incomplete" => {
                    config["sequence"].as_array_mut().unwrap().pop();
                }
                "parameter" => match sensor.descriptor().id.0.as_str() {
                    "sensor.lagged-dependency" => config["lag"] = serde_json::json!(0),
                    "sensor.dtw" => config["left"] = serde_json::json!(17),
                    _ => config["minimum_shift"] = serde_json::json!(-1.0),
                },
                "unknown-unit" => {
                    let key = match sensor.descriptor().id.0.as_str() {
                        "sensor.lagged-dependency" => "source",
                        "sensor.dtw" => "left",
                        _ => "unit",
                    };
                    config[key] = serde_json::json!("absent");
                }
                _ => config["extra"] = serde_json::json!(true),
            }
            let ctx = inputs.ctx(&fixture, &config);
            assert!(
                sensor
                    .measure(
                        &ctx,
                        ctx.calculation_token(metadata("invalid-time", &sensor.descriptor().id.0))
                    )
                    .is_err(),
                "{case}"
            );
        }
    }
}

#[tokio::test]
async fn temporal_profiles_persist_explicit_sequence_and_replay_bit_for_bit() {
    assert_persisted_batch(temporal_fixture(), temporal_profile(), temporal_params()).await;
}

#[test]
fn held_out_baseline_matches_explicit_subset_and_tracks_snapshot_dependencies() {
    use unclip_engine::HeldOutInputs;
    let fixture = fixture();
    let all = Inputs::new(&fixture);
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&profile()).unwrap();
    let training = vec![fixture.observations[0].id.clone()];
    let held_out = fixture.observations[1..]
        .iter()
        .map(|o| o.id.clone())
        .collect::<Vec<_>>();
    let split = engine
        .select_observations(
            &all.observations,
            &training,
            &held_out,
            "experiment",
            Timestamp::new("now"),
        )
        .unwrap();
    let split = Tracked::from_derived(&split, split.value().clone());
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), fixture.domain.clone());
    let frame = Tracked::from_recorded(DerivedId::new("frame"), fixture.frame.clone());
    let mut subset = fixture.clone();
    subset.observations.retain(|o| held_out.contains(&o.id));
    subset
        .alignments
        .retain(|a| held_out.contains(&a.observation));
    subset
        .rankings
        .retain(|r| held_out.contains(&r.observation));
    let inputs = Inputs::new(&subset);
    let params = BTreeMap::new();
    let run = || MeasurementRun {
        id: "baseline-run",
        timestamp: Timestamp::new("now"),
        params: &params,
    };
    let result = engine
        .measure_held_out_baseline(
            &plan,
            HeldOutInputs {
                baseline: &baseline,
                frame: &frame,
                split: &split,
                alignments: &inputs.alignments,
                rankings: &inputs.rankings,
            },
            run(),
        )
        .unwrap();
    let direct = engine
        .measure(&plan, inputs.engine_inputs(&subset), run())
        .unwrap();
    assert_eq!(result.len(), direct.len());
    for (actual, expected) in result.iter().zip(&direct) {
        assert_eq!(actual.value(), expected.value());
        for id in [baseline.id(), frame.id(), split.id()] {
            assert!(actual.provenance().inputs.contains(id));
        }
        assert!(!actual
            .provenance()
            .inputs
            .contains(all.observations[0].id()));
        assert_eq!(
            actual.provenance().domain_version,
            Some(fixture.domain.version.clone())
        );
        assert_eq!(
            actual.provenance().frame_version,
            Some(fixture.frame.version.clone())
        );
    }
    for (alignments, rankings) in [
        (&all.alignments, &inputs.rankings),
        (&inputs.alignments, &all.rankings),
    ] {
        assert!(engine
            .measure_held_out_baseline(
                &plan,
                HeldOutInputs {
                    baseline: &baseline,
                    frame: &frame,
                    split: &split,
                    alignments,
                    rankings
                },
                run()
            )
            .is_err());
    }
    let empty = engine.plan(&EngineProfile::default()).unwrap();
    assert!(engine
        .measure_held_out_baseline(
            &empty,
            HeldOutInputs {
                baseline: &baseline,
                frame: &frame,
                split: &split,
                alignments: &[],
                rankings: &[]
            },
            run()
        )
        .is_err());
    let mut invalid_frame = fixture.frame.clone();
    invalid_frame.axes.push(invalid_frame.axes[0].clone());
    let invalid_frame = Tracked::from_recorded(DerivedId::new("frame"), invalid_frame);
    assert!(engine
        .measure_held_out_baseline(
            &plan,
            HeldOutInputs {
                baseline: &baseline,
                frame: &invalid_frame,
                split: &split,
                alignments: &[],
                rankings: &[]
            },
            run()
        )
        .is_err());
}

#[test]
fn counterfactual_measurement_uses_one_split_and_retains_each_domain_provenance() {
    use unclip_domain::{CandidateKind, CandidateProposal};
    use unclip_engine::{CounterfactualMeasurementInputs, HeldOutInputs};
    let fixture = fixture();
    let inputs = Inputs::new(&fixture);
    let engine = Engine::with_builtins().unwrap();
    let plan = engine.plan(&profile()).unwrap();
    let baseline = Tracked::from_recorded(DerivedId::new("baseline"), fixture.domain.clone());
    let frame = Tracked::from_recorded(DerivedId::new("frame"), fixture.frame.clone());
    let selected = engine
        .select_observations(
            &inputs.observations,
            &[],
            &fixture
                .observations
                .iter()
                .map(|o| o.id.clone())
                .collect::<Vec<_>>(),
            "experiment",
            Timestamp::new("now"),
        )
        .unwrap();
    let split = Tracked::from_derived(&selected, selected.value().clone());
    let proposal = Tracked::from_recorded(DerivedId::new("candidate"), CandidateProposal {
        domain_version_id: serde_json::to_string(&(&fixture.domain.id.0, &fixture.domain.version.0)).unwrap(),
        kind: CandidateKind::AtomicMeaning,
        value: serde_json::json!({"pattern":{"matching":"exact_observed_label","observed_label":"new"}}).as_object().unwrap().clone(),
    });
    let candidate = engine
        .apply_candidate(&baseline, &proposal, "application", Timestamp::new("now"))
        .unwrap();
    let params = BTreeMap::new();
    let execute = |domain: &Tracked<DomainSnapshot>| {
        engine.measure_counterfactual(
            &plan,
            CounterfactualMeasurementInputs {
                baseline: HeldOutInputs {
                    baseline: domain,
                    frame: &frame,
                    split: &split,
                    alignments: &inputs.alignments,
                    rankings: &inputs.rankings,
                },
                counterfactual: &candidate,
                alignments: &inputs.alignments,
                rankings: &inputs.rankings,
            },
            MeasurementRun {
                id: "paired",
                timestamp: Timestamp::new("now"),
                params: &params,
            },
        )
    };
    let result = execute(&baseline).unwrap();
    assert!(!result.before.is_empty());
    assert_eq!(result.before.len(), result.after.len());
    for (before, after) in result.before.iter().zip(&result.after) {
        // An unaligned candidate outside this fixed frame must not invent improvement.
        assert_eq!(before.value(), after.value());
        assert_ne!(before.id(), after.id());
        assert!(before.provenance().inputs.contains(baseline.id()));
        assert!(after.provenance().inputs.contains(candidate.id()));
        assert!(!before.provenance().inputs.contains(candidate.id()));
        assert_eq!(
            before.provenance().domain_version,
            Some(fixture.domain.version.clone())
        );
        assert_eq!(
            after.provenance().domain_version,
            Some(candidate.value().domain.version.clone())
        );
        for id in [frame.id(), split.id()] {
            assert!(before.provenance().inputs.contains(id));
            assert!(after.provenance().inputs.contains(id));
        }
        assert_eq!(before.provenance().params, after.provenance().params);
    }
    let replay = execute(&baseline).unwrap();
    assert_eq!(result.before, replay.before);
    assert_eq!(result.after, replay.after);
    let compare = |pairs: &[unclip_engine::ComparisonPair], comparators: bool| {
        let mut profile = profile();
        if comparators {
            profile.comparators = vec![
                PluginSelection::any("compare.pairwise-matrix"),
                PluginSelection::any("compare.scalar-difference"),
            ];
        }
        let plan = engine.plan(&profile).unwrap();
        let params = BTreeMap::from([(
            PluginId::new("compare.pairwise-matrix"),
            serde_json::json!({"minimum_samples":2}),
        )]);
        engine.compare_counterfactual(
            &plan,
            CounterfactualMeasurementInputs {
                baseline: HeldOutInputs {
                    baseline: &baseline,
                    frame: &frame,
                    split: &split,
                    alignments: &inputs.alignments,
                    rankings: &inputs.rankings,
                },
                counterfactual: &candidate,
                alignments: &inputs.alignments,
                rankings: &inputs.rankings,
            },
            pairs,
            MeasurementRun {
                id: "paired",
                timestamp: Timestamp::new("now"),
                params: &params,
            },
        )
    };
    let before = result
        .before
        .iter()
        .find(|v| v.value().sensor.0 == "sensor.spearman")
        .unwrap();
    let after = result
        .after
        .iter()
        .find(|v| v.value().sensor.0 == "sensor.spearman")
        .unwrap();
    let pair = unclip_engine::ComparisonPair {
        before: before.id().clone(),
        after: after.id().clone(),
    };
    let compared = compare(std::slice::from_ref(&pair), true).unwrap();
    assert_eq!(compared.measurements.before, result.before);
    assert_eq!(compared.measurements.after, result.after);
    assert_eq!(compared.comparison.deltas.len(), 2);
    let profile = compared.comparison.profile.value();
    assert_eq!(profile.pairs, vec![pair.clone()]);
    assert_eq!(profile.unmatched_before.len(), result.before.len() - 1);
    assert_eq!(profile.unmatched_after.len(), result.after.len() - 1);
    for delta in &compared.comparison.deltas {
        let mut expected = vec![pair.before.clone(), pair.after.clone()];
        expected.sort();
        assert_eq!(delta.provenance().inputs, expected);
        assert!(compared
            .comparison
            .profile
            .provenance()
            .inputs
            .contains(delta.id()));
    }
    let replay = compare(std::slice::from_ref(&pair), true).unwrap();
    assert_eq!(replay.comparison.profile, compared.comparison.profile);
    assert_eq!(replay.comparison.deltas, compared.comparison.deltas);
    assert!(compare(&[], true).is_err());
    assert!(compare(std::slice::from_ref(&pair), false).is_err());
    assert!(compare(&[pair.clone(), pair], true).is_err());
    assert!(compare(
        &[unclip_engine::ComparisonPair {
            before: DerivedId::new("missing"),
            after: after.id().clone()
        }],
        true
    )
    .is_err());

    let wrong_identity =
        Tracked::from_recorded(DerivedId::new("other-baseline"), fixture.domain.clone());
    assert!(execute(&wrong_identity).is_err());
    let mut wrong_version = fixture.domain.clone();
    wrong_version.version = DomainVersion::new("2");
    assert!(execute(&Tracked::from_recorded(
        DerivedId::new("baseline"),
        wrong_version
    ))
    .is_err());
    assert_eq!(
        DependencyCollector::default().read(&baseline),
        &fixture.domain
    );
}
