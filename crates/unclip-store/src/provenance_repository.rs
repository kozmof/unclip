//! Persistence and graph traversal for epistemic provenance.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder, TransactionTrait,
};
use unclip_entity::{provenance, provenance_inputs};
use unclip_epistemic::{
    hash_params, DerivedId, DomainVersion, FrameVersion, ModelRef, Operation, ParameterHash,
    PluginId, Provenance, SourceRef, Timestamp, Tracked,
};

use crate::{StoreError, StoreResult};

/// One provenance record addressed by the identity it was emitted under.
///
/// The record is shared rather than owned: it is built straight off an emitted
/// value — see [`Derived::shared_provenance`] — and its parameter tree can be
/// arbitrarily large, so duplicating it on the way to the database is pure
/// cost. Writing the row reads through the handle and copies only the short
/// columns it has to own.
///
/// [`Derived::shared_provenance`]: unclip_epistemic::Derived::shared_provenance
#[derive(Debug, Clone, PartialEq)]
pub struct StoredProvenance {
    pub id: DerivedId,
    pub run_id: Option<String>,
    pub provenance: Arc<Provenance>,
}

impl StoredProvenance {
    /// Record the provenance an emitted value carries, without copying it.
    pub fn of<T: ?Sized, O: unclip_epistemic::OperationKind>(
        value: &unclip_epistemic::Derived<T, O>,
        run_id: Option<String>,
    ) -> Self {
        Self {
            id: value.id().clone(),
            run_id,
            provenance: value.shared_provenance(),
        }
    }
}

#[async_trait]
pub trait ProvenanceRepository: Sync {
    /// Insert one node and all of its ordered dependency edges atomically.
    async fn insert_provenance(&self, value: StoredProvenance) -> StoreResult<()>;
    async fn get_provenance(&self, id: &DerivedId) -> StoreResult<Option<StoredProvenance>>;
    /// Return immediate inputs in their recorded order.
    async fn direct_inputs(&self, id: &DerivedId) -> StoreResult<Vec<DerivedId>>;
    /// Return every reachable input once, breadth-first and deterministically.
    async fn ancestors(&self, id: &DerivedId) -> StoreResult<Vec<DerivedId>>;

    /// Restore a persisted value under the operation its provenance row records.
    ///
    /// Use this instead of naming an operation wherever the source table admits
    /// rows from more than one path. `empirical_structures` is the case that
    /// matters: `insert_calculated_structure` and `insert_unverified_structure`
    /// both write it, so neither [`Tracked::from_calculated`] nor
    /// [`Tracked::from_recorded`] is right for every row — the first mislabels
    /// an imported structure, the second leaves a calculated-evidence gate with
    /// nothing to check. The provenance row is the record that settles it, and
    /// every structure row carries a `NOT NULL` reference to one.
    ///
    /// A missing provenance row yields an unlabeled value rather than an error:
    /// "no claim was made" is the same state an import leaves behind, and a
    /// read should not fail on it.
    ///
    /// [`Tracked::from_calculated`]: unclip_epistemic::Tracked::from_calculated
    /// [`Tracked::from_recorded`]: unclip_epistemic::Tracked::from_recorded
    async fn restore_tracked<T: Send>(&self, id: DerivedId, value: T) -> StoreResult<Tracked<T>> {
        let operation = self
            .get_provenance(&id)
            .await?
            .map(|stored| stored.provenance.operation);
        Ok(Tracked::from_stored(id, value, operation))
    }
}

pub struct SeaOrmProvenanceRepository {
    db: DatabaseConnection,
}

impl SeaOrmProvenanceRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::InvalidRequest {
        message: message.into(),
    }
}

fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::Inferred => "inferred",
        Operation::Calculated => "calculated",
        Operation::Experimental => "experimental",
        Operation::Interpreted => "interpreted",
    }
}

fn parse_operation(operation: &str) -> StoreResult<Operation> {
    match operation {
        "inferred" => Ok(Operation::Inferred),
        "calculated" => Ok(Operation::Calculated),
        "experimental" => Ok(Operation::Experimental),
        "interpreted" => Ok(Operation::Interpreted),
        other => Err(invalid(format!("unknown stored operation: {other}"))),
    }
}

async fn inputs_in_txn(txn: &DatabaseTransaction, id: &DerivedId) -> StoreResult<Vec<DerivedId>> {
    Ok(provenance_inputs::Entity::find()
        .filter(provenance_inputs::Column::DerivedId.eq(id.as_str()))
        .order_by_asc(provenance_inputs::Column::Position)
        .all(txn)
        .await?
        .into_iter()
        .map(|row| DerivedId::new(row.input_derived_id))
        .collect())
}

