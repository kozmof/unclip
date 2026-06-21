//! Repository for frames (reusable constraint sets).

use std::collections::HashMap;

use async_trait::async_trait;
use sea_orm::{
    ActiveValue::{NotSet, Set},
    ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait, QueryFilter,
    TransactionTrait,
};
use unclip_core::{Frame, Slot};
use unclip_entity::{frame_slot_o2m_values, frame_slot_o2o_values, frame_slots, frames};

use crate::frame_mapper;

/// Summary of a stored frame, used for `unclip frames`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameInfo {
    pub name: String,
    pub description: Option<String>,
    pub slot_count: usize,
}

/// Persistence boundary for frames.
#[async_trait]
pub trait FrameRepository {
    /// Insert or replace a frame and all of its slots.
    async fn save_frame(&self, frame: Frame) -> anyhow::Result<()>;
    async fn get_frame(&self, name: &str) -> anyhow::Result<Option<Frame>>;
    async fn list_frames(&self) -> anyhow::Result<Vec<FrameInfo>>;
    async fn delete_frame(&self, name: &str) -> anyhow::Result<()>;
}

/// SeaORM implementation backed by SQLite.
pub struct SeaOrmFrameRepository {
    db: DatabaseConnection,
}

impl SeaOrmFrameRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Delete a frame and its slot/value rows within an existing transaction.
    async fn delete_in_txn(txn: &DatabaseTransaction, frame_id: i32) -> anyhow::Result<()> {
        let slot_ids: Vec<i32> = frame_slots::Entity::find()
            .filter(frame_slots::Column::FrameId.eq(frame_id))
            .all(txn)
            .await?
            .into_iter()
            .map(|s| s.id)
            .collect();

        if !slot_ids.is_empty() {
            frame_slot_o2o_values::Entity::delete_many()
                .filter(frame_slot_o2o_values::Column::SlotId.is_in(slot_ids.clone()))
                .exec(txn)
                .await?;
            frame_slot_o2m_values::Entity::delete_many()
                .filter(frame_slot_o2m_values::Column::SlotId.is_in(slot_ids))
                .exec(txn)
                .await?;
        }
        frame_slots::Entity::delete_many()
            .filter(frame_slots::Column::FrameId.eq(frame_id))
            .exec(txn)
            .await?;
        frames::Entity::delete_by_id(frame_id).exec(txn).await?;
        Ok(())
    }

    async fn insert_slot(
        txn: &DatabaseTransaction,
        frame_id: i32,
        slot: &Slot,
    ) -> anyhow::Result<()> {
        let am = frame_slots::ActiveModel {
            id: NotSet,
            frame_id: Set(frame_id),
            name: Set(slot.name.clone()),
            under_path: Set(slot.under.clone()),
            count: Set(slot.count as i32),
            avoid_recent: Set(slot.avoid_recent as i32),
            weighted: Set(slot.weighted as i32),
            metadata_suggest_json: Set(frame_mapper::metadata_suggest_json(slot)?),
        };
        let slot_id = frame_slots::Entity::insert(am)
            .exec(txn)
            .await?
            .last_insert_id;

        let o2o: Vec<_> = frame_mapper::slot_o2o_rows(slot)
            .into_iter()
            .map(|(mode, name, value)| frame_slot_o2o_values::ActiveModel {
                slot_id: Set(slot_id),
                mode: Set(mode.to_string()),
                name: Set(name),
                value: Set(value),
            })
            .collect();
        if !o2o.is_empty() {
            frame_slot_o2o_values::Entity::insert_many(o2o)
                .exec(txn)
                .await?;
        }

        let o2m: Vec<_> = frame_mapper::slot_o2m_rows(slot)
            .into_iter()
            .map(|(mode, name, value)| frame_slot_o2m_values::ActiveModel {
                slot_id: Set(slot_id),
                mode: Set(mode.to_string()),
                name: Set(name),
                value: Set(value),
            })
            .collect();
        if !o2m.is_empty() {
            frame_slot_o2m_values::Entity::insert_many(o2m)
                .exec(txn)
                .await?;
        }
        Ok(())
    }

    /// Hydrate many slots with a fixed number of queries (no N+1): load all
    /// o2o/o2m value rows for the whole slot-id set at once, then group them per
    /// slot. Slot order is preserved from `models`.
    async fn hydrate_slots(&self, models: Vec<frame_slots::Model>) -> anyhow::Result<Vec<Slot>> {
        if models.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i32> = models.iter().map(|m| m.id).collect();

        let o2o = frame_slot_o2o_values::Entity::find()
            .filter(frame_slot_o2o_values::Column::SlotId.is_in(ids.clone()))
            .all(&self.db)
            .await?;
        let o2m = frame_slot_o2m_values::Entity::find()
            .filter(frame_slot_o2m_values::Column::SlotId.is_in(ids))
            .all(&self.db)
            .await?;

        let mut o2o_by_id: HashMap<i32, Vec<frame_slot_o2o_values::Model>> = HashMap::new();
        for row in o2o {
            o2o_by_id.entry(row.slot_id).or_default().push(row);
        }
        let mut o2m_by_id: HashMap<i32, Vec<frame_slot_o2m_values::Model>> = HashMap::new();
        for row in o2m {
            o2m_by_id.entry(row.slot_id).or_default().push(row);
        }

        let mut slots = Vec::with_capacity(models.len());
        for model in models {
            let id = model.id;
            slots.push(frame_mapper::assemble_slot(
                model,
                o2o_by_id.remove(&id).unwrap_or_default(),
                o2m_by_id.remove(&id).unwrap_or_default(),
            )?);
        }
        Ok(slots)
    }
}

