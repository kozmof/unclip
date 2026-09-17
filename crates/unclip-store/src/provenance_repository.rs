//! Persistence and graph traversal for epistemic provenance.

use std::collections::{BTreeSet, VecDeque};

use anyhow::Context;
use async_trait::async_trait;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder, TransactionTrait,
};
use unclip_entity::{provenance, provenance_inputs};
use unclip_epistemic::{
    hash_params, DerivedId, DomainVersion, FrameVersion, ModelRef, Operation, ParameterHash,
    PluginId, Provenance, SourceRef, Timestamp,
};

use crate::{StoreError, StoreResult};

#[derive(Debug, Clone, PartialEq)]
pub struct StoredProvenance {
    pub id: DerivedId,
    pub run_id: Option<String>,
    pub provenance: Provenance,
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
        .filter(provenance_inputs::Column::DerivedId.eq(&id.0))
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
        if value.id.0.is_empty() {
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
            if !unique_inputs.insert(input.clone()) {
                return Err(invalid("provenance inputs must be unique"));
            }
        }

        let txn = self.db.begin().await?;
        if provenance::Entity::find_by_id(&value.id.0)
            .one(&txn)
            .await?
            .is_some()
        {
            return Err(StoreError::AlreadyExists { path: value.id.0 });
        }

        let StoredProvenance {
            id,
            run_id,
            provenance: details,
        } = value;
        let inputs = details.inputs;
        provenance::Entity::insert(provenance::ActiveModel {
            derived_id: Set(id.0.clone()),
            run_id: Set(run_id),
            operation: Set(operation_name(details.operation).into()),
            producer: Set(details.producer.0),
            algorithm: Set(details.algorithm),
            version: Set(details.version.to_string()),
            params_json: Set(serde_json::to_string(&details.params).map_err(anyhow::Error::from)?),
            params_hash: Set(details.params_hash.0),
            source: Set(details.source.map(|value| value.0)),
            timestamp: Set(details.timestamp.0),
            domain_version: Set(details.domain_version.map(|value| value.0)),
            frame_version: Set(details.frame_version.map(|value| value.0)),
            model: Set(details.model.map(|value| value.0)),
        })
        .exec(&txn)
        .await?;

        for (position, input) in inputs.into_iter().enumerate() {
            if provenance::Entity::find_by_id(&input.0)
                .one(&txn)
                .await?
                .is_none()
            {
                return Err(StoreError::NotFound {
                    path: format!("provenance input {}", input.0),
                });
            }
            let position = i32::try_from(position).context("too many provenance inputs")?;
            provenance_inputs::Entity::insert(provenance_inputs::ActiveModel {
                derived_id: Set(id.0.clone()),
                input_derived_id: Set(input.0),
                position: Set(position),
            })
            .exec(&txn)
            .await?;
        }

        txn.commit().await?;
        Ok(())
    }

    async fn get_provenance(&self, id: &DerivedId) -> StoreResult<Option<StoredProvenance>> {
        let Some(row) = provenance::Entity::find_by_id(&id.0).one(&self.db).await? else {
            return Ok(None);
        };
        let inputs = provenance_inputs::Entity::find()
            .filter(provenance_inputs::Column::DerivedId.eq(&id.0))
            .order_by_asc(provenance_inputs::Column::Position)
            .all(&self.db)
            .await?
            .into_iter()
            .map(|edge| DerivedId::new(edge.input_derived_id))
            .collect();
        Ok(Some(StoredProvenance {
            id: DerivedId::new(row.derived_id),
            run_id: row.run_id,
            provenance: Provenance {
                operation: parse_operation(&row.operation)?,
                producer: PluginId::new(row.producer),
                algorithm: row.algorithm,
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
            },
        }))
    }

    async fn direct_inputs(&self, id: &DerivedId) -> StoreResult<Vec<DerivedId>> {
        if provenance::Entity::find_by_id(&id.0)
            .one(&self.db)
            .await?
            .is_none()
        {
            return Err(StoreError::NotFound { path: id.0.clone() });
        }
        Ok(provenance_inputs::Entity::find()
            .filter(provenance_inputs::Column::DerivedId.eq(&id.0))
            .order_by_asc(provenance_inputs::Column::Position)
            .all(&self.db)
            .await?
            .into_iter()
            .map(|row| DerivedId::new(row.input_derived_id))
            .collect())
    }

    async fn ancestors(&self, id: &DerivedId) -> StoreResult<Vec<DerivedId>> {
        if provenance::Entity::find_by_id(&id.0)
            .one(&self.db)
            .await?
            .is_none()
        {
            return Err(StoreError::NotFound { path: id.0.clone() });
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
