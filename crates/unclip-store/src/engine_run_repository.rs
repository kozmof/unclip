//! Engine-run lifecycle persistence and replay lookup.

use async_trait::async_trait;
use sea_orm::{
    sea_query::Expr, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder,
};
use unclip_entity::{
    alignments as alignment_rows, engine_runs, measurement_profiles,
    observations as observation_rows, provenance, rankings as ranking_rows, sensor_runs,
};
use unclip_epistemic::{ParameterHash, PluginId};
pub use unclip_record::{
    EngineRunRecord, EngineRunReplay, EngineRunStatus, MeasurementInputSnapshot, RecordedInference,
};

use crate::{ObservationRepository, SensorRunRecord, StoreError, StoreResult};

/// Read a stored status string, reporting an unrecognized one as a store error.
fn parse_status(value: &str) -> StoreResult<EngineRunStatus> {
    value.parse().map_err(|error| StoreError::InvalidRequest {
        message: format!("{error}"),
    })
}

/// Which status changes the lifecycle permits.
///
/// This is a store rule rather than part of the record type: the ordering exists
/// to keep persisted rows consistent, and a caller holding an in-memory record
/// has nothing to violate.
trait EngineRunTransition {
    fn can_transition_to(self, next: Self) -> bool;
}

impl EngineRunTransition for EngineRunStatus {
    fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Planned, Self::Running)
                | (Self::Planned, Self::Failed)
                | (Self::Running, Self::Completed)
                | (Self::Running, Self::Failed)
        )
    }
}

#[async_trait]
pub trait EngineRunRepository: Sync {
    async fn insert_run(&self, run: EngineRunRecord) -> StoreResult<()>;
    async fn get_run(&self, id: &str) -> StoreResult<Option<EngineRunRecord>>;
    async fn transition_run(
        &self,
        id: &str,
        status: EngineRunStatus,
        completed_at: Option<String>,
    ) -> StoreResult<()>;
    async fn replay_run(&self, id: &str) -> StoreResult<Option<EngineRunReplay>>;
}

pub struct SeaOrmEngineRunRepository {
    db: DatabaseConnection,
}

impl SeaOrmEngineRunRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::InvalidRequest {
        message: message.into(),
    }
}

fn validate_completion(status: EngineRunStatus, completed_at: &Option<String>) -> StoreResult<()> {
    if status == EngineRunStatus::Completed && completed_at.is_none() {
        return Err(invalid("completed engine runs require a completion time"));
    }
    if matches!(status, EngineRunStatus::Planned | EngineRunStatus::Running)
        && completed_at.is_some()
    {
        return Err(invalid(
            "non-terminal engine runs cannot have a completion time",
        ));
    }
    Ok(())
}

fn hydrate_run(row: engine_runs::Model) -> StoreResult<EngineRunRecord> {
    Ok(EngineRunRecord {
        id: row.id,
        resolved_plan: serde_json::from_str(&row.resolved_plan_json)
            .map_err(anyhow::Error::from)?,
        status: parse_status(&row.status)?,
        started_at: row.started_at,
        completed_at: row.completed_at,
        metadata: serde_json::from_str(&row.metadata_json).map_err(anyhow::Error::from)?,
    })
}

fn hydrate_sensor_run(row: sensor_runs::Model) -> StoreResult<SensorRunRecord> {
    Ok(SensorRunRecord {
        id: row.id,
        engine_run_id: row.engine_run_id,
        sensor: PluginId::new(row.sensor_id),
        sensor_version: semver::Version::parse(&row.sensor_version).map_err(anyhow::Error::from)?,
        params: serde_json::from_str(&row.params_json).map_err(anyhow::Error::from)?,
        params_hash: ParameterHash::new(row.params_hash),
        status: row.status,
        started_at: row.started_at,
        completed_at: row.completed_at,
    })
}

#[async_trait]
impl EngineRunRepository for SeaOrmEngineRunRepository {
    async fn insert_run(&self, run: EngineRunRecord) -> StoreResult<()> {
        validate_completion(run.status, &run.completed_at)?;
        if engine_runs::Entity::find_by_id(&run.id)
            .one(&self.db)
            .await?
            .is_some()
        {
            return Err(StoreError::AlreadyExists { path: run.id });
        }
        engine_runs::Entity::insert(engine_runs::ActiveModel {
            id: Set(run.id),
            resolved_plan_json: Set(
                serde_json::to_string(&run.resolved_plan).map_err(anyhow::Error::from)?
            ),
            status: Set(run.status.as_str().into()),
            started_at: Set(run.started_at),
            completed_at: Set(run.completed_at),
            metadata_json: Set(serde_json::to_string(&run.metadata).map_err(anyhow::Error::from)?),
        })
        .exec(&self.db)
        .await?;
        Ok(())
    }

    async fn get_run(&self, id: &str) -> StoreResult<Option<EngineRunRecord>> {
        engine_runs::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .map(hydrate_run)
            .transpose()
    }

