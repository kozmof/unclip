//! Persistence for sensor runs and typed, sparse measurement profiles.

use std::sync::Arc;

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
use unclip_epistemic::{Calculated, DerivedId, FrameVersion, Operation, PluginId, Tracked};
use unclip_measure::{
    EmpiricalStructure, Measurement, MeasurementContext, MeasurementKind, MeasurementProfile,
    MeasurementValue, Reading,
};

use crate::provenance_repository::insert_provenance_in_transaction;
use crate::{StoreError, StoreResult, StoredProvenance};
pub use unclip_record::SensorRunRecord;

/// One stored measurement with the row identity and provenance it was read at.
///
/// The measurement is shared rather than owned inline, for the same reason a
/// [`RecordedInference`] shares its payload: a caller that keeps the record —
/// to write it into a run snapshot, or to report it — almost always tracks the
/// same measurement as engine evidence at the same time, and the two should
/// not cost two copies of the reading and its context.
///
/// [`RecordedInference`]: unclip_record::RecordedInference
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementRecord {
    pub id: String,
    pub sensor_run_id: String,
    pub provenance: DerivedId,
    /// Required for sparse readings, whose value cannot communicate a kind.
    pub kind: MeasurementKind,
    pub measurement: Arc<Measurement>,
}

impl MeasurementRecord {
    /// Track this record's measurement as calculated evidence, without copying
    /// it. Only calculation writes the `measurements` table, so the label is
    /// the row's own, not a claim the caller invents.
    pub fn tracked(&self) -> Tracked<Measurement> {
        Tracked::from_shared(
            self.provenance.clone(),
            Arc::clone(&self.measurement),
            Some(Operation::Calculated),
        )
    }

    /// Consume this record into tracked evidence, for a caller that keeps no
    /// copy of the record itself.
    pub fn into_tracked(self) -> Tracked<Measurement> {
        Tracked::from_shared(
            self.provenance,
            self.measurement,
            Some(Operation::Calculated),
        )
    }
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

/// The write side of [`StoredContext`], borrowing what it serializes.
///
/// Both halves of that column live on a shared measurement, so writing one row
/// should not have to own a copy of the context map and the sparse reading
/// just to turn them into JSON. The two shapes must serialize identically; the
/// round-trip test below is what holds them together.
#[derive(Debug, Serialize)]
struct StoredContextRef<'a> {
    values: &'a MeasurementContext,
    sparse_reading: Option<&'a Reading>,
}

#[async_trait]
pub trait MeasurementRepository: Sync {
    async fn insert_sensor_run(&self, run: SensorRunRecord) -> StoreResult<()>;
    async fn insert_profile(
        &self,
        header: MeasurementProfileHeader,
        measurements: Vec<MeasurementRecord>,
    ) -> StoreResult<()>;
    /// Atomically persist one calculated profile: each measurement's
    /// provenance (topologically ordered), the sensor runs that produced
    /// them, and the profile with its measurements.
    ///
    /// Writing these one call at a time left a failure partway through with
    /// provenance and sensor-run rows pointing at a profile that was never
    /// written; here either all of it lands or none of it does.
    async fn insert_calculated_profile(
        &self,
        provenance: Vec<StoredProvenance>,
        sensor_runs: Vec<SensorRunRecord>,
        header: MeasurementProfileHeader,
        measurements: Vec<MeasurementRecord>,
    ) -> StoreResult<()>;
    async fn get_profile(&self, id: &str) -> StoreResult<Option<MeasurementProfile>>;
    /// Hydrate measurements with their stored provenance identities.
    async fn get_profile_records(&self, id: &str) -> StoreResult<Option<Vec<MeasurementRecord>>>;
    /// Store a structure that carries no calculation provenance.
    ///
    /// This is the import and legacy path: it writes the row without a
    /// provenance record, so the result reads back as unverified and a caller
    /// must restore it with [`Tracked::from_recorded`] rather than
    /// [`Tracked::from_calculated`]. Anything this process calculated goes
    /// through [`MeasurementRepository::insert_calculated_structure`], which
    /// requires an emit token and writes the provenance edges with it.
    ///
    /// [`Tracked::from_recorded`]: unclip_epistemic::Tracked::from_recorded
    /// [`Tracked::from_calculated`]: unclip_epistemic::Tracked::from_calculated
    async fn insert_unverified_structure(
        &self,
        record: EmpiricalStructureRecord,
    ) -> StoreResult<()>;
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
    if sensor_run.sensor_id != record.measurement.sensor.as_str()
        || sensor_run.sensor_version != record.measurement.sensor_version.to_string()
    {
        return Err(invalid(
            "measurement sensor and version do not match its sensor run",
        ));
    }
    // Destructured by reference, and still exhaustively: the columns are
    // filled from the shared measurement without copying its reading or
    // context, while a new field on `Measurement` still fails to compile here
    // until it is either stored or explicitly ignored.
    let Measurement {
        sensor: _,
        sensor_version: _,
        reading,
        confidence,
        sample_count,
        context,
    } = &*record.measurement;
    let status = status_name(reading).to_string();
    let value_json = match reading {
        Reading::Value { value } => {
            Some(serde_json::to_string(value).map_err(anyhow::Error::from)?)
        }
        _ => None,
    };
    let sparse_reading = (!matches!(reading, Reading::Value { .. })).then_some(reading);
    let context_json = serde_json::to_string(&StoredContextRef {
        values: context,
        sparse_reading,
    })
    .map_err(anyhow::Error::from)?;
    let confidence = *confidence;
    let sample_count = sample_count
        .map(i64::try_from)
        .transpose()
        .context("measurement sample count exceeds SQLite INTEGER range")?;

