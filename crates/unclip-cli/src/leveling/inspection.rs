//! Read-back commands: replay a run, and inspect what it recorded.
//!
//! `verify` reruns the calculated stages of a stored run and compares both
//! values and provenance against what was persisted. It deliberately makes no
//! claim about reproducing the inference stage, which depends on an external
//! model; `explain` inspects that recorded inference instead.
use super::{discovery, empirical, parse_domain_selector, parse_frame_selector, resolved_profile};

pub(crate) async fn verify(repositories: &crate::db::Repos, run_id: &str) -> anyhow::Result<()> {
    let run = unclip_store::EngineRunRepository::get_run(&repositories.engine_runs, run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("engine run not found: {run_id}"))?;
    if run
        .metadata
        .get("stage")
        .and_then(serde_json::Value::as_str)
        == Some("empirical")
    {
        return empirical::verify(repositories, &run).await;
    }
    if run
        .metadata
        .get("stage")
        .and_then(serde_json::Value::as_str)
        == Some("discovery")
    {
        return discovery::verify(repositories, &run).await;
    }
    let replay = unclip_store::EngineRunRepository::replay_run(&repositories.engine_runs, run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("engine run not found: {run_id}"))?;
    anyhow::ensure!(
        !replay.observations.is_empty(),
        "engine run has no persisted inference products: {run_id}"
    );
    let (profile, params) = resolved_profile(&replay.run.resolved_plan)?;
    let engine = unclip_engine::Engine::with_builtins()?;
    let plan = engine.plan(&profile)?;
    if plan.sensors.is_empty() {
        crate::output::outln!(
            "VERIFIED\tINFERENCE_REPLAY\trun={} observations={} alignments={} rankings={} calculated=0",
            run_id,
            replay.observations.len(),
            replay.alignments.len(),
            replay.rankings.len()
        );
        return Ok(());
    }

    let domain_selector = replay
        .run
        .metadata
        .get("domain")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("stored run metadata has no domain selector"))?;
    let frame_selector = replay
        .run
        .metadata
        .get("frame")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("stored run metadata has no frame selector"))?;
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
    let calculated = engine.verify(
        &plan,
        &domain,
        &frame,
        &replay,
        unclip_engine::MeasurementRun {
            id: run_id,
            timestamp: &unclip_epistemic::Timestamp::new(&replay.run.started_at),
            params: &params,
        },
    )?;
    anyhow::ensure!(
        !replay.profile_ids.is_empty(),
        "engine run has no persisted measurement profiles: {run_id}"
    );
    let mut expected = Vec::new();
    for profile_id in &replay.profile_ids {
        let stored = unclip_store::MeasurementRepository::get_profile(
            &repositories.measurements,
            profile_id,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("measurement profile not found: {profile_id}"))?;
        for measurement in stored.measurements {
            expected.push(serde_json::to_vec(&measurement)?);
        }
    }
    let mut actual = calculated
        .iter()
        .map(|value| serde_json::to_vec(value.value()))
        .collect::<Result<Vec<_>, _>>()?;
    for value in &calculated {
        let recorded = unclip_store::ProvenanceRepository::get_provenance(
            &repositories.provenance,
            value.id(),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("calculated provenance not found: {}", value.id()))?;
        anyhow::ensure!(
            *recorded.provenance == *value.provenance(),
            "calculated provenance differs from stored evidence for {}",
            value.id()
        );
    }
    expected.sort();
    actual.sort();
    anyhow::ensure!(
        actual == expected,
        "calculated measurements differ from stored profiles for run {run_id}"
    );
    crate::output::outln!(
        "VERIFIED\tINFERENCE_REPLAY\trun={} observations={} alignments={} rankings={} calculated={}",
        run_id,
        replay.observations.len(),
        replay.alignments.len(),
        replay.rankings.len(),
        calculated.len()
    );
    for value in calculated {
        crate::output::outln!(
            "MEASUREMENT\tCALCULATED\t{}@{}\t{}",
            value.value().sensor,
            value.value().sensor_version,
            serde_json::to_string(&value.value().reading)?
        );
    }
    Ok(())
}

