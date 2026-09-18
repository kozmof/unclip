//! Commands for the semantic leveling engine.

pub(crate) fn plugins() -> anyhow::Result<()> {
    let registry = unclip_engine::builtin_registry()?;
    let mut found = false;

    for plugin in registry.inferrers() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tinferred\t{}\t-\tinference_output",
            descriptor.id,
            descriptor.version
        );
    }
    for plugin in registry.sensors() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tcalculated\t{}\t{:?}/{:?}\t{:?}",
            descriptor.id,
            descriptor.version,
            descriptor.applicability,
            descriptor.evidence,
            descriptor.produces
        );
    }
    for plugin in registry.comparators() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tcalculated\t{}\t{:?}\tdelta",
            descriptor.id,
            descriptor.version,
            descriptor.supports
        );
    }

    if !found {
        crate::output::outln!("no leveling plugins registered");
    }
    Ok(())
}

pub(crate) async fn domain_import(
    repository: &impl unclip_store::DomainWriter,
    file: &std::path::Path,
) -> anyhow::Result<()> {
    let snapshot = unclip_io::load_domain(file)?;
    let selector = format!("{}@{}", snapshot.id.0, snapshot.version.0);
    let units = snapshot.units.len();
    let relations = snapshot.relations.len();
    repository.insert_domain_version(snapshot).await?;
    crate::output::outln!("imported domain {selector} ({units} unit(s), {relations} relation(s))");
    Ok(())
}

pub(crate) async fn domain_show(
    repository: &impl unclip_store::DomainReader,
    selector: &str,
    format: unclip_io::Format,
) -> anyhow::Result<()> {
    let (domain, version) = parse_domain_selector(selector)?;
    let snapshot = repository
        .get_domain_version(&domain, &version)
        .await?
        .ok_or_else(|| anyhow::anyhow!("domain version not found: {selector}"))?;
    crate::output::write_stdout(&unclip_io::render_domain(&snapshot, format)?)
}

fn parse_domain_selector(
    selector: &str,
) -> anyhow::Result<(unclip_domain::DomainId, unclip_epistemic::DomainVersion)> {
    let (domain, version) = selector
        .rsplit_once('@')
        .filter(|(domain, version)| !domain.is_empty() && !version.is_empty())
        .ok_or_else(|| anyhow::anyhow!("domain selector must be domain@version"))?;
    Ok((
        unclip_domain::DomainId::new(domain),
        unclip_epistemic::DomainVersion::new(version),
    ))
}

pub(crate) async fn frame_import(
    repository: &impl unclip_store::DomainWriter,
    file: &std::path::Path,
) -> anyhow::Result<()> {
    let document = unclip_io::load_measurement_frame(file)?;
    let selector = format!("{}@{}", document.frame.id.0, document.frame.version.0);
    let axes = document.frame.axes.len();
    repository
        .insert_measurement_frame(
            &document.domain_id,
            &document.domain_version,
            document.frame,
        )
        .await?;
    crate::output::outln!("imported measurement frame {selector} ({axes} axis/axes)");
    Ok(())
}

pub(crate) async fn frame_show(
    repository: &impl unclip_store::DomainReader,
    selector: &str,
    format: unclip_io::Format,
) -> anyhow::Result<()> {
    let (frame, version) = parse_frame_selector(selector)?;
    let snapshot = repository
        .get_measurement_frame(&frame, &version)
        .await?
        .ok_or_else(|| anyhow::anyhow!("measurement frame version not found: {selector}"))?;
    let rendered = match format {
        unclip_io::Format::Yaml => serde_norway::to_string(&snapshot)?,
        unclip_io::Format::Json => format!("{}\n", serde_json::to_string_pretty(&snapshot)?),
        unclip_io::Format::Jsonl => {
            anyhow::bail!("JSONL is not supported for measurement frames")
        }
    };
    crate::output::write_stdout(&rendered)
}