#[async_trait]
impl ProvenanceRepository for SeaOrmProvenanceRepository {
    async fn insert_provenance(&self, value: StoredProvenance) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        insert_provenance_in_transaction(&txn, value).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn get_provenance(&self, id: &DerivedId) -> StoreResult<Option<StoredProvenance>> {
        let Some(row) = provenance::Entity::find_by_id(id.as_str())
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };
        let inputs = provenance_inputs::Entity::find()
            .filter(provenance_inputs::Column::DerivedId.eq(id.as_str()))
            .order_by_asc(provenance_inputs::Column::Position)
            .all(&self.db)
            .await?
            .into_iter()
            .map(|edge| DerivedId::new(edge.input_derived_id))
            .collect();
        Ok(Some(StoredProvenance {
            id: DerivedId::new(row.derived_id),
            run_id: row.run_id,
            provenance: Arc::new(Provenance {
                operation: parse_operation(&row.operation)?,
                producer: PluginId::new(row.producer),
                algorithm: row.algorithm.into(),
                version: semver::Version::parse(&row.version)
                    .context("invalid stored provenance version")?,
                params: serde_json::from_str(&row.params_json)
                    .context("invalid stored provenance parameters")?,
                params_hash: ParameterHash::new(row.params_hash),
                inputs,
                source: row.source.map(SourceRef::new),
                timestamp: Timestamp::new(row.timestamp),
                domain_version: row.domain_version.map(DomainVersion::new),
                frame_version: row.frame_version.map(FrameVersion::new),
                model: row.model.map(ModelRef::new),
            }),
        }))
    }

    async fn direct_inputs(&self, id: &DerivedId) -> StoreResult<Vec<DerivedId>> {
        if provenance::Entity::find_by_id(id.as_str())
            .one(&self.db)
            .await?
            .is_none()
        {
            return Err(StoreError::NotFound {
                path: id.to_string(),
            });
        }
        Ok(provenance_inputs::Entity::find()
            .filter(provenance_inputs::Column::DerivedId.eq(id.as_str()))
            .order_by_asc(provenance_inputs::Column::Position)
            .all(&self.db)
            .await?
            .into_iter()
            .map(|row| DerivedId::new(row.input_derived_id))
            .collect())
    }

    async fn ancestors(&self, id: &DerivedId) -> StoreResult<Vec<DerivedId>> {
        if provenance::Entity::find_by_id(id.as_str())
            .one(&self.db)
            .await?
            .is_none()
        {
            return Err(StoreError::NotFound {
                path: id.to_string(),
            });
        }
        let txn = self.db.begin().await?;
        let mut queue: VecDeque<_> = inputs_in_txn(&txn, id).await?.into();
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        while let Some(current) = queue.pop_front() {
            if !seen.insert(current.clone()) {
                continue;
            }
            queue.extend(inputs_in_txn(&txn, &current).await?);
            result.push(current);
        }
        txn.commit().await?;
        Ok(result)
    }
}

pub(crate) async fn insert_provenance_in_transaction(
    txn: &DatabaseTransaction,
    value: StoredProvenance,
) -> StoreResult<()> {
    if value.id.is_empty() {
        return Err(invalid("derived id must not be empty"));
    }
    if value.provenance.params_hash != hash_params(&value.provenance.params) {
        return Err(invalid(
            "provenance parameter hash does not match its parameters",
        ));
    }
    let mut unique_inputs = BTreeSet::new();
    for input in &value.provenance.inputs {
        if input == &value.id {
            return Err(invalid("provenance cannot depend on itself"));
        }
        if !unique_inputs.insert(input) {
            return Err(invalid("provenance inputs must be unique"));
        }
    }

    if provenance::Entity::find_by_id(value.id.as_str())
        .one(txn)
        .await?
        .is_some()
    {
        return Err(StoreError::AlreadyExists {
            path: value.id.to_string(),
        });
    }

    let StoredProvenance {
        id,
        run_id,
        provenance: details,
    } = value;
    // The columns are filled from the shared record rather than by moving out
    // of it. Only the short identifier strings are copied; `params` — the one
    // field that can be arbitrarily large — is serialized straight from the
    // borrow, so persisting provenance never duplicates its parameter tree.
    provenance::Entity::insert(provenance::ActiveModel {
        derived_id: Set(id.to_string()),
        run_id: Set(run_id),
        operation: Set(operation_name(details.operation).into()),
        producer: Set(details.producer.to_string()),
        algorithm: Set(details.algorithm.to_string()),
        version: Set(details.version.to_string()),
        params_json: Set(serde_json::to_string(&details.params).map_err(anyhow::Error::from)?),
        params_hash: Set(details.params_hash.to_string()),
        source: Set(details.source.as_ref().map(|value| value.to_string())),
        timestamp: Set(details.timestamp.to_string()),
        domain_version: Set(details
            .domain_version
            .as_ref()
            .map(|value| value.to_string())),
        frame_version: Set(details
            .frame_version
            .as_ref()
            .map(|value| value.to_string())),
        model: Set(details.model.as_ref().map(|value| value.to_string())),
    })
    .exec(txn)
    .await?;

    for (position, input) in details.inputs.iter().enumerate() {
        if provenance::Entity::find_by_id(input.as_str())
            .one(txn)
            .await?
            .is_none()
        {
            return Err(StoreError::NotFound {
                path: format!("provenance input {}", input),
            });
        }
        let position = i32::try_from(position).context("too many provenance inputs")?;
        provenance_inputs::Entity::insert(provenance_inputs::ActiveModel {
            derived_id: Set(id.to_string()),
            input_derived_id: Set(input.to_string()),
            position: Set(position),
        })
        .exec(txn)
        .await?;
    }

    Ok(())
}
