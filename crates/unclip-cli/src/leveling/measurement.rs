//! `unclip level measure` — calculate a profile over selected observations.
use super::{parse_domain_selector, parse_frame_selector};

pub(crate) async fn measure(
    repositories: &crate::db::Repos,
    observation_ids: &[String],
    profile_path: &std::path::Path,
) -> anyhow::Result<()> {
    let document = unclip_io::load_engine_profile(profile_path)?;
    let domain_selector = document
        .domain
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("engine profile must select domain@version"))?;
    let frame_selector = document
        .frame
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("engine profile must select frame@version"))?;
    let (domain_id, domain_version) = parse_domain_selector(domain_selector)?;
    let (frame_id, frame_version) = parse_frame_selector(frame_selector)?;
    let domain = unclip_store::DomainReader::get_domain_version(
        &repositories.domains,
        &domain_id,
        &domain_version,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("domain version not found: {domain_selector}"))?;
    let frame = unclip_store::DomainReader::get_measurement_frame(
        &repositories.domains,
        &frame_id,
        &frame_version,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("measurement frame version not found: {frame_selector}"))?;

    anyhow::ensure!(
        !observation_ids.is_empty(),
        "select at least one observation"
    );
    let unique = observation_ids
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    anyhow::ensure!(
        unique.len() == observation_ids.len(),
        "observation IDs must be unique"
    );
    let mut snapshot = unclip_store::MeasurementInputSnapshot {
        observations: Vec::new(),
        alignments: Vec::new(),
        rankings: Vec::new(),
    };
    for id in observation_ids {
        let observation_id = unclip_observe::ObservationId::new(id);
        let observation = unclip_store::ObservationRepository::get_recorded_observation(
            &repositories.observations,
            &observation_id,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("observation not found: {id}"))?;
        snapshot.observations.push(observation);
        snapshot.alignments.extend(
            unclip_store::ObservationRepository::alignments_for_observation(
                &repositories.observations,
                &observation_id,
            )
            .await?,
        );
        snapshot.rankings.extend(
            unclip_store::ObservationRepository::rankings_for_observation(
                &repositories.observations,
                &observation_id,
            )
            .await?,
        );
    }
    let observations = snapshot
        .observations
        .iter()
        .map(|record| record.tracked(Some(unclip_epistemic::Operation::Inferred)))
        .collect::<Vec<_>>();
    let alignments = snapshot
        .alignments
        .iter()
        .map(|record| record.tracked(Some(unclip_epistemic::Operation::Inferred)))
        .collect::<Vec<_>>();
    let rankings = snapshot
        .rankings
        .iter()
        .map(|record| record.tracked(Some(unclip_epistemic::Operation::Inferred)))
        .collect::<Vec<_>>();

    let parsed = document.resolve()?;
    anyhow::ensure!(
        parsed.profile.candidate_generators.is_empty()
            && parsed.profile.null_models.is_empty()
            && parsed.profile.comparators.is_empty()
            && parsed.profile.interpreters.is_empty(),
        "observation and measurement commands do not execute candidate generators, null models, interpreters, or comparators"
    );
    let engine = unclip_engine::Engine::with_builtins()?;
    let plan = engine.plan(&parsed.profile)?;
    anyhow::ensure!(
        !plan.sensors.is_empty(),
        "engine profile must select at least one sensor"
    );
    let timestamp = unclip_store::now();
    let run_id = super::run_id("measure", &timestamp);
    let profile_id = format!("{run_id}/profile");
    let run_record = unclip_engine::run_record(
        &plan,
        &parsed.params,
        &run_id,
        unclip_epistemic::Timestamp::new(timestamp.clone()),
        serde_json::json!({
            "observations": observation_ids,
            "measurement_inputs": snapshot,
            "domain": domain_selector,
            "frame": frame_selector,
            "profile": profile_path,
        }),
    );
    unclip_store::EngineRunRepository::insert_run(&repositories.engine_runs, run_record).await?;
    unclip_store::EngineRunRepository::transition_run(
        &repositories.engine_runs,
        &run_id,
        unclip_store::EngineRunStatus::Running,
        None,
    )
    .await?;
    let executed: anyhow::Result<Vec<String>> = async {
        let calculated = engine.measure(
            &plan,
            unclip_engine::MeasurementInputs {
                domain: &domain,
                frame: &frame,
                observations: &observations,
                alignments: &alignments,
                rankings: &rankings,
            },
            unclip_engine::MeasurementRun {
                id: &run_id,
                timestamp: unclip_epistemic::Timestamp::new(timestamp.clone()),
                params: &parsed.params,
            },
        )?;

        // Everything the profile needs is assembled first and written in one
        // transaction, and nothing is printed until it has committed, so a
        // failure leaves neither orphan rows nor reported measurements.
        let mut provenance = Vec::with_capacity(calculated.len());
        let mut sensor_runs = Vec::with_capacity(calculated.len());
        let mut records = Vec::with_capacity(calculated.len());
        let mut lines = Vec::with_capacity(calculated.len());
        for value in &calculated {
            let measurement = value.value();
            let descriptor = plan
                .sensors
                .iter()
                .map(|sensor| sensor.descriptor())
                .find(|descriptor| descriptor.id == measurement.sensor)
                .ok_or_else(|| {
                    anyhow::anyhow!("resolved sensor disappeared: {}", measurement.sensor)
                })?;
            let kind = match &measurement.reading {
                unclip_measure::Reading::Value { value } => value.kind(),
                _ => *descriptor.produces.first().ok_or_else(|| {
                    anyhow::anyhow!(
                        "sensor declares no measurement kind: {}",
                        measurement.sensor.as_str()
                    )
                })?,
            };
            provenance.push(unclip_store::StoredProvenance {
                id: value.id().clone(),
                run_id: Some(run_id.clone()),
                provenance: value.shared_provenance(),
            });
            let params = parsed
                .params
                .get(&measurement.sensor)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}).into());
            sensor_runs.push(unclip_store::SensorRunRecord {
                id: value.id().to_string(),
                engine_run_id: run_id.clone(),
                sensor: measurement.sensor.clone(),
                sensor_version: measurement.sensor_version.clone(),
                params: params.clone(),
                params_hash: unclip_epistemic::hash_params(&params),
                status: "completed".into(),
                started_at: timestamp.clone(),
                completed_at: Some(unclip_store::now()),
            });
            records.push(unclip_store::MeasurementRecord {
                id: format!("{}/measurement", value.id()),
                sensor_run_id: value.id().to_string(),
                provenance: value.id().clone(),
                kind,
                measurement: value.shared(),
            });
            lines.push(format!(
                "MEASUREMENT\tCALCULATED\t{}@{}\t{}",
                measurement.sensor,
                measurement.sensor_version,
                serde_json::to_string(&measurement.reading)?
            ));
        }
        // A sensor may emit nothing, so an empty result is an error to
        // report, not an invariant to assert.
        let profile_provenance = calculated
            .first()
            .ok_or_else(|| anyhow::anyhow!("the selected sensors produced no measurements"))?
            .id()
            .clone();
        unclip_store::MeasurementRepository::insert_calculated_profile(
            &repositories.measurements,
            provenance,
            sensor_runs,
            unclip_store::MeasurementProfileHeader {
                id: profile_id.clone(),
                engine_run_id: run_id.clone(),
                observation_id: (observation_ids.len() == 1).then(|| observation_ids[0].clone()),
                frame: frame_id,
                frame_version,
                provenance: profile_provenance,
                created_at: unclip_store::now(),
            },
            records,
        )
        .await?;
        Ok(lines)
    }
    .await;
    for line in super::finish_run(&repositories.engine_runs, &run_id, executed).await? {
        crate::output::outln!("{line}");
    }
    crate::output::outln!("PROFILE\tCALCULATED\t{profile_id}");
    Ok(())
}
