//! Promotion of one tested candidate into an immutable domain successor.

use std::collections::BTreeSet;

use anyhow::{ensure, Context};
use serde_json::Value;
use unclip_domain::{DomainId, DomainSnapshot};
use unclip_engine::{ConstraintStatus, CounterfactualEvidence, RelationBindings};
use unclip_epistemic::{
    hash_params, DependencyCollector, DerivedId, EmitMetadata, ExperimentToken, Operation,
    PluginId, Timestamp, Tracked,
};
use unclip_store::{
    CandidateInterpretationRepository, CandidateRepository, DomainReader, DomainRevision,
    DomainRevisionRepository, ExperimentRepository, ProvenanceRepository,
};

fn checked_application_parameter<T: serde::Serialize>(
    provenance: &unclip_epistemic::Provenance,
    name: &str,
    value: &T,
) -> anyhow::Result<()> {
    let recorded = provenance
        .params
        .get(name)
        .with_context(|| format!("tested counterfactual has no {name} parameter"))?;
    ensure!(
        recorded == &serde_json::to_value(value)?,
        "reconstructed candidate application differs from tested {name}"
    );
    Ok(())
}

pub(crate) async fn run(
    repos: &crate::db::Repos,
    candidate_id: &str,
    experiment_id: &str,
    target_domain: &str,
    reason: &str,
    interpretation_ids: &[String],
) -> anyhow::Result<()> {
    ensure!(
        !candidate_id.trim().is_empty(),
        "candidate ID must not be empty"
    );
    ensure!(
        !experiment_id.trim().is_empty(),
        "experiment ID must not be empty"
    );
    ensure!(
        !reason.trim().is_empty(),
        "revision reason must not be empty"
    );

    let candidate_id = DerivedId::new(candidate_id);
    let candidate = repos
        .experiments
        .get_candidate(&candidate_id)
        .await?
        .with_context(|| format!("candidate not found: {candidate_id}"))?;
    let experiment_id = DerivedId::new(experiment_id);
    let experiment = repos
        .experiments
        .get_completed_experiment(&experiment_id)
        .await?
        .with_context(|| format!("completed experiment not found: {experiment_id}"))?;
    ensure!(
        experiment.outcome.candidate_id == candidate_id,
        "completed experiment belongs to another candidate"
    );
    ensure!(
        experiment.outcome.domain_version_id == candidate.proposal.domain_version_id,
        "completed experiment and candidate use different baseline domain versions"
    );

    let (baseline_domain, baseline_version): (String, String) =
        serde_json::from_str(&candidate.proposal.domain_version_id)
            .context("candidate has an invalid baseline domain-version key")?;
    ensure!(
        !baseline_domain.trim().is_empty() && !baseline_version.trim().is_empty(),
        "candidate baseline domain-version key contains an empty component"
    );
    let baseline_domain = DomainId::new(baseline_domain);
    let baseline_version = unclip_epistemic::DomainVersion::new(baseline_version);
    let (target_domain_id, target_version) = super::parse_domain_selector(target_domain)?;
    ensure!(
        target_domain_id == baseline_domain,
        "target domain differs from the candidate baseline domain"
    );
    ensure!(
        target_version != baseline_version,
        "target domain version must differ from the baseline"
    );
    let baseline = repos
        .domains
        .get_domain_version(&baseline_domain, &baseline_version)
        .await?
        .with_context(|| {
            format!(
                "candidate baseline domain version not found: {}@{}",
                baseline_domain.0, baseline_version.0
            )
        })?;

    let evidence: CounterfactualEvidence =
        serde_json::from_value(Value::Object(experiment.outcome.result.clone()))
            .context("completed experiment has incompatible counterfactual evidence")?;
    ensure!(
        evidence.candidate == candidate_id,
        "completed experiment evidence belongs to another candidate"
    );
    ensure!(
        evidence
            .constraints
            .iter()
            .all(|constraint| constraint.status == ConstraintStatus::Satisfied),
        "completed experiment has violated or unavailable explicit constraints"
    );
    let tested_application = repos
        .provenance
        .get_provenance(&evidence.counterfactual)
        .await?
        .context("tested counterfactual provenance not found")?;
    ensure!(
        tested_application.provenance.operation == Operation::Calculated
            && tested_application.provenance.producer
                == PluginId::new("experiment.apply-candidate")
            && tested_application.provenance.inputs.contains(&candidate_id),
        "completed experiment counterfactual is not a tested candidate application"
    );
    ensure!(
        tested_application
            .provenance
            .params
            .get("baseline_domain_version_id")
            .and_then(Value::as_str)
            == Some(candidate.proposal.domain_version_id.as_str()),
        "tested counterfactual uses a different baseline domain version"
    );
    let bindings: Option<RelationBindings> = serde_json::from_value(
        tested_application
            .provenance
            .params
            .get("relation_bindings")
            .context("tested counterfactual has no relation-binding parameter")?
            .clone(),
    )
    .context("tested counterfactual has invalid relation bindings")?;

    let mut interpretations = Vec::with_capacity(interpretation_ids.len());
    let mut unique_interpretations = BTreeSet::new();
    for id in interpretation_ids {
        let id = DerivedId::new(id);
        ensure!(
            unique_interpretations.insert(id.clone()),
            "duplicate interpretation selection: {id}"
        );
        let interpretation = repos
            .experiments
            .get_candidate_interpretation(&id)
            .await?
            .with_context(|| format!("candidate interpretation not found: {id}"))?;
        ensure!(
            interpretation.candidate_id == candidate_id,
            "interpretation {id} belongs to another candidate"
        );
        interpretations.push(id);
    }

    let timestamp = unclip_store::now();
    let revision_id = DerivedId::new(format!(
        "{}/revision/{}@{}",
        experiment.id.0, target_domain_id.0, target_version.0
    ));
    let engine = unclip_engine::Engine::with_builtins()?;
    let application_kind = candidate.proposal.kind;
    let from_version_id = candidate.proposal.domain_version_id.clone();
    let tracked_baseline = Tracked::from_recorded(evidence.baseline.clone(), baseline);
    let tracked_candidate = Tracked::from_recorded(candidate_id.clone(), candidate.proposal);
    let application = engine.apply_candidate_with_relation_bindings(
        &tracked_baseline,
        &tracked_candidate,
        bindings.as_ref(),
        &revision_id.0,
        Timestamp::new(timestamp.clone()),
    )?;
    checked_application_parameter(
        &tested_application.provenance,
        "added_units",
        &application.value().added_units,
    )?;
    checked_application_parameter(
        &tested_application.provenance,
        "added_relations",
        &application.value().added_relations,
    )?;
    checked_application_parameter(
        &tested_application.provenance,
        "property_changes",
        &application.value().property_changes,
    )?;
    checked_application_parameter(
        &tested_application.provenance,
        "application_kind",
        &application_kind,
    )?;

    let mut snapshot: DomainSnapshot = application.value().domain.clone();
    snapshot.version = target_version.clone();
    let target_key = serde_json::to_string(&(&target_domain_id.0, &target_version.0))?;
    let dependencies = DependencyCollector::default();
    dependencies.read(&Tracked::from_recorded(candidate_id.clone(), ()));
    dependencies.read(&Tracked::from_recorded(experiment.id.clone(), ()));
    for id in &interpretations {
        dependencies.read(&Tracked::from_recorded(id.clone(), ()));
    }
    let params = serde_json::json!({
        "candidate":candidate_id,
        "experiment":experiment.id,
        "tested_counterfactual":evidence.counterfactual,
        "from_domain_version":from_version_id,
        "to_domain_version":target_key,
        "interpretations":interpretations,
        "reason":reason,
    });
    let revision = ExperimentToken::from_harness(
        EmitMetadata {
            id: revision_id.clone(),
            producer: PluginId::new("revision.apply"),
            algorithm: "immutable_candidate_promotion".into(),
            version: "0.1.0".parse().expect("valid revision producer version"),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp: Timestamp::new(timestamp),
            domain_version: Some(target_version),
            frame_version: None,
            model: None,
        },
        dependencies,
    )
    .emit(DomainRevision {
        candidate_id,
        experiment_id: experiment.id,
        from_version_id,
        to_version_id: target_key,
        reason: reason.into(),
        evidence: experiment.outcome.result,
        interpretation_ids: interpretations,
    });
    repos
        .experiments
        .apply_domain_revision(Some(experiment.run_id), snapshot, revision)
        .await?;

    crate::output::outln!(
        "APPLIED\tEXPERIMENTAL\t{}\tdomain={target_domain}",
        revision_id
    );
    Ok(())
}
