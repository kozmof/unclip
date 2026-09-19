//! Stored-profile selection and reproducible empirical calculations.
use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use unclip_engine::{EmpiricalMethod, EmpiricalResult, Engine};
use unclip_epistemic::{DerivedId, Timestamp, Tracked};
use unclip_measure::MeasurementKind;
use unclip_store::{
    EngineRunRecord, EngineRunRepository, EngineRunStatus, MeasurementRecord,
    MeasurementRepository, ProvenanceRepository,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedMeasurement {
    profile: String,
    record: MeasurementRecord,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    inputs: Vec<SelectedMeasurement>,
    outputs: Vec<(DerivedId, Option<DerivedId>)>,
}

fn calculate(run: &EngineRunRecord, snapshot: &Snapshot) -> anyhow::Result<Vec<EmpiricalResult>> {
    let method: EmpiricalMethod = serde_json::from_value(run.resolved_plan.clone())?;
    let inputs = snapshot
        .inputs
        .iter()
        .map(|input| {
            Tracked::from_recorded(
                input.record.provenance.clone(),
                input.record.measurement.clone(),
            )
        })
        .collect::<Vec<_>>();
    Ok(Engine::with_builtins()?.derive_empirical(
        &inputs,
        method,
        &run.id,
        Timestamp::new(run.started_at.clone()),
    )?)
}

pub(crate) async fn derive(
    repos: &crate::db::Repos,
    profiles: &[String],
    config: &std::path::Path,
) -> anyhow::Result<()> {
    let method: EmpiricalMethod = serde_norway::from_str(&std::fs::read_to_string(config)?)
        .context("invalid empirical method configuration")?;
    let mut unique = std::collections::BTreeSet::new();
    let mut snapshot = Snapshot {
        inputs: Vec::new(),
        outputs: Vec::new(),
    };
    for profile in profiles {
        ensure!(
            unique.insert(profile),
            "duplicate profile selection: {profile}"
        );
        let records = repos
            .measurements
            .get_profile_records(profile)
            .await?
            .ok_or_else(|| anyhow::anyhow!("measurement profile not found: {profile}"))?;
        let mut count = 0;
        for record in records
            .into_iter()
            .filter(|record| record.kind == MeasurementKind::Matrix)
        {
            ensure!(
                repos
                    .provenance
                    .get_provenance(&record.provenance)
                    .await?
                    .is_some(),
                "measurement provenance not found: {}",
                record.provenance
            );
            snapshot.inputs.push(SelectedMeasurement {
                profile: profile.clone(),
                record,
            });
            count += 1;
        }
        ensure!(count > 0, "profile has no matrix measurements: {profile}");
    }
    let timestamp = unclip_store::now();
    let mut run = EngineRunRecord {
        id: format!("empirical-{timestamp}"),
        resolved_plan: serde_json::to_value(method)?,
        status: EngineRunStatus::Planned,
        started_at: timestamp,
        completed_at: None,
        metadata: serde_json::json!({}),
    };
    // Validate and calculate before any write, including malformed matrix evidence.
    let outputs = calculate(&run, &snapshot)?;
    snapshot.outputs = outputs
        .iter()
        .map(|output| {
            (
                output.measurement.clone(),
                output
                    .structure
                    .as_ref()
                    .map(|structure| structure.id().clone()),
            )
        })
        .collect();
    run.metadata = serde_json::json!({"stage":"empirical", "snapshot":snapshot});
    repos.engine_runs.insert_run(run.clone()).await?;
    repos
        .engine_runs
        .transition_run(&run.id, EngineRunStatus::Running, None)
        .await?;
    let persist: anyhow::Result<()> = async {
        for output in &outputs {
            if let Some(structure) = &output.structure {
                let source = snapshot
                    .inputs
                    .iter()
                    .find(|input| input.record.provenance == output.measurement)
                    .expect("calculated source is selected");
                repos
                    .measurements
                    .insert_calculated_structure(
                        Some(run.id.clone()),
                        Some(source.profile.clone()),
                        structure.clone(),
                    )
                    .await?;
            }
        }
        Ok(())
    }
    .await;
    if let Err(error) = persist {
        repos
            .engine_runs
            .transition_run(&run.id, EngineRunStatus::Failed, Some(unclip_store::now()))
            .await?;
        return Err(error);
    }
    repos
        .engine_runs
        .transition_run(
            &run.id,
            EngineRunStatus::Completed,
            Some(unclip_store::now()),
        )
        .await?;
    crate::output::outln!("CALCULATED\tEMPIRICAL\trun={}", run.id);
    for output in outputs {
        match output.structure {
            Some(structure) => crate::output::outln!(
                "STRUCTURE\t{}\tsource={}",
                structure.id(),
                output.measurement
            ),
            None => crate::output::outln!("INSUFFICIENT_EVIDENCE\tsource={}", output.measurement),
        }
    }
    Ok(())
}

pub(crate) async fn verify(repos: &crate::db::Repos, run: &EngineRunRecord) -> anyhow::Result<()> {
    ensure!(
        run.status == EngineRunStatus::Completed,
        "empirical run is not completed: {}",
        run.id
    );
    let snapshot: Snapshot = serde_json::from_value(
        run.metadata
            .get("snapshot")
            .cloned()
            .context("empirical run has no input snapshot")?,
    )?;
    let outputs = calculate(run, &snapshot)?;
    let manifest = outputs
        .iter()
        .map(|output| {
            (
                output.measurement.clone(),
                output
                    .structure
                    .as_ref()
                    .map(|structure| structure.id().clone()),
            )
        })
        .collect::<Vec<_>>();
    ensure!(
        manifest == snapshot.outputs,
        "empirical output manifest differs from stored run"
    );
    let mut count = 0;
    for output in outputs {
        if let Some(structure) = output.structure {
            let stored = repos
                .measurements
                .get_empirical_structure(&structure.id().0)
                .await?
                .context("empirical structure not found")?;
            let provenance = repos
                .provenance
                .get_provenance(structure.id())
                .await?
                .context("empirical provenance not found")?;
            let source = snapshot
                .inputs
                .iter()
                .find(|input| input.record.provenance == output.measurement)
                .context("empirical source not selected")?;
            ensure!(
                stored.structure == *structure.value()
                    && stored.provenance == *structure.id()
                    && stored.profile_id.as_ref() == Some(&source.profile)
                    && stored.created_at == run.started_at,
                "calculated empirical structure differs from stored result: {}",
                structure.id()
            );
            ensure!(
                provenance.provenance == *structure.provenance()
                    && provenance.run_id.as_ref() == Some(&run.id),
                "empirical provenance differs from stored evidence: {}",
                structure.id()
            );
            count += 1;
        }
    }
    crate::output::outln!("VERIFIED\tEMPIRICAL\trun={} calculated={count}", run.id);
    Ok(())
}

pub(crate) async fn show(
    repos: &crate::db::Repos,
    id: &str,
    format: unclip_io::Format,
) -> anyhow::Result<()> {
    let record = repos
        .measurements
        .get_empirical_structure(id)
        .await?
        .context("empirical structure not found")?;
    let rendered = match format {
        unclip_io::Format::Json => serde_json::to_string_pretty(&record.structure)?,
        unclip_io::Format::Yaml => serde_norway::to_string(&record.structure)?,
        unclip_io::Format::Jsonl => {
            anyhow::bail!("JSONL is not supported for empirical structure display")
        }
    };
    crate::output::write_stdout(&format!("{rendered}\n"))
}