fn parse_frame_selector(
    selector: &str,
) -> anyhow::Result<(unclip_domain::FrameId, unclip_epistemic::FrameVersion)> {
    let (frame, version) = selector
        .rsplit_once('@')
        .filter(|(frame, version)| !frame.is_empty() && !version.is_empty())
        .ok_or_else(|| anyhow::anyhow!("frame selector must be frame@version"))?;
    Ok((
        unclip_domain::FrameId::new(frame),
        unclip_epistemic::FrameVersion::new(version),
    ))
}

struct FileInferenceIo;

#[async_trait::async_trait]
impl unclip_plugin::InferenceIo for FileInferenceIo {
    async fn request(
        &self,
        source: &unclip_epistemic::SourceRef,
        params: &serde_json::Value,
    ) -> unclip_plugin::Result<serde_json::Value> {
        let path = params
            .get("file")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&source.0);
        let text = unclip_io::read_text_file(std::path::Path::new(path), "inference input")
            .map_err(|error| unclip_plugin::PluginError::Message(error.to_string()))?;
        serde_norway::from_str(&text)
            .map_err(|error| unclip_plugin::PluginError::Message(error.to_string()))
    }
}

pub(crate) async fn observe(
    repositories: &crate::db::Repos,
    source: &std::path::Path,
    profile_path: &std::path::Path,
) -> anyhow::Result<()> {
    let document = unclip_io::load_engine_profile(profile_path)?;
    let domain_selector = document
        .domain
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("engine profile must select domain@version"))?;
    let (domain_id, domain_version) = parse_domain_selector(domain_selector)?;
    let domain = unclip_store::DomainReader::get_domain_version(
        &repositories.domains,
        &domain_id,
        &domain_version,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("domain version not found: {domain_selector}"))?;
    let parsed = document.resolve()?;
    let engine = unclip_engine::Engine::with_builtins()?;
    let plan = engine.plan(&parsed.profile)?;
    let source = source
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("inference source path must be valid UTF-8"))?;
    let timestamp = unclip_store::now();
    let run_id = format!("observe-{timestamp}");
    let record = engine.run_record(
        &plan,
        &parsed.params,
        &run_id,
        unclip_epistemic::Timestamp::new(timestamp.clone()),
        serde_json::json!({
            "source": source,
            "profile": profile_path,
            "domain": domain_selector,
            "frame": document.frame,
        }),
    );
    unclip_store::EngineRunRepository::insert_run(&repositories.engine_runs, record).await?;
    unclip_store::EngineRunRepository::transition_run(
        &repositories.engine_runs,
        &run_id,
        unclip_store::EngineRunStatus::Running,
        None,
    )
    .await?;
    let results = engine
        .infer(
            &plan,
            &domain,
            unclip_engine::InferenceRun {
                id: &run_id,
                source: unclip_epistemic::SourceRef::new(source),
                timestamp: unclip_epistemic::Timestamp::new(timestamp),
                params: &parsed.params,
                io: &FileInferenceIo,
            },
        )
        .await?;

    persist_inference(repositories, &run_id, &domain_id, &domain_version, &results).await?;
    unclip_store::EngineRunRepository::transition_run(
        &repositories.engine_runs,
        &run_id,
        unclip_store::EngineRunStatus::Completed,
        Some(unclip_store::now()),
    )
    .await?;

    crate::output::outln!("RUN\t{run_id}");
    for output in &results.outputs {
        let provenance = output.provenance();
        let (observations, alignments, rankings) = match output.value() {
            unclip_plugin::InferenceOutput::Bundle {
                observations,
                alignments,
                rankings,
            } => (observations.len(), alignments.len(), rankings.len()),
            unclip_plugin::InferenceOutput::Observations(values) => (values.len(), 0, 0),
            unclip_plugin::InferenceOutput::Alignments(values) => (0, values.len(), 0),
            unclip_plugin::InferenceOutput::Rankings(values) => (0, 0, values.len()),
            unclip_plugin::InferenceOutput::Structured(_) => (0, 0, 0),
        };
        crate::output::outln!(
            "INFERRED\t{}@{}\tobservations={} alignments={} rankings={}",
            provenance.producer,
            provenance.version,
            observations,
            alignments,
            rankings
        );
    }
    Ok(())
}