#[async_trait]
impl FrameRepository for SeaOrmFrameRepository {
    async fn save_frame(&self, frame: Frame) -> anyhow::Result<()> {
        let txn = self.db.begin().await?;

        if let Some(existing) = frames::Entity::find()
            .filter(frames::Column::Name.eq(&frame.name))
            .one(&txn)
            .await?
        {
            Self::delete_in_txn(&txn, existing.id).await?;
        }

        let am = frames::ActiveModel {
            id: NotSet,
            name: Set(frame.name.clone()),
            description: Set(frame.description.clone()),
        };
        let frame_id = frames::Entity::insert(am).exec(&txn).await?.last_insert_id;

        for slot in &frame.slots {
            Self::insert_slot(&txn, frame_id, slot).await?;
        }

        txn.commit().await?;
        Ok(())
    }

    async fn get_frame(&self, name: &str) -> anyhow::Result<Option<Frame>> {
        let Some(frame) = frames::Entity::find()
            .filter(frames::Column::Name.eq(name))
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };

        let slot_models = frame_slots::Entity::find()
            .filter(frame_slots::Column::FrameId.eq(frame.id))
            .all(&self.db)
            .await?;
        let slots = self.hydrate_slots(slot_models).await?;

        Ok(Some(frame_mapper::assemble_frame(
            frame.name,
            frame.description,
            slots,
        )))
    }

    async fn list_frames(&self) -> anyhow::Result<Vec<FrameInfo>> {
        let mut frames_list = frames::Entity::find().all(&self.db).await?;
        frames_list.sort_by(|a, b| a.name.cmp(&b.name));

        // Count slots per frame in one query rather than one query per frame.
        let mut slot_counts: HashMap<i32, usize> = HashMap::new();
        for slot in frame_slots::Entity::find().all(&self.db).await? {
            *slot_counts.entry(slot.frame_id).or_default() += 1;
        }

        Ok(frames_list
            .into_iter()
            .map(|frame| FrameInfo {
                slot_count: slot_counts.get(&frame.id).copied().unwrap_or(0),
                name: frame.name,
                description: frame.description,
            })
            .collect())
    }

    async fn delete_frame(&self, name: &str) -> anyhow::Result<()> {
        let Some(frame) = frames::Entity::find()
            .filter(frames::Column::Name.eq(name))
            .one(&self.db)
            .await?
        else {
            return Ok(());
        };
        let txn = self.db.begin().await?;
        Self::delete_in_txn(&txn, frame.id).await?;
        txn.commit().await?;
        Ok(())
    }
}
