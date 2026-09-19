//! Persistence for sensor runs and typed, sparse measurement profiles.

use anyhow::Context;
use async_trait::async_trait;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use unclip_domain::FrameId;
use unclip_entity::{
    empirical_structures, frame_versions, measurement_profiles, measurements, sensor_runs,
};
use unclip_epistemic::{Calculated, DerivedId, FrameVersion, ParameterHash, PluginId};
use unclip_measure::{
    EmpiricalStructure, Measurement, MeasurementContext, MeasurementKind, MeasurementProfile,
    MeasurementValue, Reading,
};

use crate::provenance_repository::insert_provenance_in_transaction;
use crate::{StoreError, StoreResult, StoredProvenance};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorRunRecord {
    pub id: String,
    pub engine_run_id: String,
    pub sensor: PluginId,
    pub sensor_version: semver::Version,
    pub params: serde_json::Value,
    pub params_hash: ParameterHash,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementRecord {
    pub id: String,
    pub sensor_run_id: String,
    pub provenance: DerivedId,
    /// Required for sparse readings, whose value cannot communicate a kind.
    pub kind: MeasurementKind,
    pub measurement: Measurement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasurementProfileHeader {
    pub id: String,
    pub engine_run_id: String,
    pub observation_id: Option<String>,
    pub frame: FrameId,
    pub frame_version: FrameVersion,
    pub provenance: DerivedId,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmpiricalStructureRecord {
    pub id: String,
    pub profile_id: Option<String>,
    pub provenance: DerivedId,
    pub created_at: String,
    pub structure: EmpiricalStructure,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredContext {
    values: MeasurementContext,
    sparse_reading: Option<Reading>,
}

#[async_trait]
pub trait MeasurementRepository: Sync {
    async fn insert_sensor_run(&self, run: SensorRunRecord) -> StoreResult<()>;
    async fn insert_profile(
        &self,
        header: MeasurementProfileHeader,
        measurements: Vec<MeasurementRecord>,
    ) -> StoreResult<()>;
    async fn get_profile(&self, id: &str) -> StoreResult<Option<MeasurementProfile>>;
    /// Hydrate measurements with their stored provenance identities.
    async fn get_profile_records(&self, id: &str) -> StoreResult<Option<Vec<MeasurementRecord>>>;
    async fn insert_empirical_structure(&self, record: EmpiricalStructureRecord)
        -> StoreResult<()>;
    /// Atomically persist calculated empirical structure, provenance, and input
    /// edges. At least one existing evidence input is required. Structure ID
    /// and creation timestamp come from the calculated value's provenance.
    async fn insert_calculated_structure(
        &self,
        run_id: Option<String>,
        profile_id: Option<String>,
        structure: Calculated<EmpiricalStructure>,
    ) -> StoreResult<()>;
    async fn get_empirical_structure(
        &self,
        id: &str,
    ) -> StoreResult<Option<EmpiricalStructureRecord>>;
}

pub struct SeaOrmMeasurementRepository {
    db: DatabaseConnection,
}

impl SeaOrmMeasurementRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::InvalidRequest {
        message: message.into(),
    }
}

fn kind_name(kind: MeasurementKind) -> &'static str {
    match kind {
        MeasurementKind::Scalar => "scalar",
        MeasurementKind::Vector => "vector",
        MeasurementKind::Matrix => "matrix",
        MeasurementKind::Distribution => "distribution",
        MeasurementKind::Events => "events",
        MeasurementKind::Graph => "graph",
        MeasurementKind::Ranking => "ranking",
        MeasurementKind::Partition => "partition",
        MeasurementKind::Structured => "structured",
    }
}

fn parse_kind(kind: &str) -> StoreResult<MeasurementKind> {
    match kind {
        "scalar" => Ok(MeasurementKind::Scalar),
        "vector" => Ok(MeasurementKind::Vector),
        "matrix" => Ok(MeasurementKind::Matrix),
        "distribution" => Ok(MeasurementKind::Distribution),
        "events" => Ok(MeasurementKind::Events),
        "graph" => Ok(MeasurementKind::Graph),
        "ranking" => Ok(MeasurementKind::Ranking),
        "partition" => Ok(MeasurementKind::Partition),
        "structured" => Ok(MeasurementKind::Structured),
        other => Err(invalid(format!("unknown stored measurement kind: {other}"))),
    }
}

fn status_name(reading: &Reading) -> &'static str {
    match reading {
        Reading::Value { .. } => "value",
        Reading::NotApplicable { .. } => "not_applicable",
        Reading::NotMeasured => "not_measured",
        Reading::InsufficientEvidence { .. } => "insufficient_evidence",
    }
}

fn validate_measurement(record: &MeasurementRecord) -> StoreResult<()> {
    if let Reading::Value { value } = &record.measurement.reading {
        if value.kind() != record.kind {
            return Err(invalid(
                "declared measurement kind does not match its value",
            ));
        }
    }
    if record
        .measurement
        .confidence
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(invalid(
            "measurement confidence must be finite and between zero and one",
        ));
    }
    validate_value(&record.measurement.reading)
}

