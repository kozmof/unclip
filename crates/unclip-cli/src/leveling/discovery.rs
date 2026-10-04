//! Candidate generation from explicitly selected stored evidence.
use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;
use unclip_epistemic::{DerivedId, Operation, Timestamp, Tracked};
use unclip_store::{
    CandidateRepository, DomainReader, EngineRunRecord, EngineRunRepository, EngineRunStatus,
    MeasurementRecord, MeasurementRepository, ObservationRepository, ProvenanceRepository,
    RecordedInference,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedStructure {
    id: String,
    provenance: DerivedId,
    /// Shared with the tracked copy handed to the engine: the snapshot records
    /// the structure a run read, and the run reads the same allocation.
    value: Arc<unclip_measure::EmpiricalStructure>,
}

/// The evidence a discovery run selected, recorded in its run metadata.
///
/// One type serves both sides: `discover` builds it, and `verify` reads it back
/// to recalculate the run. That is the point of declaring it — the snapshot's
/// shape is what makes a discovery run replayable, so the writer and the reader
/// must not be able to drift apart. `deny_unknown_fields` makes a snapshot
/// written by a different version fail loudly instead of verifying against
/// silently missing evidence.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoverySnapshot {
    domain: String,
    profiles: Vec<String>,
    observation_ids: Vec<String>,
    structure_ids: Vec<String>,
    measurements: Vec<MeasurementRecord>,
    observations: Vec<RecordedInference<unclip_observe::Observation>>,
    structures: Vec<SelectedStructure>,
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
        .map(|record| record.into_tracked())
        .collect::<Vec<_>>();
    let observations = observations
        .into_iter()
        .map(|record| record.into_tracked(Some(Operation::Inferred)))
        .collect::<Vec<_>>();
    let structures = structures
        .into_iter()
        // The label is left exactly as this path has always claimed it, so
        // sharing the payload changes nothing but the copy. Note that
        // `discover` labels the same structures `Calculated`; the two paths
        // disagree, which is a separate question from this one.
        .map(|record| {
            Tracked::from_shared(record.provenance, record.value, Some(Operation::Inferred))
        })
        .collect::<Vec<_>>();
    let (domain_id, version) = super::parse_domain_selector(domain)?;
    let domain_key = serde_json::to_string(&(domain_id.as_str(), version.as_str()))?;
    Ok(unclip_engine::generate_candidates(
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
            && parsed.profile.interpreters.is_empty()
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
    let mut snapshot = DiscoverySnapshot {
        domain: selector.to_owned(),
        profiles: profiles.to_vec(),
        observation_ids: observations.to_vec(),
        structure_ids: structures.to_vec(),
        measurements: Vec::new(),
        observations: Vec::new(),
        structures: Vec::new(),
    };
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
            measurements.push(record.tracked());
            snapshot.measurements.push(record);
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
        observed.push(record.tracked(Some(Operation::Inferred)));
        snapshot.observations.push(record);
    }
    let mut unique = BTreeSet::new();
    for id in structures {
        ensure!(unique.insert(id), "duplicate structure selection: {id}");
        let record = repos
            .measurements
            .get_empirical_structure(id)
            .await?
            .with_context(|| format!("empirical structure not found: {id}"))?;
        let structure = Arc::new(record.structure);
        empirical.push(Tracked::from_shared(
            record.provenance.clone(),
            Arc::clone(&structure),
            Some(Operation::Calculated),
        ));
        snapshot.structures.push(SelectedStructure {
            id: record.id,
            provenance: record.provenance,
            value: structure,
        });
    }
    let engine = unclip_engine::Engine::with_builtins()?;
    let plan = engine.plan(&parsed.profile)?;
    let timestamp = unclip_store::now();
    let run_id = super::run_id("discover", &timestamp);
    let domain_key = serde_json::to_string(&(domain.as_str(), version.as_str()))?;
    let outputs = unclip_engine::generate_candidates(
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
    let record = unclip_engine::run_record(
        &plan,
        &parsed.params,
        &run_id,
        Timestamp::new(timestamp),
        serde_json::json!({"stage":"discovery","snapshot":snapshot,"outputs":outputs.iter().map(|c| c.id()).collect::<Vec<_>>()}),
    );
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
    super::finish_run(&repos.engine_runs, &run_id, persist).await?;
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
    // One typed read replaces five field lookups: the snapshot's shape is
    // stated once, by the struct `discover` wrote, and a snapshot missing or
    // misnaming any part of it fails here with serde saying which.
    let snapshot: DiscoverySnapshot = serde_json::from_value(
        run.metadata
            .get("snapshot")
            .cloned()
            .context("discovery run has no input snapshot")?,
    )
    .context("discovery run snapshot does not match the recorded evidence shape")?;
    let outputs = calculate(
        run,
        &snapshot.domain,
        snapshot.measurements,
        snapshot.observations,
        snapshot.structures,
    )?;
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
                && *provenance.provenance == *output.provenance()
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