fn resolved_profile(
    value: &serde_json::Value,
) -> anyhow::Result<(
    unclip_plugin::EngineProfile,
    std::collections::BTreeMap<unclip_epistemic::PluginId, serde_json::Value>,
)> {
    fn section(
        value: &serde_json::Value,
        name: &str,
        params: &mut std::collections::BTreeMap<unclip_epistemic::PluginId, serde_json::Value>,
    ) -> anyhow::Result<Vec<unclip_plugin::PluginSelection>> {
        value
            .get(name)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("stored run plan has no {name} array"))?
            .iter()
            .map(|entry| {
                let id = entry
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("stored {name} entry has no id"))?;
                let version = entry
                    .get("version")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("stored {name} entry has no version"))?;
                let id = unclip_epistemic::PluginId::new(id);
                params.insert(
                    id.clone(),
                    entry
                        .get("params")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                );
                Ok(unclip_plugin::PluginSelection {
                    id,
                    version: format!("={version}").parse()?,
                })
            })
            .collect()
    }

    let mut params = std::collections::BTreeMap::new();
    let inferrers = section(value, "inferrers", &mut params)?;
    let sensors = section(value, "sensors", &mut params)?;
    let comparators = section(value, "comparators", &mut params)?;
    Ok((
        unclip_plugin::EngineProfile {
            sensors,
            inferrers,
            comparators,
        },
        params,
    ))
}