    async fn transition_run(
        &self,
        id: &str,
        status: EngineRunStatus,
        completed_at: Option<String>,
    ) -> StoreResult<()> {
        validate_completion(status, &completed_at)?;
        let current = engine_runs::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or_else(|| StoreError::NotFound { path: id.into() })?;
        let current_status = parse_status(&current.status)?;
        if !current_status.can_transition_to(status) {
            return Err(StoreError::Conflict { path: id.into() });
        }
        let result = engine_runs::Entity::update_many()
            .col_expr(engine_runs::Column::Status, Expr::value(status.as_str()))
            .col_expr(engine_runs::Column::CompletedAt, Expr::value(completed_at))
            .filter(engine_runs::Column::Id.eq(id))
            .filter(engine_runs::Column::Status.eq(current_status.as_str()))
            .exec(&self.db)
            .await?;
        if result.rows_affected != 1 {
            return Err(StoreError::Conflict { path: id.into() });
        }
        Ok(())
    }

    async fn replay_run(&self, id: &str) -> StoreResult<Option<EngineRunReplay>> {
        let Some(run) = self.get_run(id).await? else {
            return Ok(None);
        };
        let sensor_runs = sensor_runs::Entity::find()
            .filter(sensor_runs::Column::EngineRunId.eq(id))
            .order_by_asc(sensor_runs::Column::SensorId)
            .order_by_asc(sensor_runs::Column::Id)
            .all(&self.db)
            .await?
            .into_iter()
            .map(hydrate_sensor_run)
            .collect::<StoreResult<Vec<_>>>()?;
        let provenance_ids: Vec<String> = provenance::Entity::find()
            .filter(provenance::Column::RunId.eq(id))
            .order_by_asc(provenance::Column::DerivedId)
            .all(&self.db)
            .await?
            .into_iter()
            .map(|row| row.derived_id)
            .collect();

        let (observations, alignments, rankings) = if let Some(snapshot) =
            run.metadata.get("measurement_inputs")
        {
            let snapshot: MeasurementInputSnapshot = serde_json::from_value(snapshot.clone())
                .map_err(|error| invalid(format!("invalid measurement input snapshot: {error}")))?;
            (
                snapshot.observations,
                snapshot.alignments,
                snapshot.rankings,
            )
        } else {
            let observation_repo = crate::SeaOrmObservationRepository::new(self.db.clone());
            let observation_rows = if provenance_ids.is_empty() {
                Vec::new()
            } else {
                observation_rows::Entity::find()
                    .filter(observation_rows::Column::ProvenanceId.is_in(provenance_ids.clone()))
                    .order_by_asc(observation_rows::Column::Id)
                    .all(&self.db)
                    .await?
            };
            let mut observations = Vec::with_capacity(observation_rows.len());
            for row in observation_rows {
                let value = observation_repo
                    .get_observation(&unclip_observe::ObservationId::new(&row.id))
                    .await?
                    .ok_or_else(|| invalid(format!("missing replay observation: {}", row.id)))?;
                observations.push(RecordedInference {
                    provenance: unclip_epistemic::DerivedId::new(row.provenance_id),
                    value,
                });
            }
            let alignment_rows = if provenance_ids.is_empty() {
                Vec::new()
            } else {
                alignment_rows::Entity::find()
                    .filter(alignment_rows::Column::ProvenanceId.is_in(provenance_ids.clone()))
                    .order_by_asc(alignment_rows::Column::Id)
                    .all(&self.db)
                    .await?
            };
            let mut alignments = Vec::with_capacity(alignment_rows.len());
            for row in alignment_rows {
                let value = observation_repo
                    .get_alignment(&row.id)
                    .await?
                    .ok_or_else(|| invalid(format!("missing replay alignment: {}", row.id)))?;
                alignments.push(RecordedInference {
                    provenance: unclip_epistemic::DerivedId::new(row.provenance_id),
                    value,
                });
            }
            let ranking_rows = if provenance_ids.is_empty() {
                Vec::new()
            } else {
                ranking_rows::Entity::find()
                    .filter(ranking_rows::Column::ProvenanceId.is_in(provenance_ids.clone()))
                    .order_by_asc(ranking_rows::Column::Id)
                    .all(&self.db)
                    .await?
            };
            let mut rankings = Vec::with_capacity(ranking_rows.len());
            for row in ranking_rows {
                let value = observation_repo
                    .get_ranking(&row.id)
                    .await?
                    .ok_or_else(|| invalid(format!("missing replay ranking: {}", row.id)))?;
                rankings.push(RecordedInference {
                    provenance: unclip_epistemic::DerivedId::new(row.provenance_id),
                    value,
                });
            }
            (observations, alignments, rankings)
        };
        let profile_ids = measurement_profiles::Entity::find()
            .filter(measurement_profiles::Column::EngineRunId.eq(id))
            .order_by_asc(measurement_profiles::Column::Id)
            .all(&self.db)
            .await?
            .into_iter()
            .map(|row| row.id)
            .collect();
        Ok(Some(EngineRunReplay {
            run,
            sensor_runs,
            provenance_ids,
            profile_ids,
            observations,
            alignments,
            rankings,
        }))
    }
}