fn validate_value(reading: &Reading) -> StoreResult<()> {
    fn finite(values: impl Iterator<Item = f64>) -> bool {
        values.into_iter().all(f64::is_finite)
    }
    let valid = match reading {
        Reading::Value {
            value: MeasurementValue::Scalar(value),
        } => value.is_finite(),
        Reading::Value {
            value: MeasurementValue::Vector(values),
        } => finite(values.iter().copied()),
        Reading::Value {
            value: MeasurementValue::Matrix(rows),
        } => finite(rows.iter().flatten().copied()),
        Reading::Value {
            value: MeasurementValue::Distribution(values),
        } => finite(values.iter().map(|(_, value)| *value)),
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid("numeric measurement values must be finite"))
    }
}

async fn insert_measurement(
    txn: &DatabaseTransaction,
    profile_id: &str,
    record: MeasurementRecord,
) -> StoreResult<()> {
    validate_measurement(&record)?;
    let sensor_run = sensor_runs::Entity::find_by_id(&record.sensor_run_id)
        .one(txn)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            path: format!("sensor run {}", record.sensor_run_id),
        })?;
    if sensor_run.sensor_id != record.measurement.sensor.0
        || sensor_run.sensor_version != record.measurement.sensor_version.to_string()
    {
        return Err(invalid(
            "measurement sensor and version do not match its sensor run",
        ));
    }
    let Measurement {
        sensor: _,
        sensor_version: _,
        reading,
        confidence,
        sample_count,
        context,
    } = record.measurement;
    let status = status_name(&reading).to_string();
    let value_json = match &reading {
        Reading::Value { value } => {
            Some(serde_json::to_string(value).map_err(anyhow::Error::from)?)
        }
        _ => None,
    };
    let sparse_reading = if matches!(reading, Reading::Value { .. }) {
        None
    } else {
        Some(reading)
    };
    let context_json = serde_json::to_string(&StoredContext {
        values: context,
        sparse_reading,
    })
    .map_err(anyhow::Error::from)?;
    let sample_count = sample_count
        .map(i64::try_from)
        .transpose()
        .context("measurement sample count exceeds SQLite INTEGER range")?;

    measurements::Entity::insert(measurements::ActiveModel {
        id: Set(record.id),
        profile_id: Set(profile_id.into()),
        sensor_run_id: Set(record.sensor_run_id),
        provenance_id: Set(record.provenance.0),
        kind: Set(kind_name(record.kind).into()),
        status: Set(status),
        value_json: Set(value_json),
        confidence: Set(confidence),
        sample_count: Set(sample_count),
        context_json: Set(context_json),
    })
    .exec(txn)
    .await?;
    Ok(())
}

#[async_trait]
impl MeasurementRepository for SeaOrmMeasurementRepository {
    async fn insert_sensor_run(&self, run: SensorRunRecord) -> StoreResult<()> {
        if sensor_runs::Entity::find_by_id(&run.id)
            .one(&self.db)
            .await?
            .is_some()
        {
            return Err(StoreError::AlreadyExists { path: run.id });
        }
        sensor_runs::Entity::insert(sensor_runs::ActiveModel {
            id: Set(run.id),
            engine_run_id: Set(run.engine_run_id),
            sensor_id: Set(run.sensor.0),
            sensor_version: Set(run.sensor_version.to_string()),
            params_json: Set(serde_json::to_string(&run.params).map_err(anyhow::Error::from)?),
            params_hash: Set(run.params_hash.0),
            status: Set(run.status),
            started_at: Set(run.started_at),
            completed_at: Set(run.completed_at),
        })
        .exec(&self.db)
        .await?;
        Ok(())
    }

