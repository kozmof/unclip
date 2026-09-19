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
    let fixture = fixture();
    let db = connect_and_migrate("sqlite::memory:").await.unwrap();
    let domains = SeaOrmDomainRepository::new(db.clone());
    let provenance = SeaOrmProvenanceRepository::new(db.clone());
    let observations = SeaOrmObservationRepository::new(db.clone());
    let measurements = SeaOrmMeasurementRepository::new(db.clone());
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
    let plan = engine.plan(&profile()).unwrap();
    let params = BTreeMap::new();
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
                params: serde_json::json!({}),
                params_hash: hash_params(&serde_json::json!({})),
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
    let replay = runs.replay_run("batch").await.unwrap().unwrap();
    assert_eq!(replay.observations.len(), 6);
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
            17
        );
    }
}
