//! Candidate generation from explicitly selected stored evidence.
use anyhow::{ensure, Context};
use serde::Deserialize;
use std::collections::BTreeSet;
use unclip_epistemic::{DerivedId, Timestamp, Tracked};
use unclip_store::{
    CandidateRepository, DomainReader, EngineRunRecord, EngineRunRepository, EngineRunStatus,
    MeasurementRecord, MeasurementRepository, ObservationRepository, ProvenanceRepository,
    RecordedInference,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedStructure {
    #[allow(dead_code)]
    id: String,
    provenance: DerivedId,
    value: unclip_measure::EmpiricalStructure,
}

fn calculate(
    run: &EngineRunRecord,
    domain: &str,
    measurements: Vec<MeasurementRecord>,
    observations: Vec<RecordedInference<unclip_observe::Observation>>,
    structures: Vec<SelectedStructure>,
) -> anyhow::Result<Vec<unclip_epistemic::Calculated<unclip_domain::CandidateProposal>>> {
    let (profile, params) = super::resolved_profile(&run.resolved_plan)?;
    let engine = unclip_engine::Engine::with_builtins()?;
    let plan = engine.plan(&profile)?;
    let measurements = measurements
        .into_iter()
        .map(|record| Tracked::from_recorded(record.provenance, record.measurement))
        .collect::<Vec<_>>();
    let observations = observations
        .into_iter()
        .map(|record| Tracked::from_recorded(record.provenance, record.value))
        .collect::<Vec<_>>();
    let structures = structures
        .into_iter()
        .map(|record| Tracked::from_recorded(record.provenance, record.value))
        .collect::<Vec<_>>();
    let (domain_id, version) = super::parse_domain_selector(domain)?;
    let domain_key = serde_json::to_string(&(&domain_id.0, &version.0))?;
    Ok(engine.generate_candidates(
        &plan,
        unclip_engine::CandidateInputs {
            domain_version_id: &domain_key,
            measurements: &measurements,
            observations: &observations,
            structures: &structures,
        },
        unclip_engine::MeasurementRun {
            id: &run.id,
            timestamp: Timestamp::new(run.started_at.clone()),
            params: &params,
        },
    )?)
}

pub(crate) async fn discover(
    repos: &crate::db::Repos,
    profile: &std::path::Path,
    profiles: &[String],
    observations: &[String],
    structures: &[String],
) -> anyhow::Result<()> {
    let document = unclip_io::load_engine_profile(profile)?;
    let selector = document
        .domain
        .as_deref()
        .context("discovery profile must select domain@version")?;
    let (domain, version) = super::parse_domain_selector(selector)?;
    ensure!(
        repos
            .domains
            .get_domain_version(&domain, &version)
            .await?
            .is_some(),
        "domain version not found: {selector}"
    );
    let parsed = document.resolve()?;
    ensure!(
        !parsed.profile.candidate_generators.is_empty(),
        "discovery requires explicitly selected candidate generators"
    );
    ensure!(
        parsed.profile.sensors.is_empty()
            && parsed.profile.inferrers.is_empty()
            && parsed.profile.comparators.is_empty()
            && parsed.profile.null_models.is_empty(),
        "discovery executes only candidate generators"
    );
    ensure!(
        !profiles.is_empty() || !observations.is_empty() || !structures.is_empty(),
        "discovery requires explicit stored evidence selections"
    );
    let mut measurements = Vec::new();
    let mut observed = Vec::new();
    let mut empirical = Vec::new();
    let mut snapshot = serde_json::json!({"domain":selector,"profiles":profiles,"observation_ids":observations,"structure_ids":structures,"measurements":[],"observations":[],"structures":[]});
    let mut unique = BTreeSet::new();
    let mut measurement_ids = BTreeSet::new();
    for id in profiles {
        ensure!(unique.insert(id), "duplicate profile selection: {id}");
        let records = repos
            .measurements
            .get_profile_records(id)
            .await?
            .with_context(|| format!("measurement profile not found: {id}"))?;
        for record in records {
            ensure!(
                measurement_ids.insert(record.provenance.clone()),
                "duplicate measurement provenance: {}",
                record.provenance
            );
            measurements.push(Tracked::from_recorded(
                record.provenance.clone(),
                record.measurement.clone(),
            ));
            snapshot["measurements"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::to_value(record)?);
        }
    }
    let mut unique = BTreeSet::new();
    for id in observations {
        ensure!(unique.insert(id), "duplicate observation selection: {id}");
        let record = repos
            .observations
            .get_recorded_observation(&unclip_observe::ObservationId::new(id))
            .await?
            .with_context(|| format!("observation not found: {id}"))?;
        observed.push(Tracked::from_recorded(
            record.provenance.clone(),
            record.value.clone(),
        ));
        snapshot["observations"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::to_value(record)?);
    }
    let mut unique = BTreeSet::new();
    for id in structures {
        ensure!(unique.insert(id), "duplicate structure selection: {id}");
        let record = repos
            .measurements
            .get_empirical_structure(id)
            .await?
            .with_context(|| format!("empirical structure not found: {id}"))?;
        empirical.push(Tracked::from_recorded(
            record.provenance.clone(),
            record.structure.clone(),
        ));
        snapshot["structures"].as_array_mut().unwrap().push(serde_json::json!({"id":record.id,"provenance":record.provenance,"value":record.structure}));
    }
    let engine = unclip_engine::Engine::with_builtins()?;
    let plan = engine.plan(&parsed.profile)?;
    let timestamp = unclip_store::now();
    let run_id = format!("discover-{timestamp}");
    let domain_key = serde_json::to_string(&(&domain.0, &version.0))?;
    let outputs = engine.generate_candidates(
        &plan,
        unclip_engine::CandidateInputs {
            domain_version_id: &domain_key,
            measurements: &measurements,
            observations: &observed,
            structures: &empirical,
        },
        unclip_engine::MeasurementRun {
            id: &run_id,
            timestamp: Timestamp::new(timestamp.clone()),
            params: &parsed.params,
        },
    )?;
    let record = engine.run_record(&plan, &parsed.params, &run_id, Timestamp::new(timestamp), serde_json::json!({"stage":"discovery","snapshot":snapshot,"outputs":outputs.iter().map(|c| c.id()).collect::<Vec<_>>()}));
    repos.engine_runs.insert_run(record).await?;
    repos
        .engine_runs
        .transition_run(&run_id, EngineRunStatus::Running, None)
        .await?;
    let persist: anyhow::Result<()> = async {
        for candidate in &outputs {
            repos
                .experiments
                .insert_candidate(Some(run_id.clone()), candidate.clone())
                .await?;
        }
        Ok(())
    }
    .await;
    if let Err(error) = persist {
        repos
            .engine_runs
            .transition_run(&run_id, EngineRunStatus::Failed, Some(unclip_store::now()))
            .await?;
        return Err(error);
    }
    repos
        .engine_runs
        .transition_run(
            &run_id,
            EngineRunStatus::Completed,
            Some(unclip_store::now()),
        )
        .await?;
    crate::output::outln!("CALCULATED\tDISCOVERY\trun={run_id}");
    for candidate in outputs {
        crate::output::outln!(
            "CANDIDATE\t{}\t{}",
            candidate.id(),
            serde_json::to_string(candidate.value())?
        );
    }
    Ok(())
}

pub(crate) async fn verify(repos: &crate::db::Repos, run: &EngineRunRecord) -> anyhow::Result<()> {
    ensure!(
        run.status == EngineRunStatus::Completed,
        "discovery run is not completed: {}",
        run.id
    );
    let snapshot = run
        .metadata
        .get("snapshot")
        .context("discovery run has no input snapshot")?;
    let domain = snapshot
        .get("domain")
        .and_then(serde_json::Value::as_str)
        .context("discovery snapshot has no domain selector")?;
    let measurements: Vec<MeasurementRecord> = serde_json::from_value(
        snapshot
            .get("measurements")
            .cloned()
            .context("discovery snapshot has no measurements")?,
    )?;
    let observations: Vec<RecordedInference<unclip_observe::Observation>> = serde_json::from_value(
        snapshot
            .get("observations")
            .cloned()
            .context("discovery snapshot has no observations")?,
    )?;
    let structures: Vec<SelectedStructure> = serde_json::from_value(
        snapshot
            .get("structures")
            .cloned()
            .context("discovery snapshot has no structures")?,
    )?;
    let outputs = calculate(run, domain, measurements, observations, structures)?;
    let manifest: Vec<DerivedId> = serde_json::from_value(
        run.metadata
            .get("outputs")
            .cloned()
            .context("discovery run has no output manifest")?,
    )?;
    ensure!(
        outputs
            .iter()
            .map(|value| value.id().clone())
            .collect::<Vec<_>>()
            == manifest,
        "discovery output manifest differs from stored run"
    );
    for output in &outputs {
        let stored = repos
            .experiments
            .get_candidate(output.id())
            .await?
            .with_context(|| format!("candidate not found: {}", output.id()))?;
        let provenance = repos
            .provenance
            .get_provenance(output.id())
            .await?
            .with_context(|| format!("candidate provenance not found: {}", output.id()))?;
        ensure!(
            stored.proposal == *output.value()
                && stored.created_at == run.started_at
                && provenance.provenance == *output.provenance()
                && provenance.run_id.as_ref() == Some(&run.id),
            "discovery candidate differs from stored result: {}",
            output.id()
        );
    }
    crate::output::outln!(
        "VERIFIED\tDISCOVERY_REPLAY\trun={} candidates={}",
        run.id,
        outputs.len()
    );
    Ok(())
}
