//! Execute and atomically persist one explicit held-out counterfactual experiment.

use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use unclip_engine::{
    ComparisonPair, CounterfactualMeasurementInputs, ExperimentConstraints, HeldOutInputs,
    RelationBindings,
};
use unclip_epistemic::{
    hash_params, DerivedId, Operation, PluginId, PluginParams, Provenance, Timestamp, Tracked,
};
use unclip_measure::{Measurement, MeasurementKind, Reading};
use unclip_store::{
    CandidateRepository, CompletedExperimentBundle, DomainReader, EngineRunRepository,
    EngineRunStatus, ExperimentMeasurementProfile, ExperimentRepository, MeasurementProfileHeader,
    MeasurementRecord, MeasurementRepository, ObservationRepository, ProvenanceRepository,
    SensorRunRecord, StoredProvenance,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    run_id: String,
    candidate: DerivedId,
    training: Vec<unclip_observe::ObservationId>,
    held_out: Vec<unclip_observe::ObservationId>,
    comparison_sensor: PluginId,
    #[serde(default)]
    relation_bindings: Option<RelationBindings>,
    #[serde(default)]
    constraints: Vec<unclip_engine::ExperimentConstraint>,
    #[serde(default)]
    pareto_dimensions: Vec<unclip_engine::ParetoDimension>,
    #[serde(default)]
    transfer_measurement_profiles: Vec<String>,
}

fn root_provenance(
    run_id: &str,
    id: DerivedId,
    producer: &str,
    timestamp: &str,
    domain_version: Option<unclip_epistemic::DomainVersion>,
    frame_version: Option<unclip_epistemic::FrameVersion>,
) -> StoredProvenance {
    let params = serde_json::json!({});
    StoredProvenance {
        id,
        run_id: Some(run_id.into()),
        provenance: std::sync::Arc::new(Provenance {
            operation: Operation::Calculated,
            producer: PluginId::new(producer),
            algorithm: "stored_version_snapshot".into(),
            version: "0.1.0".parse().expect("valid snapshot producer version"),
            params_hash: hash_params(&params),
            params: Arc::new(params),
            inputs: vec![],
            source: None,
            timestamp: Timestamp::new(timestamp),
            domain_version,
            frame_version,
            model: None,
        }),
    }
}

fn measurement_kind(
    plan: &unclip_plugin::RunPlan,
    measurement: &Measurement,
) -> anyhow::Result<MeasurementKind> {
    match &measurement.reading {
        Reading::Value { value } => Ok(value.kind()),
        _ => plan
            .sensors
            .iter()
            .map(|sensor| sensor.descriptor())
            .find(|descriptor| descriptor.id == measurement.sensor)
            .and_then(|descriptor| descriptor.produces.first().copied())
            .with_context(|| {
                format!(
                    "sensor declares no measurement kind: {}",
                    measurement.sensor
                )
            }),
    }
}

fn stored_sensor_runs(
    run_id: &str,
    before: &[unclip_epistemic::Calculated<Measurement>],
    after: &[unclip_epistemic::Calculated<Measurement>],
    params: &PluginParams,
    timestamp: &str,
) -> anyhow::Result<(BTreeMap<PluginId, String>, Vec<SensorRunRecord>)> {
    let mut identities = BTreeMap::new();
    let mut sensor_runs = Vec::new();
    for value in before.iter().chain(after) {
        let measurement = value.value();
        if let Some(id) = identities.get(&measurement.sensor) {
            let run = sensor_runs
                .iter()
                .find(|run: &&SensorRunRecord| &run.id == id)
                .expect("sensor-run identity is indexed");
            ensure!(
                run.sensor_version == measurement.sensor_version,
                "one experiment sensor resolved to multiple versions: {}",
                measurement.sensor
            );
            continue;
        }
        let id = format!("{run_id}/sensor-runs/{}", measurement.sensor);
        let sensor_params = params
            .get(&measurement.sensor)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}).into());
        identities.insert(measurement.sensor.clone(), id.clone());
        sensor_runs.push(SensorRunRecord {
            id,
            engine_run_id: run_id.into(),
            sensor: measurement.sensor.clone(),
            sensor_version: measurement.sensor_version.clone(),
            params_hash: hash_params(&sensor_params),
            params: sensor_params,
            status: "completed".into(),
            started_at: timestamp.into(),
            completed_at: Some(timestamp.into()),
        });
    }
    Ok((identities, sensor_runs))
}

