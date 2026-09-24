//! Commands for the semantic leveling engine.

pub(crate) mod application;
pub(crate) mod discovery;
pub(crate) mod empirical;
pub(crate) mod experiment;
pub(crate) mod interpretation;

pub(crate) fn plugins() -> anyhow::Result<()> {
    let registry = unclip_engine::builtin_registry()?;
    let mut found = false;

    for plugin in registry.inferrers() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tINFERRED\t{}\t-\tinference_output",
            descriptor.id,
            descriptor.version
        );
    }
    for plugin in registry.sensors() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tCALCULATED\t{}\t{:?}/{:?}\t{:?}",
            descriptor.id,
            descriptor.version,
            descriptor.applicability,
            descriptor.evidence,
            descriptor.produces
        );
    }
    for plugin in registry.product_sensors() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tCALCULATED\t{}\t{:?}/{:?}\t{:?}",
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
            "{}\tCALCULATED\t{}\t{:?}\tdelta",
            descriptor.id,
            descriptor.version,
            descriptor.supports
        );
    }
    for plugin in registry.interpreters() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tINTERPRETED\t{}\t-\tinterpretation",
            descriptor.id,
            descriptor.version
        );
    }

    for plugin in registry.candidate_generators() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tCALCULATED\t{}\t-\tcandidate",
            descriptor.id,
            descriptor.version
        );
    }
    for plugin in registry.null_models() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tCALCULATED\t{}\t-\tnull_reading",
            descriptor.id,
            descriptor.version
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
    anyhow::ensure!(
        parsed.profile.candidate_generators.is_empty()
            && parsed.profile.null_models.is_empty()
            && parsed.profile.comparators.is_empty()
            && parsed.profile.interpreters.is_empty(),
        "observation and measurement commands do not execute candidate generators, null models, interpreters, or comparators"
    );
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
    let interpreters = if value.get("interpreters").is_some() {
        section(value, "interpreters", &mut params)?
    } else {
        Vec::new()
    };
    let candidate_generators = if value.get("candidate_generators").is_some() {
        section(value, "candidate_generators", &mut params)?
    } else {
        Vec::new()
    };
    let null_models = if value.get("null_models").is_some() {
        section(value, "null_models", &mut params)?
    } else {
        Vec::new()
    };
    Ok((
        unclip_plugin::EngineProfile {
            sensors,
            inferrers,
            comparators,
            interpreters,
            candidate_generators,
            null_models,
        },
        params,
    ))
}

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
            timestamp: unclip_epistemic::Timestamp::new(replay.run.started_at.clone()),
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
            recorded.provenance == *value.provenance(),
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
        .map(|record| {
            unclip_epistemic::Tracked::from_recorded(
                record.provenance.clone(),
                record.value.clone(),
            )
        })
        .collect::<Vec<_>>();
    let alignments = snapshot
        .alignments
        .iter()
        .map(|record| {
            unclip_epistemic::Tracked::from_recorded(
                record.provenance.clone(),
                record.value.clone(),
            )
        })
        .collect::<Vec<_>>();
    let rankings = snapshot
        .rankings
        .iter()
        .map(|record| {
            unclip_epistemic::Tracked::from_recorded(
                record.provenance.clone(),
                record.value.clone(),
            )
        })
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
    let run_id = format!("measure-{timestamp}");
    let profile_id = format!("{run_id}/profile");
    let run_record = engine.run_record(
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
            observation_id: (observation_ids.len() == 1).then(|| observation_ids[0].clone()),
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

#[cfg(test)]
mod discovery_replay_tests {
    use super::resolved_profile;
    #[test]
    fn replay_restores_exact_discovery_selections_and_accepts_legacy_plans() {
        let mut plan = serde_json::json!({"sensors":[],"inferrers":[],"comparators":[]});
        let (legacy, _) = resolved_profile(&plan).unwrap();
        assert!(
            legacy.candidate_generators.is_empty()
                && legacy.null_models.is_empty()
                && legacy.interpreters.is_empty()
        );
        plan["candidate_generators"] =
            serde_json::json!([{"id":"generate.fixture","version":"1.2.3","params":{"count":2}}]);
        plan["null_models"] =
            serde_json::json!([{"id":"null.fixture","version":"1.0.0","params":{"seed":7}}]);
        plan["interpreters"] = serde_json::json!([{"id":"interpret.fixture","version":"2.1.0","params":{"temperature":0}}]);
        let (profile, params) = resolved_profile(&plan).unwrap();
        assert_eq!(
            profile.candidate_generators[0].version.to_string(),
            "=1.2.3"
        );
        assert_eq!(profile.null_models[0].version.to_string(), "=1.0.0");
        assert_eq!(profile.interpreters[0].version.to_string(), "=2.1.0");
        assert_eq!(
            params[&unclip_epistemic::PluginId::new("generate.fixture")]["count"],
            2
        );
        assert_eq!(
            params[&unclip_epistemic::PluginId::new("interpret.fixture")]["temperature"],
            0
        );
        for key in ["candidate_generators", "null_models", "interpreters"] {
            let mut invalid = plan.clone();
            invalid[key] = serde_json::Value::Null;
            assert!(resolved_profile(&invalid).is_err());
        }
    }
}

pub(crate) async fn candidates(
    repos: &crate::db::Repos,
    domain: &str,
    after: Option<&str>,
    limit: u64,
) -> anyhow::Result<()> {
    use unclip_store::{CandidateRepository, DomainReader};
    let (domain_id, version) = parse_domain_selector(domain)?;
    anyhow::ensure!(
        repos
            .domains
            .get_domain_version(&domain_id, &version)
            .await?
            .is_some(),
        "domain version not found: {domain}"
    );
    let key = serde_json::to_string(&(&domain_id.0, &version.0))?;
    let after = after.map(unclip_epistemic::DerivedId::new);
    let records = repos
        .experiments
        .list_candidates(&key, after.as_ref(), limit)
        .await?;
    crate::output::write_stdout(&format!("{}\n", serde_json::to_string_pretty(&records)?))
}