    measurements::Entity::insert(measurements::ActiveModel {
        id: Set(record.id),
        profile_id: Set(profile_id.into()),
        sensor_run_id: Set(record.sensor_run_id),
        provenance_id: Set(record.provenance.to_string()),
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

pub(crate) async fn insert_sensor_run_in_transaction(
    txn: &DatabaseTransaction,
    run: SensorRunRecord,
) -> StoreResult<()> {
    if sensor_runs::Entity::find_by_id(&run.id)
        .one(txn)
        .await?
        .is_some()
    {
        return Err(StoreError::AlreadyExists { path: run.id });
    }
    sensor_runs::Entity::insert(sensor_runs::ActiveModel {
        id: Set(run.id),
        engine_run_id: Set(run.engine_run_id),
        sensor_id: Set(run.sensor.to_string()),
        sensor_version: Set(run.sensor_version.to_string()),
        params_json: Set(serde_json::to_string(&run.params).map_err(anyhow::Error::from)?),
        params_hash: Set(run.params_hash.to_string()),
        status: Set(run.status),
        started_at: Set(run.started_at),
        completed_at: Set(run.completed_at),
    })
    .exec(txn)
    .await?;
    Ok(())
}

pub(crate) async fn insert_profile_in_transaction(
    txn: &DatabaseTransaction,
    header: MeasurementProfileHeader,
    records: Vec<MeasurementRecord>,
) -> StoreResult<()> {
    if measurement_profiles::Entity::find_by_id(&header.id)
        .one(txn)
        .await?
        .is_some()
    {
        return Err(StoreError::AlreadyExists { path: header.id });
    }
    let frame_version_id = frame_versions::Entity::find()
        .filter(frame_versions::Column::FrameId.eq(header.frame.as_str()))
        .filter(frame_versions::Column::Version.eq(header.frame_version.as_str()))
        .one(txn)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            path: format!("frame {} version {}", header.frame, header.frame_version),
        })?
        .id;
    measurement_profiles::Entity::insert(measurement_profiles::ActiveModel {
        id: Set(header.id.clone()),
        engine_run_id: Set(header.engine_run_id),
        observation_id: Set(header.observation_id),
        frame_version_id: Set(frame_version_id),
        provenance_id: Set(header.provenance.to_string()),
        created_at: Set(header.created_at),
    })
    .exec(txn)
    .await?;
    for record in records {
        insert_measurement(txn, &header.id, record).await?;
    }
    Ok(())
}

#[async_trait]
impl MeasurementRepository for SeaOrmMeasurementRepository {
    async fn insert_sensor_run(&self, run: SensorRunRecord) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        insert_sensor_run_in_transaction(&txn, run).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn insert_profile(
        &self,
        header: MeasurementProfileHeader,
        records: Vec<MeasurementRecord>,
    ) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        insert_profile_in_transaction(&txn, header, records).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn insert_calculated_profile(
        &self,
        provenance: Vec<StoredProvenance>,
        sensor_runs: Vec<SensorRunRecord>,
        header: MeasurementProfileHeader,
        measurements: Vec<MeasurementRecord>,
    ) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        for value in provenance {
            insert_provenance_in_transaction(&txn, value).await?;
        }
        for run in sensor_runs {
            insert_sensor_run_in_transaction(&txn, run).await?;
        }
        insert_profile_in_transaction(&txn, header, measurements).await?;
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
                    // The records were just hydrated for this call, so each
                    // handle is the only one and nothing is copied here.
                    .map(|record| Arc::unwrap_or_clone(record.measurement))
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
                measurement: Arc::new(Measurement {
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
                }),
            });
        }
        Ok(Some(hydrated))
    }
    async fn insert_unverified_structure(
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
        let provenance = StoredProvenance::of(&structure, run_id);
        let record = EmpiricalStructureRecord {
            id: structure.id().to_string(),
            profile_id,
            provenance: structure.id().clone(),
            created_at: structure.provenance().timestamp.to_string(),
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
        provenance_id: Set(record.provenance.to_string()),
        created_at: Set(record.created_at),
    })
    .exec(txn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `StoredContextRef` exists only so writing a row need not own the
    /// context it serializes. It is the same column as `StoredContext`, so if
    /// the two shapes ever drift, every measurement written after the drift
    /// becomes unreadable. This pins them to one another.
    #[test]
    fn the_borrowed_context_writes_what_the_owned_one_reads() {
        let values = MeasurementContext {
            values: [
                ("axis".to_owned(), serde_json::json!("a")),
                ("samples".to_owned(), serde_json::json!(3)),
            ]
            .into_iter()
            .collect(),
        };
        for sparse_reading in [
            None,
            Some(Reading::InsufficientEvidence { have: 1, need: 4 }),
        ] {
            let borrowed = serde_json::to_string(&StoredContextRef {
                values: &values,
                sparse_reading: sparse_reading.as_ref(),
            })
            .expect("borrowed context serializes");
            let owned = serde_json::to_string(&StoredContext {
                values: values.clone(),
                sparse_reading: sparse_reading.clone(),
            })
            .expect("owned context serializes");
            assert_eq!(borrowed, owned);

            let read: StoredContext =
                serde_json::from_str(&borrowed).expect("a written column reads back");
            assert_eq!(read.values, values);
            assert_eq!(read.sparse_reading, sparse_reading);
        }
    }
}
