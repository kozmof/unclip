//! Commands for the semantic leveling engine.

pub(crate) mod application;
pub(crate) mod discovery;
pub(crate) mod empirical;
pub(crate) mod experiment;
pub(crate) mod interpretation;

pub(crate) mod catalog;
pub(crate) mod plugin_listing;
pub(crate) mod profile;

pub(crate) use catalog::{
    domain_import, domain_show, frame_import, frame_show, parse_domain_selector,
    parse_frame_selector,
};
pub(crate) use plugin_listing::plugins;
pub(crate) use profile::resolved_profile;
pub(crate) mod inspection;
pub(crate) mod measurement;
pub(crate) use inspection::{explain, profile_show, provenance, verify};
pub(crate) use measurement::measure;

/// Close a `Running` engine run with the outcome of its work.
///
/// `Ok` marks the run `Completed`; `Err` marks it `Failed` and returns the
/// work's error. Every run a command moves to `Running` ends here, so a failure
/// cannot leave it `Running` forever — which `level observe` and
/// `level measure` did, each returning early through `?` before reaching their
/// `Completed` transition.
///
/// If recording `Failed` itself fails, the work's error is still the one
/// returned, with the bookkeeping failure attached as context: the cause the
/// user needs is why the run failed, not why its status could not be written.
pub(crate) async fn finish_run<T>(
    runs: &impl unclip_store::EngineRunRepository,
    run_id: &str,
    outcome: anyhow::Result<T>,
) -> anyhow::Result<T> {
    use unclip_store::EngineRunStatus;

    match outcome {
        Ok(value) => {
            runs.transition_run(
                run_id,
                EngineRunStatus::Completed,
                Some(unclip_store::now()),
            )
            .await?;
            Ok(value)
        }
        Err(error) => {
            match runs
                .transition_run(run_id, EngineRunStatus::Failed, Some(unclip_store::now()))
                .await
            {
                Ok(()) => Err(error),
                Err(status) => Err(error.context(format!(
                    "run {run_id} also could not be marked failed: {status}"
                ))),
            }
        }
    }
}

/// Mint an engine run id as `{kind}-{timestamp}-{random}`.
///
/// The timestamp keeps ids readable and roughly sortable, but it has only
/// millisecond precision, and `engine_runs.id` is a primary key: two runs
/// started in the same millisecond (a script, a parallel shell) would collide
/// and the second insert would fail. The random suffix makes that negligible.
pub(crate) fn run_id(kind: &str, timestamp: &str) -> String {
    use rand::Rng;
    format!("{kind}-{timestamp}-{:08x}", rand::thread_rng().gen::<u32>())
}

/// Reads a recorded model response from a file instead of calling a provider.
///
/// Inference is the one stage that leaves the process, so the boundary is a
/// file: a run is reproducible from its recorded response.
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
            .unwrap_or(source.as_str());
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
    let run_id = run_id("observe", &timestamp);
    let record = unclip_engine::run_record(
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
    let executed: anyhow::Result<_> = async {
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
        Ok(results)
    }
    .await;
    let results = finish_run(&repositories.engine_runs, &run_id, executed).await?;

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
                provenance: output.shared_provenance(),
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
                &[] as &[std::sync::Arc<unclip_observe::Alignment>],
                &[] as &[std::sync::Arc<unclip_observe::PartialRanking>],
            ),
            unclip_plugin::InferenceOutput::Alignments(v) => (
                &[] as &[std::sync::Arc<unclip_observe::Observation>],
                v.as_slice(),
                &[] as &[std::sync::Arc<unclip_observe::PartialRanking>],
            ),
            unclip_plugin::InferenceOutput::Rankings(v) => (
                &[] as &[std::sync::Arc<unclip_observe::Observation>],
                &[] as &[std::sync::Arc<unclip_observe::Alignment>],
                v.as_slice(),
            ),
            unclip_plugin::InferenceOutput::Structured(_) => (
                &[] as &[std::sync::Arc<unclip_observe::Observation>],
                &[] as &[std::sync::Arc<unclip_observe::Alignment>],
                &[] as &[std::sync::Arc<unclip_observe::PartialRanking>],
            ),
        };
        for observation in obs {
            observations.insert(
                observation.id.to_string(),
                (observation.clone(), provenance_id.clone()),
            );
        }
        alignments.extend(aligns.iter().cloned().map(|v| (v, provenance_id.clone())));
        rankings.extend(ranks.iter().cloned().map(|v| (v, provenance_id.clone())));
    }
    for (_, (observation, provenance)) in observations {
        unclip_store::ObservationRepository::insert_observation(
            &repositories.observations,
            &observation,
            &provenance,
        )
        .await?;
    }
    for (index, (alignment, provenance)) in alignments.into_iter().enumerate() {
        let id = format!("{run_id}/alignment/{index}");
        unclip_store::ObservationRepository::insert_alignment(
            &repositories.observations,
            &id,
            &alignment,
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
            &ranking,
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
        assert_eq!(domain.as_str(), "coffee");
        assert_eq!(version.as_str(), "7");
        assert!(parse_domain_selector("coffee").is_err());
        assert!(parse_domain_selector("@7").is_err());
        assert!(parse_domain_selector("coffee@").is_err());

        let (frame, frame_version) = parse_frame_selector("coffee.general@2").unwrap();
        assert_eq!(frame.as_str(), "coffee.general");
        assert_eq!(frame_version.as_str(), "2");
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
    let key = serde_json::to_string(&(domain_id.as_str(), version.as_str()))?;
    let after = after.map(unclip_epistemic::DerivedId::new);
    let records = repos
        .experiments
        .list_candidates(&key, after.as_ref(), limit)
        .await?;
    crate::output::write_stdout(&format!("{}\n", serde_json::to_string_pretty(&records)?))
}