    async fn insert_profile(
        &self,
        header: MeasurementProfileHeader,
        records: Vec<MeasurementRecord>,
    ) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        if measurement_profiles::Entity::find_by_id(&header.id)
            .one(&txn)
            .await?
            .is_some()
        {
            return Err(StoreError::AlreadyExists { path: header.id });
        }
        let frame_version_id = frame_versions::Entity::find()
            .filter(frame_versions::Column::FrameId.eq(&header.frame.0))
            .filter(frame_versions::Column::Version.eq(&header.frame_version.0))
            .one(&txn)
            .await?
            .ok_or_else(|| StoreError::NotFound {
                path: format!(
                    "frame {} version {}",
                    header.frame.0, header.frame_version.0
                ),
            })?
            .id;
        measurement_profiles::Entity::insert(measurement_profiles::ActiveModel {
            id: Set(header.id.clone()),
            engine_run_id: Set(header.engine_run_id),
            observation_id: Set(header.observation_id),
            frame_version_id: Set(frame_version_id),
            provenance_id: Set(header.provenance.0),
            created_at: Set(header.created_at),
        })
        .exec(&txn)
        .await?;
        for record in records {
            insert_measurement(&txn, &header.id, record).await?;
        }
        txn.commit().await?;
        Ok(())
    }

    async fn get_profile(&self, id: &str) -> StoreResult<Option<MeasurementProfile>> {
        Ok(self
            .get_profile_records(id)
            .await?
            .map(|records| MeasurementProfile {
                measurements: records
                    .into_iter()
                    .map(|record| record.measurement)
                    .collect(),
            }))
    }

    async fn get_profile_records(&self, id: &str) -> StoreResult<Option<Vec<MeasurementRecord>>> {
        if measurement_profiles::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let rows = measurements::Entity::find()
            .filter(measurements::Column::ProfileId.eq(id))
            .order_by_asc(measurements::Column::Id)
            .all(&self.db)
            .await?;
        let mut hydrated = Vec::with_capacity(rows.len());
        for row in rows {
            let kind = parse_kind(&row.kind)?;
            let stored: StoredContext = serde_json::from_str(&row.context_json)
                .context("invalid stored measurement context")?;
            let reading = if row.status == "value" {
                let value: MeasurementValue = serde_json::from_str(
                    row.value_json
                        .as_deref()
                        .ok_or_else(|| invalid("stored value reading has no value"))?,
                )
                .context("invalid stored measurement value")?;
                if value.kind() != kind {
                    return Err(invalid("stored measurement kind does not match its value"));
                }
                Reading::Value { value }
            } else {
                let reading = stored
                    .sparse_reading
                    .ok_or_else(|| invalid("stored sparse reading has no details"))?;
                if status_name(&reading) != row.status {
                    return Err(invalid("stored sparse reading status is inconsistent"));
                }
                reading
            };
            let run = sensor_runs::Entity::find_by_id(&row.sensor_run_id)
                .one(&self.db)
                .await?
                .ok_or_else(|| invalid("measurement refers to a missing sensor run"))?;
            hydrated.push(MeasurementRecord {
                id: row.id,
                sensor_run_id: row.sensor_run_id,
                provenance: DerivedId::new(row.provenance_id),
                kind,
                measurement: Measurement {
                    sensor: PluginId::new(run.sensor_id),
                    sensor_version: semver::Version::parse(&run.sensor_version)
                        .context("invalid stored sensor version")?,
                    reading,
                    confidence: row.confidence,
                    sample_count: row
                        .sample_count
                        .map(usize::try_from)
                        .transpose()
                        .context("negative stored sample count")?,
                    context: stored.values,
                },
            });
        }
        Ok(Some(hydrated))
    }
    async fn insert_empirical_structure(
        &self,
        record: EmpiricalStructureRecord,
    ) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        insert_structure_in_transaction(&txn, record).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn insert_calculated_structure(
        &self,
        run_id: Option<String>,
        profile_id: Option<String>,
        structure: Calculated<EmpiricalStructure>,
    ) -> StoreResult<()> {
        if structure.provenance().inputs.is_empty() {
            return Err(invalid(
                "calculated empirical structures require recorded evidence inputs",
            ));
        }
        let provenance = StoredProvenance {
            id: structure.id().clone(),
            run_id,
            provenance: structure.provenance().clone(),
        };
        let record = EmpiricalStructureRecord {
            id: structure.id().0.clone(),
            profile_id,
            provenance: structure.id().clone(),
            created_at: structure.provenance().timestamp.0.clone(),
            structure: structure.into_value(),
        };
        let txn = self.db.begin().await?;
        insert_provenance_in_transaction(&txn, provenance).await?;
        insert_structure_in_transaction(&txn, record).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn get_empirical_structure(
        &self,
        id: &str,
    ) -> StoreResult<Option<EmpiricalStructureRecord>> {
        empirical_structures::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .map(|row| {
                Ok(EmpiricalStructureRecord {
                    id: row.id,
                    profile_id: row.profile_id,
                    provenance: DerivedId::new(row.provenance_id),
                    created_at: row.created_at,
                    structure: EmpiricalStructure {
                        kind: row.kind,
                        value: serde_json::from_str(&row.value_json)
                            .context("invalid stored empirical structure")?,
                    },
                })
            })
            .transpose()
    }
}

async fn insert_structure_in_transaction(
    txn: &DatabaseTransaction,
    record: EmpiricalStructureRecord,
) -> StoreResult<()> {
    if record.id.is_empty() {
        return Err(invalid("empirical structure id must not be empty"));
    }
    if record.structure.kind.is_empty() {
        return Err(invalid("empirical structure kind must not be empty"));
    }
    if empirical_structures::Entity::find_by_id(&record.id)
        .one(txn)
        .await?
        .is_some()
    {
        return Err(StoreError::AlreadyExists { path: record.id });
    }
    empirical_structures::Entity::insert(empirical_structures::ActiveModel {
        id: Set(record.id),
        profile_id: Set(record.profile_id),
        kind: Set(record.structure.kind),
        value_json: Set(
            serde_json::to_string(&record.structure.value).map_err(anyhow::Error::from)?
        ),
        provenance_id: Set(record.provenance.0),
        created_at: Set(record.created_at),
    })
    .exec(txn)
    .await?;
    Ok(())
}
