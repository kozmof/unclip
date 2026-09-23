//! Atomic storage for anonymous proposals and completed counterfactual evidence.

use std::collections::BTreeSet;

use anyhow::Context;
use async_trait::async_trait;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use unclip_domain::DomainSnapshot;
use unclip_entity::{
    candidate_interpretations, candidates, domain_revision_interpretations, domain_revisions,
    domain_versions, experiment_deltas, experiment_observations, experiments, frame_versions,
    measurement_profiles, observations, sensor_runs,
};
use unclip_epistemic::{
    Calculated, DerivedId, Experimental, Interpreted, ParameterHash, PluginId, Provenance,
};
use unclip_measure::{Delta, MeasurementValue};

use crate::{
    measurement_repository::{insert_profile_in_transaction, insert_sensor_run_in_transaction},
    provenance_repository::insert_provenance_in_transaction,
    MeasurementProfileHeader, MeasurementRecord, MeasurementRepository,
    SeaOrmMeasurementRepository, SensorRunRecord, StoreError, StoreResult, StoredProvenance,
};

pub use unclip_domain::{CandidateKind, CandidateProposal};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub id: DerivedId,
    pub created_at: String,
    pub proposal: CandidateProposal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateInterpretationRecord {
    pub id: DerivedId,
    pub candidate_id: DerivedId,
    pub value: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentOutcome {
    pub candidate_id: DerivedId,
    pub domain_version_id: String,
    pub frame_version_id: String,
    pub plan: Map<String, Value>,
    pub result: Map<String, Value>,
    /// Explicit sequence order is preserved independently in each split.
    pub training: Vec<unclip_observe::ObservationId>,
    pub held_out: Vec<unclip_observe::ObservationId>,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExperimentDelta {
    pub before_profile_id: String,
    pub after_profile_id: String,
    pub calculated: Calculated<Delta>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExperimentDeltaRecord {
    pub id: DerivedId,
    pub before_profile_id: String,
    pub after_profile_id: String,
    pub comparator_version: semver::Version,
    pub delta: Delta,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletedExperimentRecord {
    pub id: DerivedId,
    pub run_id: String,
    pub completed_at: String,
    pub outcome: ExperimentOutcome,
    pub deltas: Vec<ExperimentDeltaRecord>,
}

pub struct ExperimentMeasurementProfile {
    pub header: MeasurementProfileHeader,
    pub sensor_runs: Vec<SensorRunRecord>,
    pub measurements: Vec<MeasurementRecord>,
}

pub struct CompletedExperimentBundle {
    /// Must be topologically ordered; candidate and observation provenance is preexisting.
    pub prerequisite_provenance: Vec<StoredProvenance>,
    pub profiles: Vec<ExperimentMeasurementProfile>,
    /// Must be topologically ordered after calculated delta provenance.
    pub post_delta_provenance: Vec<StoredProvenance>,
    pub experiment: Experimental<ExperimentOutcome>,
    pub deltas: Vec<ExperimentDelta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainRevision {
    pub candidate_id: DerivedId,
    pub experiment_id: DerivedId,
    pub from_version_id: String,
    pub to_version_id: String,
    pub reason: String,
    pub evidence: Map<String, Value>,
    #[serde(default)]
    pub interpretation_ids: Vec<DerivedId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DomainRevisionRecord {
    pub id: DerivedId,
    pub created_at: String,
    pub revision: DomainRevision,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RevisionLedgerProfile {
    pub header: MeasurementProfileHeader,
    pub measurements: Vec<MeasurementRecord>,
    pub sensor_runs: Vec<SensorRunRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DomainRevisionLedger {
    pub revision: DomainRevisionRecord,
    pub candidate: CandidateRecord,
    pub experiment: CompletedExperimentRecord,
    pub profiles: Vec<RevisionLedgerProfile>,
    pub interpretations: Vec<CandidateInterpretationRecord>,
    pub provenance: Vec<StoredProvenance>,
}

#[async_trait]
pub trait CandidateRepository: Sync {
    async fn insert_candidate(
        &self,
        run_id: Option<String>,
        candidate: Calculated<CandidateProposal>,
    ) -> StoreResult<()>;
    async fn get_candidate(&self, id: &DerivedId) -> StoreResult<Option<CandidateRecord>>;
    async fn list_candidates(
        &self,
        domain_version_id: &str,
        after: Option<&DerivedId>,
        limit: u64,
    ) -> StoreResult<Vec<CandidateRecord>>;
}

#[async_trait]
pub trait CandidateInterpretationRepository: Sync {
    async fn insert_candidate_interpretation(
        &self,
        run_id: Option<String>,
        candidate_id: &DerivedId,
        interpretation: Interpreted<Value>,
    ) -> StoreResult<()>;
    async fn get_candidate_interpretation(
        &self,
        id: &DerivedId,
    ) -> StoreResult<Option<CandidateInterpretationRecord>>;
}

#[async_trait]
pub trait ExperimentRepository: Sync {
    /// Persist a completed result, ordered disjoint observation splits, deltas,
    /// and all their provenance atomically. No partial result is exposed.
    async fn insert_completed_experiment(
        &self,
        run_id: &str,
        experiment: Experimental<ExperimentOutcome>,
        deltas: Vec<ExperimentDelta>,
    ) -> StoreResult<()>;
    /// Persist prerequisite calculations, profiles, deltas, and the completed result atomically.
    async fn insert_completed_experiment_bundle(
        &self,
        run_id: &str,
        bundle: CompletedExperimentBundle,
    ) -> StoreResult<()>;
    /// Planned/running/failed records are not completed results and are rejected.
    async fn get_completed_experiment(
        &self,
        id: &DerivedId,
    ) -> StoreResult<Option<CompletedExperimentRecord>>;
}

#[async_trait]
pub trait DomainRevisionRepository: Sync {
    /// Record explicit revision evidence for already-existing domain versions.
    /// This never creates, updates, or promotes a semantic-domain version.
    async fn insert_domain_revision(
        &self,
        run_id: Option<String>,
        revision: Experimental<DomainRevision>,
    ) -> StoreResult<()>;
    /// Atomically create the successor domain version and its immutable revision row.
    async fn apply_domain_revision(
        &self,
        run_id: Option<String>,
        snapshot: DomainSnapshot,
        revision: Experimental<DomainRevision>,
    ) -> StoreResult<()>;
    async fn get_domain_revision(
        &self,
        id: &DerivedId,
    ) -> StoreResult<Option<DomainRevisionRecord>>;
    /// Rehydrate every immutable record needed to explain why and how a revision occurred.
    async fn get_domain_revision_ledger(
        &self,
        id: &DerivedId,
    ) -> StoreResult<Option<DomainRevisionLedger>>;
}

pub struct SeaOrmExperimentRepository {
    db: DatabaseConnection,
}
impl SeaOrmExperimentRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}
fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::InvalidRequest {
        message: message.into(),
    }
}
fn json<T: Serialize>(value: &T) -> StoreResult<String> {
    serde_json::to_string(value)
        .context("invalid experiment JSON")
        .map_err(Into::into)
}
fn parse<T: serde::de::DeserializeOwned>(value: &str) -> StoreResult<T> {
    serde_json::from_str(value)
        .context("invalid stored experiment data")
        .map_err(Into::into)
}
fn require_input(provenance: &Provenance, id: &str) -> StoreResult<()> {
    if !provenance.inputs.iter().any(|input| input.0 == id) {
        return Err(invalid(format!(
            "required evidence input is not tracked: {id}"
        )));
    }
    Ok(())
}
async fn provenance(
    txn: &DatabaseTransaction,
    run_id: Option<String>,
    id: &DerivedId,
    value: &Provenance,
) -> StoreResult<()> {
    if value.inputs.is_empty() {
        return Err(invalid(
            "derived experiment records require evidence inputs",
        ));
    }
    insert_provenance_in_transaction(
        txn,
        StoredProvenance {
            id: id.clone(),
            run_id,
            provenance: value.clone(),
        },
    )
    .await
}

#[async_trait]
impl CandidateRepository for SeaOrmExperimentRepository {
    async fn insert_candidate(
        &self,
        run_id: Option<String>,
        candidate: Calculated<CandidateProposal>,
    ) -> StoreResult<()> {
        let value = candidate.value();
        let txn = self.db.begin().await?;
        provenance(&txn, run_id, candidate.id(), candidate.provenance()).await?;
        candidates::Entity::insert(candidates::ActiveModel {
            id: Set(candidate.id().0.clone()),
            domain_version_id: Set(value.domain_version_id.clone()),
            kind: Set(serde_json::to_value(value.kind)
                .context("invalid candidate kind")?
                .as_str()
                .expect("enum string")
                .into()),
            value_json: Set(json(&value.value)?),
            provenance_id: Set(candidate.id().0.clone()),
            created_at: Set(candidate.provenance().timestamp.0.clone()),
        })
        .exec(&txn)
        .await?;
        txn.commit().await?;
        Ok(())
    }
    async fn list_candidates(
        &self,
        domain_version_id: &str,
        after: Option<&DerivedId>,
        limit: u64,
    ) -> StoreResult<Vec<CandidateRecord>> {
        if domain_version_id.is_empty() || !(1..=1000).contains(&limit) {
            return Err(invalid(
                "candidate listing requires a domain and limit between 1 and 1000",
            ));
        }
        let mut query = candidates::Entity::find()
            .filter(candidates::Column::DomainVersionId.eq(domain_version_id));
        if let Some(after) = after {
            query = query.filter(candidates::Column::Id.gt(&after.0));
        }
        query
            .order_by_asc(candidates::Column::Id)
            .limit(limit)
            .all(&self.db)
            .await?
            .into_iter()
            .map(|row| {
                Ok(CandidateRecord {
                    id: DerivedId::new(row.id),
                    created_at: row.created_at,
                    proposal: CandidateProposal {
                        domain_version_id: row.domain_version_id,
                        kind: serde_json::from_value(Value::String(row.kind))
                            .context("invalid stored candidate kind")?,
                        value: parse(&row.value_json)?,
                    },
                })
            })
            .collect()
    }
    async fn get_candidate(&self, id: &DerivedId) -> StoreResult<Option<CandidateRecord>> {
        let Some(row) = candidates::Entity::find_by_id(&id.0).one(&self.db).await? else {
            return Ok(None);
        };
        Ok(Some(CandidateRecord {
            id: DerivedId::new(row.id),
            created_at: row.created_at,
            proposal: CandidateProposal {
                domain_version_id: row.domain_version_id,
                kind: serde_json::from_value(Value::String(row.kind))
                    .context("invalid stored candidate kind")?,
                value: parse(&row.value_json)?,
            },
        }))
    }
}

#[async_trait]
impl CandidateInterpretationRepository for SeaOrmExperimentRepository {
    async fn insert_candidate_interpretation(
        &self,
        run_id: Option<String>,
        candidate_id: &DerivedId,
        interpretation: Interpreted<Value>,
    ) -> StoreResult<()> {
        if !interpretation.value().is_object() {
            return Err(invalid("candidate interpretations must be JSON objects"));
        }
        let txn = self.db.begin().await?;
        if candidates::Entity::find_by_id(&candidate_id.0)
            .one(&txn)
            .await?
            .is_none()
        {
            return Err(invalid("interpretation candidate not found"));
        }
        provenance(
            &txn,
            run_id,
            interpretation.id(),
            interpretation.provenance(),
        )
        .await?;
        candidate_interpretations::Entity::insert(candidate_interpretations::ActiveModel {
            id: Set(interpretation.id().0.clone()),
            candidate_id: Set(candidate_id.0.clone()),
            value_json: Set(json(interpretation.value())?),
            provenance_id: Set(interpretation.id().0.clone()),
            created_at: Set(interpretation.provenance().timestamp.0.clone()),
        })
        .exec(&txn)
        .await?;
        txn.commit().await?;
        Ok(())
    }

    async fn get_candidate_interpretation(
        &self,
        id: &DerivedId,
    ) -> StoreResult<Option<CandidateInterpretationRecord>> {
        candidate_interpretations::Entity::find_by_id(&id.0)
            .one(&self.db)
            .await?
            .map(|row| {
                Ok(CandidateInterpretationRecord {
                    id: DerivedId::new(row.id),
                    candidate_id: DerivedId::new(row.candidate_id),
                    value: parse(&row.value_json)?,
                    created_at: row.created_at,
                })
            })
            .transpose()
    }
}

async fn insert_completed_experiment_in_transaction(
    txn: &DatabaseTransaction,
    run_id: &str,
    experiment: Experimental<ExperimentOutcome>,
    deltas: Vec<ExperimentDelta>,
    post_delta_provenance: Vec<StoredProvenance>,
) -> StoreResult<()> {
    let value = experiment.value();
    if value.held_out.is_empty() || value.started_at.is_empty() {
        return Err(invalid(
            "completed experiments require held-out observations and a start timestamp",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for observation in value.training.iter().chain(&value.held_out) {
        if !seen.insert(observation) {
            return Err(invalid(
                "experiment observation splits must be disjoint and unique",
            ));
        }
    }
    let candidate = candidates::Entity::find_by_id(&value.candidate_id.0)
        .one(txn)
        .await?
        .ok_or_else(|| invalid("experiment candidate not found"))?;
    require_input(experiment.provenance(), &candidate.provenance_id)?;
    for observation in value.training.iter().chain(&value.held_out) {
        let recorded = observations::Entity::find_by_id(&observation.0)
            .one(txn)
            .await?
            .ok_or_else(|| invalid("experiment observation not found"))?;
        require_input(experiment.provenance(), &recorded.provenance_id)?;
    }
    let mut delta_rows = Vec::new();
    for delta in deltas {
        require_input(experiment.provenance(), &delta.calculated.id().0)?;
        delta_rows.push(prepare_delta(txn, run_id, experiment.id(), delta).await?);
    }
    for value in post_delta_provenance {
        insert_provenance_in_transaction(txn, value).await?;
    }
    provenance(
        txn,
        Some(run_id.into()),
        experiment.id(),
        experiment.provenance(),
    )
    .await?;
    experiments::Entity::insert(experiments::ActiveModel {
        id: Set(experiment.id().0.clone()),
        engine_run_id: Set(run_id.into()),
        candidate_id: Set(value.candidate_id.0.clone()),
        domain_version_id: Set(value.domain_version_id.clone()),
        frame_version_id: Set(value.frame_version_id.clone()),
        plan_json: Set(json(&value.plan)?),
        status: Set("planned".into()),
        result_json: Set(None),
        provenance_id: Set(experiment.id().0.clone()),
        started_at: Set(value.started_at.clone()),
        completed_at: Set(None),
    })
    .exec(txn)
    .await?;
    for (split, selected) in [("training", &value.training), ("held_out", &value.held_out)] {
        for (position, observation) in selected.iter().enumerate() {
            experiment_observations::Entity::insert(experiment_observations::ActiveModel {
                experiment_id: Set(experiment.id().0.clone()),
                observation_id: Set(observation.0.clone()),
                split: Set(split.into()),
                position: Set(i64::try_from(position)
                    .map_err(|_| invalid("too many experiment observations"))?),
            })
            .exec(txn)
            .await?;
        }
    }
    for row in delta_rows {
        experiment_deltas::Entity::insert(row).exec(txn).await?;
    }
    experiments::Entity::update_many()
        .col_expr(
            experiments::Column::Status,
            sea_orm::sea_query::Expr::value("running"),
        )
        .filter(experiments::Column::Id.eq(&experiment.id().0))
        .exec(txn)
        .await?;
    experiments::Entity::update_many()
        .col_expr(
            experiments::Column::Status,
            sea_orm::sea_query::Expr::value("completed"),
        )
        .col_expr(
            experiments::Column::ResultJson,
            sea_orm::sea_query::Expr::value(json(&value.result)?),
        )
        .col_expr(
            experiments::Column::CompletedAt,
            sea_orm::sea_query::Expr::value(experiment.provenance().timestamp.0.clone()),
        )
        .filter(experiments::Column::Id.eq(&experiment.id().0))
        .exec(txn)
        .await?;
    Ok(())
}

#[async_trait]
impl ExperimentRepository for SeaOrmExperimentRepository {
    async fn insert_completed_experiment(
        &self,
        run_id: &str,
        experiment: Experimental<ExperimentOutcome>,
        deltas: Vec<ExperimentDelta>,
    ) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        insert_completed_experiment_in_transaction(&txn, run_id, experiment, deltas, vec![])
            .await?;
        txn.commit().await?;
        Ok(())
    }

    async fn insert_completed_experiment_bundle(
        &self,
        run_id: &str,
        bundle: CompletedExperimentBundle,
    ) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        for value in bundle.prerequisite_provenance {
            insert_provenance_in_transaction(&txn, value).await?;
        }
        for profile in bundle.profiles {
            for sensor_run in profile.sensor_runs {
                insert_sensor_run_in_transaction(&txn, sensor_run).await?;
            }
            insert_profile_in_transaction(&txn, profile.header, profile.measurements).await?;
        }
        insert_completed_experiment_in_transaction(
            &txn,
            run_id,
            bundle.experiment,
            bundle.deltas,
            bundle.post_delta_provenance,
        )
        .await?;
        txn.commit().await?;
        Ok(())
    }

    async fn get_completed_experiment(
        &self,
        id: &DerivedId,
    ) -> StoreResult<Option<CompletedExperimentRecord>> {
        let Some(row) = experiments::Entity::find_by_id(&id.0).one(&self.db).await? else {
            return Ok(None);
        };
        if row.status != "completed" {
            return Err(invalid("experiment is not completed"));
        }
        let mut training = Vec::new();
        let mut held_out = Vec::new();
        for observation in experiment_observations::Entity::find()
            .filter(experiment_observations::Column::ExperimentId.eq(&id.0))
            .order_by_asc(experiment_observations::Column::Position)
            .all(&self.db)
            .await?
        {
            match observation.split.as_str() {
                "training" => training.push(unclip_observe::ObservationId::new(
                    observation.observation_id,
                )),
                "held_out" => held_out.push(unclip_observe::ObservationId::new(
                    observation.observation_id,
                )),
                _ => return Err(invalid("invalid stored observation split")),
            }
        }
        let mut deltas = Vec::new();
        for delta in experiment_deltas::Entity::find()
            .filter(experiment_deltas::Column::ExperimentId.eq(&id.0))
            .order_by_asc(experiment_deltas::Column::Id)
            .all(&self.db)
            .await?
        {
            let value: MeasurementValue = parse(&delta.value_json)?;
            if kind_name(&value)? != delta.kind {
                return Err(invalid("stored delta kind does not match its value"));
            }
            deltas.push(ExperimentDeltaRecord {
                id: DerivedId::new(delta.id),
                before_profile_id: delta.before_profile_id,
                after_profile_id: delta.after_profile_id,
                comparator_version: semver::Version::parse(&delta.comparator_version)
                    .context("invalid comparator version")?,
                delta: Delta {
                    comparator: unclip_epistemic::PluginId::new(delta.comparator_id),
                    value,
                },
                created_at: delta.created_at,
            });
        }
        Ok(Some(CompletedExperimentRecord {
            id: DerivedId::new(row.id),
            run_id: row.engine_run_id,
            completed_at: row
                .completed_at
                .ok_or_else(|| invalid("completed experiment has no timestamp"))?,
            outcome: ExperimentOutcome {
                candidate_id: DerivedId::new(row.candidate_id),
                domain_version_id: row.domain_version_id,
                frame_version_id: row.frame_version_id,
                plan: parse(&row.plan_json)?,
                result: parse(
                    &row.result_json
                        .ok_or_else(|| invalid("completed experiment has no result"))?,
                )?,
                training,
                held_out,
                started_at: row.started_at,
            },
            deltas,
        }))
    }
}

fn kind_name(value: &MeasurementValue) -> StoreResult<String> {
    Ok(serde_json::to_value(value.kind())
        .context("invalid delta kind")?
        .as_str()
        .ok_or_else(|| invalid("delta kind is not a string"))?
        .into())
}
async fn insert_revision_in_transaction(
    txn: &DatabaseTransaction,
    run_id: Option<String>,
    revision: &Experimental<DomainRevision>,
) -> StoreResult<()> {
    let value = revision.value();
    if value.reason.trim().is_empty() {
        return Err(invalid("domain revisions require an explicit reason"));
    }
    let mut seen_interpretations = BTreeSet::new();
    for interpretation_id in &value.interpretation_ids {
        if !seen_interpretations.insert(interpretation_id) {
            return Err(invalid("domain revision interpretations must be unique"));
        }
        let interpretation = candidate_interpretations::Entity::find_by_id(&interpretation_id.0)
            .one(txn)
            .await?
            .ok_or_else(|| invalid("domain revision interpretation not found"))?;
        if interpretation.candidate_id != value.candidate_id.0 {
            return Err(invalid(
                "domain revision interpretation belongs to another candidate",
            ));
        }
        require_input(revision.provenance(), &interpretation.provenance_id)?;
    }
    let experiment = experiments::Entity::find_by_id(&value.experiment_id.0)
        .one(txn)
        .await?
        .ok_or_else(|| invalid("revision experiment not found"))?;
    if experiment.status != "completed"
        || experiment.candidate_id != value.candidate_id.0
        || experiment.domain_version_id != value.from_version_id
    {
        return Err(invalid(
            "domain revisions require a matching completed candidate experiment",
        ));
    }
    require_input(revision.provenance(), &experiment.provenance_id)?;
    provenance(txn, run_id, revision.id(), revision.provenance()).await?;
    domain_revisions::Entity::insert(domain_revisions::ActiveModel {
        id: Set(revision.id().0.clone()),
        candidate_id: Set(value.candidate_id.0.clone()),
        experiment_id: Set(value.experiment_id.0.clone()),
        from_version_id: Set(value.from_version_id.clone()),
        to_version_id: Set(value.to_version_id.clone()),
        reason: Set(value.reason.clone()),
        evidence_json: Set(json(&value.evidence)?),
        provenance_id: Set(revision.id().0.clone()),
        created_at: Set(revision.provenance().timestamp.0.clone()),
    })
    .exec(txn)
    .await?;
    for (position, interpretation_id) in value.interpretation_ids.iter().enumerate() {
        domain_revision_interpretations::Entity::insert(
            domain_revision_interpretations::ActiveModel {
                revision_id: Set(revision.id().0.clone()),
                candidate_id: Set(value.candidate_id.0.clone()),
                interpretation_id: Set(interpretation_id.0.clone()),
                position: Set(i64::try_from(position)
                    .map_err(|_| invalid("too many domain revision interpretations"))?),
            },
        )
        .exec(txn)
        .await?;
    }
    Ok(())
}

async fn prepare_delta(
    txn: &DatabaseTransaction,
    run_id: &str,
    experiment: &DerivedId,
    delta: ExperimentDelta,
) -> StoreResult<experiment_deltas::ActiveModel> {
    let calculated = &delta.calculated;
    if calculated.value().comparator != calculated.provenance().producer {
        return Err(invalid(
            "delta comparator must match its provenance producer",
        ));
    }
    for profile_id in [&delta.before_profile_id, &delta.after_profile_id] {
        let profile = measurement_profiles::Entity::find_by_id(profile_id)
            .one(txn)
            .await?
            .ok_or_else(|| invalid("delta measurement profile not found"))?;
        require_input(calculated.provenance(), &profile.provenance_id)?;
    }
    let value_json = json(&calculated.value().value)?;
    let _: MeasurementValue = parse(&value_json)?; // Reject lossy non-finite float serialization.
    provenance(
        txn,
        Some(run_id.into()),
        calculated.id(),
        calculated.provenance(),
    )
    .await?;
    Ok(experiment_deltas::ActiveModel {
        id: Set(calculated.id().0.clone()),
        experiment_id: Set(experiment.0.clone()),
        before_profile_id: Set(delta.before_profile_id),
        after_profile_id: Set(delta.after_profile_id),
        comparator_id: Set(calculated.value().comparator.0.clone()),
        comparator_version: Set(calculated.provenance().version.to_string()),
        kind: Set(kind_name(&calculated.value().value)?),
        value_json: Set(value_json),
        provenance_id: Set(calculated.id().0.clone()),
        created_at: Set(calculated.provenance().timestamp.0.clone()),
    })
}

async fn revision_interpretation_ids(
    db: &DatabaseConnection,
    revision_id: &DerivedId,
) -> StoreResult<Vec<DerivedId>> {
    Ok(domain_revision_interpretations::Entity::find()
        .filter(domain_revision_interpretations::Column::RevisionId.eq(&revision_id.0))
        .order_by_asc(domain_revision_interpretations::Column::Position)
        .all(db)
        .await?
        .into_iter()
        .map(|row| DerivedId::new(row.interpretation_id))
        .collect())
}

fn hydrate_sensor_run(row: sensor_runs::Model) -> StoreResult<SensorRunRecord> {
    Ok(SensorRunRecord {
        id: row.id,
        engine_run_id: row.engine_run_id,
        sensor: PluginId::new(row.sensor_id),
        sensor_version: semver::Version::parse(&row.sensor_version)
            .context("invalid stored sensor version")?,
        params: parse(&row.params_json)?,
        params_hash: ParameterHash::new(row.params_hash),
        status: row.status,
        started_at: row.started_at,
        completed_at: row.completed_at,
    })
}

async fn hydrate_ledger_profile(
    db: &DatabaseConnection,
    id: &str,
) -> StoreResult<RevisionLedgerProfile> {
    let row = measurement_profiles::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| invalid("revision ledger measurement profile not found"))?;
    let frame = frame_versions::Entity::find_by_id(&row.frame_version_id)
        .one(db)
        .await?
        .ok_or_else(|| invalid("revision ledger frame version not found"))?;
    let measurements = MeasurementRepository::get_profile_records(
        &SeaOrmMeasurementRepository::new(db.clone()),
        id,
    )
    .await?
    .ok_or_else(|| invalid("revision ledger measurement profile not found"))?;
    let sensor_ids = measurements
        .iter()
        .map(|record| record.sensor_run_id.clone())
        .collect::<BTreeSet<_>>();
    let mut hydrated_sensor_runs = Vec::with_capacity(sensor_ids.len());
    for sensor_id in sensor_ids {
        let row = sensor_runs::Entity::find_by_id(&sensor_id)
            .one(db)
            .await?
            .ok_or_else(|| invalid("revision ledger sensor run not found"))?;
        hydrated_sensor_runs.push(hydrate_sensor_run(row)?);
    }
    Ok(RevisionLedgerProfile {
        header: MeasurementProfileHeader {
            id: row.id,
            engine_run_id: row.engine_run_id,
            observation_id: row.observation_id,
            frame: unclip_domain::FrameId::new(frame.frame_id),
            frame_version: unclip_epistemic::FrameVersion::new(frame.version),
            provenance: DerivedId::new(row.provenance_id),
            created_at: row.created_at,
        },
        measurements,
        sensor_runs: hydrated_sensor_runs,
    })
}

fn collect_json_strings(value: &Value, strings: &mut BTreeSet<String>) {
    match value {
        Value::String(value) => {
            strings.insert(value.clone());
        }
        Value::Array(values) => {
            for value in values {
                collect_json_strings(value, strings);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_json_strings(value, strings);
            }
        }
        _ => {}
    }
}

async fn hydrate_ledger_provenance(
    db: &DatabaseConnection,
    mut required: BTreeSet<DerivedId>,
    evidence: &[Value],
) -> StoreResult<Vec<StoredProvenance>> {
    let repository = crate::SeaOrmProvenanceRepository::new(db.clone());
    let mut possible = BTreeSet::new();
    for value in evidence {
        collect_json_strings(value, &mut possible);
    }
    for id in possible {
        let id = DerivedId::new(id);
        if crate::ProvenanceRepository::get_provenance(&repository, &id)
            .await?
            .is_some()
        {
            required.insert(id);
        }
    }
    let seeds = required.iter().cloned().collect::<Vec<_>>();
    for id in seeds {
        let ancestors = crate::ProvenanceRepository::ancestors(&repository, &id).await?;
        required.extend(ancestors);
    }
    let mut result = Vec::with_capacity(required.len());
    for id in required {
        result.push(
            crate::ProvenanceRepository::get_provenance(&repository, &id)
                .await?
                .ok_or_else(|| {
                    invalid(format!("revision ledger provenance not found: {}", id.0))
                })?,
        );
    }
    Ok(result)
}

#[async_trait]
impl DomainRevisionRepository for SeaOrmExperimentRepository {
    async fn insert_domain_revision(
        &self,
        run_id: Option<String>,
        revision: Experimental<DomainRevision>,
    ) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        insert_revision_in_transaction(&txn, run_id, &revision).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn apply_domain_revision(
        &self,
        run_id: Option<String>,
        snapshot: DomainSnapshot,
        revision: Experimental<DomainRevision>,
    ) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        let value = revision.value();
        let predecessor = domain_versions::Entity::find_by_id(&value.from_version_id)
            .one(&txn)
            .await?
            .ok_or_else(|| invalid("revision baseline domain version not found"))?;
        let successor_key = crate::domain_repository::version_key(&snapshot.id, &snapshot.version);
        if snapshot.id.0 != predecessor.domain_id
            || snapshot.version.0.trim().is_empty()
            || value.to_version_id != successor_key
            || value.from_version_id == value.to_version_id
        {
            return Err(invalid(
                "revision snapshot must be a distinct successor of the selected baseline",
            ));
        }
        if domain_versions::Entity::find()
            .filter(domain_versions::Column::PredecessorId.eq(&value.from_version_id))
            .one(&txn)
            .await?
            .is_some()
        {
            return Err(StoreError::Conflict {
                path: value.from_version_id.clone(),
            });
        }
        crate::domain_repository::insert_snapshot(
            &txn,
            snapshot,
            Some(value.from_version_id.clone()),
        )
        .await?;
        insert_revision_in_transaction(&txn, run_id, &revision).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn get_domain_revision(
        &self,
        id: &DerivedId,
    ) -> StoreResult<Option<DomainRevisionRecord>> {
        let Some(row) = domain_revisions::Entity::find_by_id(&id.0)
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };
        let id = DerivedId::new(row.id);
        let interpretation_ids = revision_interpretation_ids(&self.db, &id).await?;
        Ok(Some(DomainRevisionRecord {
            id,
            created_at: row.created_at,
            revision: DomainRevision {
                candidate_id: DerivedId::new(row.candidate_id),
                experiment_id: DerivedId::new(row.experiment_id),
                from_version_id: row.from_version_id,
                to_version_id: row.to_version_id,
                reason: row.reason,
                evidence: parse(&row.evidence_json)?,
                interpretation_ids,
            },
        }))
    }

    async fn get_domain_revision_ledger(
        &self,
        id: &DerivedId,
    ) -> StoreResult<Option<DomainRevisionLedger>> {
        let Some(revision) = self.get_domain_revision(id).await? else {
            return Ok(None);
        };
        let candidate = self
            .get_candidate(&revision.revision.candidate_id)
            .await?
            .ok_or_else(|| invalid("revision ledger candidate not found"))?;
        let experiment = self
            .get_completed_experiment(&revision.revision.experiment_id)
            .await?
            .ok_or_else(|| invalid("revision ledger experiment not found"))?;

        let mut profile_ids = BTreeSet::new();
        for delta in &experiment.deltas {
            profile_ids.insert(delta.before_profile_id.clone());
            profile_ids.insert(delta.after_profile_id.clone());
        }
        let mut profiles = Vec::with_capacity(profile_ids.len());
        for profile_id in profile_ids {
            profiles.push(hydrate_ledger_profile(&self.db, &profile_id).await?);
        }

        let mut interpretations = Vec::with_capacity(revision.revision.interpretation_ids.len());
        for interpretation_id in &revision.revision.interpretation_ids {
            let interpretation = self
                .get_candidate_interpretation(interpretation_id)
                .await?
                .ok_or_else(|| invalid("revision ledger interpretation not found"))?;
            if interpretation.candidate_id != candidate.id {
                return Err(invalid(
                    "revision ledger interpretation belongs to another candidate",
                ));
            }
            interpretations.push(interpretation);
        }

        let mut required_provenance = BTreeSet::from([
            revision.id.clone(),
            candidate.id.clone(),
            experiment.id.clone(),
        ]);
        required_provenance.extend(experiment.deltas.iter().map(|delta| delta.id.clone()));
        for profile in &profiles {
            required_provenance.insert(profile.header.provenance.clone());
            required_provenance.extend(
                profile
                    .measurements
                    .iter()
                    .map(|measurement| measurement.provenance.clone()),
            );
        }
        required_provenance.extend(interpretations.iter().map(|value| value.id.clone()));
        let mut evidence = vec![
            Value::Object(revision.revision.evidence.clone()),
            Value::Object(experiment.outcome.plan.clone()),
            Value::Object(experiment.outcome.result.clone()),
        ];
        evidence.extend(interpretations.iter().map(|value| value.value.clone()));
        let provenance =
            hydrate_ledger_provenance(&self.db, required_provenance, &evidence).await?;

        Ok(Some(DomainRevisionLedger {
            revision,
            candidate,
            experiment,
            profiles,
            interpretations,
            provenance,
        }))
    }
}
