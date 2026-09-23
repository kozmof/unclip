//! Explicit semantic interpretation of a stored candidate's empirical source.

use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use unclip_epistemic::{DerivedId, Timestamp, Tracked};
use unclip_store::{
    CandidateInterpretationRepository, CandidateRepository, EngineRunRepository, EngineRunStatus,
    MeasurementRepository, ProvenanceRepository,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseDocument {
    candidate: DerivedId,
    structure: String,
    model: String,
    model_version: String,
    response: Value,
}

struct FileInterpretationIo {
    document: ResponseDocument,
}

#[async_trait::async_trait]
impl unclip_plugin::InterpretationIo for FileInterpretationIo {
    async fn request(
        &self,
        request: &unclip_plugin::InterpretationRequest,
    ) -> unclip_plugin::Result<Value> {
        if self.document.model != request.model
            || self.document.model_version != request.model_version
        {
            return Err(unclip_plugin::PluginError::Message(
                "interpretation response model does not match the resolved interpreter model"
                    .into(),
            ));
        }
        if !self.document.response.is_object() {
            return Err(unclip_plugin::PluginError::Message(
                "interpretation response must be a JSON object".into(),
            ));
        }
        Ok(self.document.response.clone())
    }
}

fn load_response(path: &std::path::Path) -> anyhow::Result<ResponseDocument> {
    let text = unclip_io::read_text_file(path, "interpretation response file")?;
    Ok(serde_norway::from_str(&text)?)
}

pub(crate) async fn run(
    repos: &crate::db::Repos,
    candidate_id: &str,
    structure_id: &str,
    profile_path: &std::path::Path,
    response_path: &std::path::Path,
) -> anyhow::Result<()> {
    ensure!(
        !candidate_id.trim().is_empty(),
        "candidate ID must not be empty"
    );
    ensure!(
        !structure_id.trim().is_empty(),
        "structure ID must not be empty"
    );
    let candidate_id = DerivedId::new(candidate_id);
    let candidate = repos
        .experiments
        .get_candidate(&candidate_id)
        .await?
        .with_context(|| format!("candidate not found: {candidate_id}"))?;
    let structure = repos
        .measurements
        .get_empirical_structure(structure_id)
        .await?
        .with_context(|| format!("empirical structure not found: {structure_id}"))?;

    let document = unclip_io::load_engine_profile(profile_path)?;
    let domain_selector = document
        .domain
        .as_deref()
        .context("interpretation profile must select domain@version")?;
    let (domain_id, domain_version) = super::parse_domain_selector(domain_selector)?;
    let domain_key = serde_json::to_string(&(&domain_id.0, &domain_version.0))?;
    ensure!(
        candidate.proposal.domain_version_id == domain_key,
        "interpretation profile domain does not match candidate domain version"
    );
    let parsed = document.resolve()?;
    ensure!(
        parsed.profile.interpreters.len() == 1,
        "interpretation requires exactly one explicitly selected interpreter"
    );
    ensure!(
        parsed.profile.sensors.is_empty()
            && parsed.profile.inferrers.is_empty()
            && parsed.profile.comparators.is_empty()
            && parsed.profile.candidate_generators.is_empty()
            && parsed.profile.null_models.is_empty(),
        "interpretation executes only an interpreter"
    );
    let ancestry = repos.provenance.ancestors(&candidate_id).await?;
    ensure!(
        ancestry.contains(&structure.provenance),
        "selected structure is not provenance evidence for candidate {candidate_id}"
    );

    let response = load_response(response_path)?;
    ensure!(
        response.candidate == candidate_id,
        "interpretation response candidate does not match the selected candidate"
    );
    ensure!(
        response.structure == structure.id,
        "interpretation response structure does not match the selected structure"
    );

    let engine = unclip_engine::Engine::with_builtins()?;
    let plan = engine.plan(&parsed.profile)?;
    let timestamp = unclip_store::now();
    let run_id = format!("interpret-{timestamp}");
    let run = engine.run_record(
        &plan,
        &parsed.params,
        &run_id,
        Timestamp::new(timestamp.clone()),
        serde_json::json!({
            "stage":"interpretation",
            "candidate":candidate_id,
            "structure":structure.id,
            "profile":profile_path,
            "domain":domain_selector,
            "frame":document.frame,
            "response_file":response_path,
            "response":response,
        }),
    );
    repos.engine_runs.insert_run(run).await?;
    repos
        .engine_runs
        .transition_run(&run_id, EngineRunStatus::Running, None)
        .await?;

    let io = FileInterpretationIo { document: response };
    let source = Tracked::from_recorded(structure.provenance, structure.structure);
    let executed: anyhow::Result<_> = async {
        let mut outputs = engine
            .interpret(
                &plan,
                &[source],
                unclip_engine::InterpretationRun {
                    id: &run_id,
                    timestamp: Timestamp::new(timestamp),
                    params: &parsed.params,
                    io: &io,
                },
            )
            .await?;
        ensure!(
            outputs.len() == 1,
            "one interpreter and source must produce exactly one interpretation"
        );
        let output = outputs.pop().expect("length checked");
        repos
            .experiments
            .insert_candidate_interpretation(Some(run_id.clone()), &candidate_id, output.clone())
            .await?;
        Ok(output)
    }
    .await;
    let output = match executed {
        Ok(output) => output,
        Err(error) => {
            repos
                .engine_runs
                .transition_run(&run_id, EngineRunStatus::Failed, Some(unclip_store::now()))
                .await?;
            return Err(error);
        }
    };
    repos
        .engine_runs
        .transition_run(
            &run_id,
            EngineRunStatus::Completed,
            Some(unclip_store::now()),
        )
        .await?;

    crate::output::outln!("INTERPRETED\tLABEL\trun={run_id}");
    crate::output::outln!(
        "INTERPRETATION\t{}\t{}",
        output.id(),
        serde_json::to_string(output.value())?
    );
    Ok(())
}