pub(crate) async fn verify(repositories: &crate::db::Repos, run_id: &str) -> anyhow::Result<()> {
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
            id: &format!("verify-{run_id}"),
            timestamp: unclip_epistemic::Timestamp::new(unclip_store::now()),
            params: &params,
        },
    )?;
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
                anyhow::anyhow!("provenance not found: {}", id.0)
            } else {
                anyhow::anyhow!("provenance dependency not found: {}", id.0)
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
            value.id.0,
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
) -> anyhow::Result<()> {
    anyhow::ensure!(
        format != unclip_io::Format::Jsonl,
        "JSONL is not supported for measurement profile display"
    );
    let profile = repository
        .get_profile(profile_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("measurement profile not found: {profile_id}"))?;
    crate::output::write_stdout(&unclip_io::render_measurement_profile(&profile, format)?)
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
    .ok_or_else(|| anyhow::anyhow!("provenance not found: {}", observation.provenance.0))?;
    crate::output::outln!(
        "OBSERVATION\tINFERRED\t{}@{}\tid={} units={} relations={}",
        provenance.provenance.producer,
        provenance.provenance.version,
        observation.value.id.0,
        observation.value.units.len(),
        observation.value.relations.len()
    );

    for alignment in alignments {
        let provenance = unclip_store::ProvenanceRepository::get_provenance(
            &repositories.provenance,
            &alignment.provenance,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("provenance not found: {}", alignment.provenance.0))?;
        let mut counts = std::collections::BTreeMap::new();
        for candidate in &alignment.value.candidates {
            *counts.entry(&candidate.observed.0).or_insert(0usize) += 1;
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
        .ok_or_else(|| anyhow::anyhow!("provenance not found: {}", ranking.provenance.0))?;
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

pub(crate) async fn measure(
    repositories: &crate::db::Repos,
    observation_id: &str,
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

    let observation_id = unclip_observe::ObservationId::new(observation_id);
    let observation = unclip_store::ObservationRepository::get_recorded_observation(
        &repositories.observations,
        &observation_id,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("observation not found: {}", observation_id.0))?;
    let alignment_records = unclip_store::ObservationRepository::alignments_for_observation(
        &repositories.observations,
        &observation_id,
    )
    .await?;
    let ranking_records = unclip_store::ObservationRepository::rankings_for_observation(
        &repositories.observations,
        &observation_id,
    )
    .await?;
    let observations = vec![unclip_epistemic::Tracked::from_recorded(
        observation.provenance,
        observation.value,
    )];
    let alignments = alignment_records
        .into_iter()
        .map(|record| unclip_epistemic::Tracked::from_recorded(record.provenance, record.value))
        .collect::<Vec<_>>();
    let rankings = ranking_records
        .into_iter()
        .map(|record| unclip_epistemic::Tracked::from_recorded(record.provenance, record.value))
        .collect::<Vec<_>>();

    let parsed = document.resolve()?;
    let engine = unclip_engine::Engine::with_builtins()?;
    let plan = engine.plan(&parsed.profile)?;
    anyhow::ensure!(
        !plan.sensors.is_empty(),
        "engine profile must select at least one sensor"
    );
    let timestamp = unclip_store::now();
    let run_id = format!("measure-{timestamp}");
    let profile_id = format!("{run_id}/profile");
    let run_record = engine.run_record(
        &plan,
        &parsed.params,
        &run_id,
        unclip_epistemic::Timestamp::new(timestamp.clone()),
        serde_json::json!({
            "observation": observation_id.0,
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

    let mut records = Vec::with_capacity(calculated.len());
    for value in &calculated {
        let measurement = value.value();
        let descriptor = plan
            .sensors
            .iter()
            .map(|sensor| sensor.descriptor())
            .find(|descriptor| descriptor.id == measurement.sensor)
            .ok_or_else(|| {
                anyhow::anyhow!("resolved sensor disappeared: {}", measurement.sensor.0)
            })?;
        let kind = match &measurement.reading {
            unclip_measure::Reading::Value { value } => value.kind(),
            _ => *descriptor.produces.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "sensor declares no measurement kind: {}",
                    measurement.sensor.0
                )
            })?,
        };
        unclip_store::ProvenanceRepository::insert_provenance(
            &repositories.provenance,
            unclip_store::StoredProvenance {
                id: value.id().clone(),
                run_id: Some(run_id.clone()),
                provenance: value.provenance().clone(),
            },
        )
        .await?;
        let params = parsed
            .params
            .get(&measurement.sensor)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        unclip_store::MeasurementRepository::insert_sensor_run(
            &repositories.measurements,
            unclip_store::SensorRunRecord {
                id: value.id().0.clone(),
                engine_run_id: run_id.clone(),
                sensor: measurement.sensor.clone(),
                sensor_version: measurement.sensor_version.clone(),
                params: params.clone(),
                params_hash: unclip_epistemic::hash_params(&params),
                status: "completed".into(),
                started_at: timestamp.clone(),
                completed_at: Some(unclip_store::now()),
            },
        )
        .await?;
        records.push(unclip_store::MeasurementRecord {
            id: format!("{}/measurement", value.id().0),
            sensor_run_id: value.id().0.clone(),
            provenance: value.id().clone(),
            kind,
            measurement: measurement.clone(),
        });
        crate::output::outln!(
            "MEASUREMENT\tCALCULATED\t{}@{}\t{}",
            measurement.sensor,
            measurement.sensor_version,
            serde_json::to_string(&measurement.reading)?
        );
    }
    let profile_provenance = calculated
        .first()
        .expect("non-empty sensor plan emits one result per sensor")
        .id()
        .clone();
    unclip_store::MeasurementRepository::insert_profile(
        &repositories.measurements,
        unclip_store::MeasurementProfileHeader {
            id: profile_id.clone(),
            engine_run_id: run_id.clone(),
            observation_id: Some(observation_id.0),
            frame: frame_id,
            frame_version,
            provenance: profile_provenance,
            created_at: unclip_store::now(),
        },
        records,
    )
    .await?;
    unclip_store::EngineRunRepository::transition_run(
        &repositories.engine_runs,
        &run_id,
        unclip_store::EngineRunStatus::Completed,
        Some(unclip_store::now()),
    )
    .await?;
    crate::output::outln!("PROFILE\tCALCULATED\t{profile_id}");
    Ok(())
}

async fn persist_inference(
    repositories: &crate::db::Repos,
    run_id: &str,
    domain_id: &unclip_domain::DomainId,
    domain_version: &unclip_epistemic::DomainVersion,
    results: &unclip_engine::InferenceResults,
) -> anyhow::Result<()> {
    use std::collections::BTreeMap;
    let mut observations = BTreeMap::new();
    let mut alignments = Vec::new();
    let mut rankings = Vec::new();
    for output in &results.outputs {
        let provenance_id = output.id().clone();
        unclip_store::ProvenanceRepository::insert_provenance(
            &repositories.provenance,
            unclip_store::StoredProvenance {
                id: provenance_id.clone(),
                run_id: Some(run_id.to_owned()),
                provenance: output.provenance().clone(),
            },
        )
        .await?;
        let (obs, aligns, ranks) = match output.value() {
            unclip_plugin::InferenceOutput::Bundle {
                observations,
                alignments,
                rankings,
            } => (
                observations.as_slice(),
                alignments.as_slice(),
                rankings.as_slice(),
            ),
            unclip_plugin::InferenceOutput::Observations(v) => (
                v.as_slice(),
                &[] as &[unclip_observe::Alignment],
                &[] as &[unclip_observe::PartialRanking],
            ),
            unclip_plugin::InferenceOutput::Alignments(v) => (
                &[] as &[unclip_observe::Observation],
                v.as_slice(),
                &[] as &[unclip_observe::PartialRanking],
            ),
            unclip_plugin::InferenceOutput::Rankings(v) => (
                &[] as &[unclip_observe::Observation],
                &[] as &[unclip_observe::Alignment],
                v.as_slice(),
            ),
            unclip_plugin::InferenceOutput::Structured(_) => (
                &[] as &[unclip_observe::Observation],
                &[] as &[unclip_observe::Alignment],
                &[] as &[unclip_observe::PartialRanking],
            ),
        };
        for observation in obs {
            observations.insert(
                observation.id.0.clone(),
                (observation.clone(), provenance_id.clone()),
            );
        }
        alignments.extend(aligns.iter().cloned().map(|v| (v, provenance_id.clone())));
        rankings.extend(ranks.iter().cloned().map(|v| (v, provenance_id.clone())));
    }
    for (_, (observation, provenance)) in observations {
        unclip_store::ObservationRepository::insert_observation(
            &repositories.observations,
            observation,
            &provenance,
        )
        .await?;
    }
    for (index, (alignment, provenance)) in alignments.into_iter().enumerate() {
        let id = format!("{run_id}/alignment/{index}");
        unclip_store::ObservationRepository::insert_alignment(
            &repositories.observations,
            &id,
            alignment,
            domain_id,
            domain_version,
            &provenance,
        )
        .await?;
    }
    for (index, (ranking, provenance)) in rankings.into_iter().enumerate() {
        let id = format!("{run_id}/ranking/{index}");
        unclip_store::ObservationRepository::insert_ranking(
            &repositories.observations,
            &id,
            ranking,
            &provenance,
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_selector_requires_both_immutable_parts() {
        let (domain, version) = parse_domain_selector("coffee@7").unwrap();
        assert_eq!(domain.0, "coffee");
        assert_eq!(version.0, "7");
        assert!(parse_domain_selector("coffee").is_err());
        assert!(parse_domain_selector("@7").is_err());
        assert!(parse_domain_selector("coffee@").is_err());

        let (frame, frame_version) = parse_frame_selector("coffee.general@2").unwrap();
        assert_eq!(frame.0, "coffee.general");
        assert_eq!(frame_version.0, "2");
        assert!(parse_frame_selector("coffee.general").is_err());
    }
}