pub(crate) async fn provenance(
    repository: &impl unclip_store::ProvenanceRepository,
    derived_id: &str,
) -> anyhow::Result<()> {
    let root = unclip_epistemic::DerivedId::new(derived_id);
    let mut queue = std::collections::VecDeque::from([(root, 0usize)]);
    let mut visited = std::collections::BTreeSet::new();
    while let Some((id, depth)) = queue.pop_front() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let value = repository.get_provenance(&id).await?.ok_or_else(|| {
            if depth == 0 {
                anyhow::anyhow!("provenance not found: {}", id)
            } else {
                anyhow::anyhow!("provenance dependency not found: {}", id)
            }
        })?;
        for input in &value.provenance.inputs {
            queue.push_back((input.clone(), depth + 1));
        }
        let operation = match value.provenance.operation {
            unclip_epistemic::Operation::Inferred => "INFERRED",
            unclip_epistemic::Operation::Calculated => "CALCULATED",
            unclip_epistemic::Operation::Experimental => "EXPERIMENTAL",
            unclip_epistemic::Operation::Interpreted => "INTERPRETED",
        };
        crate::output::outln!(
            "{}\t{}\t{}@{}\tdepth={} inputs={}",
            value.id.as_str(),
            operation,
            value.provenance.producer,
            value.provenance.version,
            depth,
            value.provenance.inputs.len()
        );
    }
    Ok(())
}

pub(crate) async fn profile_show(
    repository: &impl unclip_store::MeasurementRepository,
    profile_id: &str,
    format: unclip_io::Format,
    table: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        format != unclip_io::Format::Jsonl,
        "JSONL is not supported for measurement profile display"
    );
    let profile = repository
        .get_profile(profile_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("measurement profile not found: {profile_id}"))?;
    let rendered = if table {
        unclip_io::render_measurement_profile_table(&profile)?
    } else {
        unclip_io::render_measurement_profile(&profile, format)?
    };
    crate::output::write_stdout(&rendered)
}

pub(crate) async fn explain(
    repositories: &crate::db::Repos,
    observation_id: &str,
) -> anyhow::Result<()> {
    let id = unclip_observe::ObservationId::new(observation_id);
    let observation = unclip_store::ObservationRepository::get_recorded_observation(
        &repositories.observations,
        &id,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("observation not found: {observation_id}"))?;
    let alignments = unclip_store::ObservationRepository::alignments_for_observation(
        &repositories.observations,
        &id,
    )
    .await?;
    let rankings = unclip_store::ObservationRepository::rankings_for_observation(
        &repositories.observations,
        &id,
    )
    .await?;

    let provenance = unclip_store::ProvenanceRepository::get_provenance(
        &repositories.provenance,
        &observation.provenance,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("provenance not found: {}", observation.provenance))?;
    crate::output::outln!(
        "OBSERVATION\tINFERRED\t{}@{}\tid={} units={} relations={}",
        provenance.provenance.producer,
        provenance.provenance.version,
        observation.value.id.as_str(),
        observation.value.units.len(),
        observation.value.relations.len()
    );

    for alignment in alignments {
        let provenance = unclip_store::ProvenanceRepository::get_provenance(
            &repositories.provenance,
            &alignment.provenance,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("provenance not found: {}", alignment.provenance))?;
        let mut counts = std::collections::BTreeMap::new();
        for candidate in &alignment.value.candidates {
            *counts.entry(candidate.observed.as_str()).or_insert(0usize) += 1;
        }
        let ambiguous = counts.values().filter(|count| **count > 1).count();
        crate::output::outln!(
            "ALIGNMENT\tINFERRED\t{}@{}\tcandidates={} confident={} ambiguous={}",
            provenance.provenance.producer,
            provenance.provenance.version,
            alignment.value.candidates.len(),
            counts.len().saturating_sub(ambiguous),
            ambiguous
        );
    }
    for ranking in rankings {
        let provenance = unclip_store::ProvenanceRepository::get_provenance(
            &repositories.provenance,
            &ranking.provenance,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("provenance not found: {}", ranking.provenance))?;
        let known = ranking
            .value
            .tiers
            .iter()
            .map(|tier| tier.units.len())
            .sum::<usize>();
        crate::output::outln!(
            "RANKING\tINFERRED\t{}@{}\ttiers={} known={} unknown={}",
            provenance.provenance.producer,
            provenance.provenance.version,
            ranking.value.tiers.len(),
            known,
            ranking.value.unknown.len()
        );
    }
    Ok(())
}
