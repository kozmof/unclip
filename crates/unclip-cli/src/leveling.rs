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
        serde_json::json!({"source": source, "profile": profile_path}),
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