/// What the before and after profiles of one experiment have in common.
///
/// Built once and shared, so the two calls differ only in the four things that
/// actually differ. `stored_profile` previously took all nine positionally, with
/// `run_id` and `id` adjacent and both `&str` — a transposition that compiled
/// and filed the profile under the wrong identity.
struct ProfileContext<'a> {
    run_id: &'a str,
    frame: &'a unclip_domain::MeasurementFrame,
    sensor_run_ids: &'a BTreeMap<PluginId, String>,
    plan: &'a unclip_plugin::RunPlan,
    timestamp: &'a str,
}

fn stored_profile(
    ctx: &ProfileContext<'_>,
    id: &str,
    values: &[unclip_epistemic::Calculated<Measurement>],
    profile_provenance: DerivedId,
    sensor_runs: Vec<SensorRunRecord>,
) -> anyhow::Result<ExperimentMeasurementProfile> {
    let ProfileContext {
        run_id,
        frame,
        sensor_run_ids,
        plan,
        timestamp,
    } = *ctx;
    let measurements = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let measurement = value.value();
            Ok(MeasurementRecord {
                id: format!("{id}/measurements/{index}"),
                sensor_run_id: sensor_run_ids
                    .get(&measurement.sensor)
                    .with_context(|| {
                        format!("sensor run was not prepared: {}", measurement.sensor)
                    })?
                    .clone(),
                provenance: value.id().clone(),
                kind: measurement_kind(plan, measurement)?,
                measurement: value.shared(),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(ExperimentMeasurementProfile {
        header: MeasurementProfileHeader {
            id: id.into(),
            engine_run_id: run_id.into(),
            observation_id: None,
            frame: frame.id.clone(),
            frame_version: frame.version.clone(),
            provenance: profile_provenance,
            created_at: timestamp.into(),
        },
        sensor_runs,
        measurements,
    })
}

pub(crate) async fn run(
    repos: &crate::db::Repos,
    profile_path: &std::path::Path,
    request_path: &std::path::Path,
) -> anyhow::Result<()> {
    let request: Request = serde_norway::from_str(&std::fs::read_to_string(request_path)?)
        .context("invalid experiment request")?;
    ensure!(
        !request.run_id.trim().is_empty(),
        "experiment run ID is empty"
    );
    ensure!(
        !request.held_out.is_empty(),
        "select at least one held-out observation"
    );
    let document = unclip_io::load_engine_profile(profile_path)?;
    let domain_selector = document
        .domain
        .as_deref()
        .context("experiment profile must select domain@version")?;
    let frame_selector = document
        .frame
        .as_deref()
        .context("experiment profile must select frame@version")?;
    let (domain_id, domain_version) = super::parse_domain_selector(domain_selector)?;
    let (frame_id, frame_version) = super::parse_frame_selector(frame_selector)?;
    let domain = repos
        .domains
        .get_domain_version(&domain_id, &domain_version)
        .await?
        .with_context(|| format!("domain version not found: {domain_selector}"))?;
    let frame = repos
        .domains
        .get_measurement_frame(&frame_id, &frame_version)
        .await?
        .with_context(|| format!("measurement frame not found: {frame_selector}"))?;
    let parsed = document.resolve()?;
    ensure!(
        parsed.profile.inferrers.is_empty()
            && parsed.profile.candidate_generators.is_empty()
            && parsed.profile.interpreters.is_empty(),
        "experiments execute sensors, comparators, and null models only"
    );
    ensure!(
        !parsed.profile.sensors.is_empty() && !parsed.profile.comparators.is_empty(),
        "experiments require selected sensors and comparators"
    );
    let engine = unclip_engine::Engine::with_builtins()?;
    let plan = engine.plan(&parsed.profile)?;
    ensure!(
        plan.sensors
            .iter()
            .any(|sensor| sensor.descriptor().id == request.comparison_sensor),
        "comparison sensor is not selected by the profile"
    );
    let candidate_record = repos
        .experiments
        .get_candidate(&request.candidate)
        .await?
        .context("candidate not found")?;
    ensure!(
        repos
            .provenance
            .get_provenance(&request.candidate)
            .await?
            .is_some(),
        "candidate provenance not found"
    );
    let candidate_ancestors = repos.provenance.ancestors(&request.candidate).await?;
    let candidate = Tracked::from_recorded(request.candidate.clone(), candidate_record.proposal);

    let selected_ids = request
        .training
        .iter()
        .chain(&request.held_out)
        .collect::<BTreeSet<_>>();
    ensure!(
        selected_ids.len() == request.training.len() + request.held_out.len(),
        "training and held-out observation IDs must be unique and disjoint"
    );
    let mut observations = Vec::new();
    for id in request.training.iter().chain(&request.held_out) {
        let record = repos
            .observations
            .get_recorded_observation(id)
            .await?
            .with_context(|| format!("observation not found: {id:?}"))?;
        observations.push(record.into_tracked(Some(Operation::Inferred)));
    }
    let mut alignments = Vec::new();
    let mut rankings = Vec::new();
    for id in &request.held_out {
        alignments.extend(
            repos
                .observations
                .alignments_for_observation(id)
                .await?
                .into_iter()
                .map(|record| record.into_tracked(Some(Operation::Inferred))),
        );
        rankings.extend(
            repos
                .observations
                .rankings_for_observation(id)
                .await?
                .into_iter()
                .map(|record| record.into_tracked(Some(Operation::Inferred))),
        );
    }
    let mut transfer = Vec::new();
    let mut transfer_profiles = BTreeSet::new();
    for profile_id in &request.transfer_measurement_profiles {
        ensure!(
            transfer_profiles.insert(profile_id),
            "duplicate transfer profile selection: {profile_id}"
        );
        let records = repos
            .measurements
            .get_profile_records(profile_id)
            .await?
            .with_context(|| format!("transfer measurement profile not found: {profile_id}"))?;
        transfer.extend(records.into_iter().map(|record| record.into_tracked()));
    }
    let timestamp = unclip_store::now();
    let baseline_id = DerivedId::new(format!("{}/baseline", request.run_id));
    let frame_snapshot_id = DerivedId::new(format!("{}/frame", request.run_id));
    // The run tracks the same snapshot and frame it goes on to read versions
    // from, so both are shared rather than copied into the tracked inputs.
    let domain = Arc::new(domain);
    let frame = Arc::new(frame);
    let baseline = Tracked::from_shared(baseline_id.clone(), Arc::clone(&domain), None);
    let tracked_frame = Tracked::from_shared(frame_snapshot_id.clone(), Arc::clone(&frame), None);
    let split = unclip_engine::select_observations(
        &observations,
        &request.training,
        &request.held_out,
        &request.run_id,
        Timestamp::new(timestamp.clone()),
    )?;
    let held_out_inference_products = alignments
        .iter()
        .map(|value| value.id().clone())
        .chain(rankings.iter().map(|value| value.id().clone()))
        .collect::<Vec<_>>();
    unclip_engine::validate_candidate_ancestry(
        candidate.id(),
        &candidate_ancestors,
        split.value(),
        &held_out_inference_products,
    )?;
    let tracked_split = Tracked::from(&split);
    let application_id = format!("{}/application", request.run_id);
    let application = engine.apply_candidate_with_relation_bindings(
        &baseline,
        &candidate,
        request.relation_bindings.as_ref(),
        &application_id,
        Timestamp::new(timestamp.clone()),
    )?;
    let pair = ComparisonPair {
        before: DerivedId::new(format!(
            "{}/before/{}",
            request.run_id, request.comparison_sensor
        )),
        after: DerivedId::new(format!(
            "{}/after/{}",
            request.run_id, request.comparison_sensor
        )),
    };
    let experiment = engine.run_counterfactual_experiment(
        &plan,
        CounterfactualMeasurementInputs {
            baseline: HeldOutInputs {
                baseline: &baseline,
                frame: &tracked_frame,
                split: &tracked_split,
                alignments: &alignments,
                rankings: &rankings,
            },
            counterfactual: &application,
            alignments: &alignments,
            rankings: &rankings,
        },
        &candidate,
        std::slice::from_ref(&pair),
        ExperimentConstraints {
            requirements: &request.constraints,
            transfer_measurements: &transfer,
            pareto_dimensions: &request.pareto_dimensions,
        },
        unclip_engine::MeasurementRun {
            id: &request.run_id,
            timestamp: Timestamp::new(timestamp.clone()),
            params: &parsed.params,
        },
    )?;
    let before_profile_id = format!("{}/before-profile", request.run_id);
    let after_profile_id = format!("{}/after-profile", request.run_id);
    let domain_key = serde_json::to_string(&(domain.id.as_str(), domain.version.as_str()))?;
    let frame_key = serde_json::to_string(&(frame.id.as_str(), frame.version.as_str()))?;
    let persistable = unclip_engine::persistable_experiment(
        &experiment,
        &split,
        &candidate,
        unclip_engine::ExperimentStorageIds {
            domain_version_id: &domain_key,
            frame_version_id: &frame_key,
            before_profile_id: &before_profile_id,
            after_profile_id: &after_profile_id,
            started_at: &timestamp,
        },
    )?;
    let before_pair = experiment
        .execution
        .measurements
        .before
        .iter()
        .find(|value| value.id() == &pair.before)
        .context("selected before measurement was not emitted")?;
    let after_pair = experiment
        .execution
        .measurements
        .after
        .iter()
        .find(|value| value.id() == &pair.after)
        .context("selected after measurement was not emitted")?;
    let (sensor_run_ids, sensor_runs) = stored_sensor_runs(
        &request.run_id,
        &experiment.execution.measurements.before,
        &experiment.execution.measurements.after,
        &parsed.params,
        &timestamp,
    )?;
    let profile_context = ProfileContext {
        run_id: &request.run_id,
        frame: &frame,
        sensor_run_ids: &sensor_run_ids,
        plan: &plan,
        timestamp: &timestamp,
    };
    let before_profile = stored_profile(
        &profile_context,
        &before_profile_id,
        &experiment.execution.measurements.before,
        before_pair.id().clone(),
        sensor_runs,
    )?;
    // The sensor runs are written with the before profile; the after profile
    // references the same rows rather than inserting them twice.
    let after_profile = stored_profile(
        &profile_context,
        &after_profile_id,
        &experiment.execution.measurements.after,
        after_pair.id().clone(),
        vec![],
    )?;
    let mut prerequisite_provenance = vec![
        root_provenance(
            &request.run_id,
            baseline_id,
            "storage.domain-snapshot",
            &timestamp,
            Some(domain.version.clone()),
            None,
        ),
        root_provenance(
            &request.run_id,
            frame_snapshot_id,
            "storage.frame-snapshot",
            &timestamp,
            Some(domain.version.clone()),
            Some(frame.version.clone()),
        ),
        StoredProvenance {
            id: split.id().clone(),
            run_id: Some(request.run_id.clone()),
            provenance: split.shared_provenance(),
        },
        StoredProvenance {
            id: application.id().clone(),
            run_id: Some(request.run_id.clone()),
            provenance: application.shared_provenance(),
        },
    ];
    prerequisite_provenance.extend(
        experiment
            .execution
            .measurements
            .before
            .iter()
            .chain(&experiment.execution.measurements.after)
            .map(|value| StoredProvenance {
                id: value.id().clone(),
                run_id: Some(request.run_id.clone()),
                provenance: value.shared_provenance(),
            }),
    );
    prerequisite_provenance.extend(
        experiment
            .null_results
            .iter()
            .map(|value| StoredProvenance {
                id: value.id().clone(),
                run_id: Some(request.run_id.clone()),
                provenance: value.shared_provenance(),
            }),
    );
    if let Some(value) = &experiment.constraints {
        prerequisite_provenance.push(StoredProvenance {
            id: value.id().clone(),
            run_id: Some(request.run_id.clone()),
            provenance: value.shared_provenance(),
        });
    }
    if let Some(value) = &experiment.pareto {
        prerequisite_provenance.push(StoredProvenance {
            id: value.id().clone(),
            run_id: Some(request.run_id.clone()),
            provenance: value.shared_provenance(),
        });
    }
    let post_delta_provenance = vec![
        StoredProvenance {
            id: experiment.execution.comparison.profile.id().clone(),
            run_id: Some(request.run_id.clone()),
            provenance: experiment.execution.comparison.profile.shared_provenance(),
        },
        StoredProvenance {
            id: experiment.evidence.id().clone(),
            run_id: Some(request.run_id.clone()),
            provenance: experiment.evidence.shared_provenance(),
        },
    ];
    let run_record = unclip_engine::run_record(
        &plan,
        &parsed.params,
        &request.run_id,
        Timestamp::new(timestamp.clone()),
        serde_json::json!({
            "stage":"experiment",
            "profile":profile_path,
            "request":&request,
            "experiment":persistable.outcome.id(),
        }),
    );
    repos.engine_runs.insert_run(run_record).await?;
    repos
        .engine_runs
        .transition_run(&request.run_id, EngineRunStatus::Running, None)
        .await?;
    let experiment_id = persistable.outcome.id().clone();
    let result = repos
        .experiments
        .insert_completed_experiment_bundle(
            &request.run_id,
            CompletedExperimentBundle {
                prerequisite_provenance,
                profiles: vec![before_profile, after_profile],
                post_delta_provenance,
                experiment: persistable.outcome,
                deltas: persistable.deltas,
            },
        )
        .await
        .map_err(anyhow::Error::from);
    super::finish_run(&repos.engine_runs, &request.run_id, result).await?;
    crate::output::outln!("EXPERIMENT\tEXPERIMENTAL\t{}", experiment_id);
    crate::output::outln!(
        "RESULT\t{}",
        serde_json::to_string(experiment.evidence.value())?
    );
    Ok(())
}
