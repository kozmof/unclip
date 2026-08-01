//! Repository for frames (reusable constraint sets).

use std::collections::{HashMap, HashSet};

use anyhow::{ensure, Context};
use async_trait::async_trait;
use sea_orm::{
    ActiveValue::{NotSet, Set},
    ColumnTrait, DatabaseConnection, DatabaseTransaction, DbBackend, EntityTrait, FromQueryResult,
    QueryFilter, QueryOrder, QuerySelect, Statement, TransactionTrait,
};
use unclip_core::{validate_frame, Frame, Slot};
use unclip_entity::{frame_slot_o2m_values, frame_slot_o2o_values, frame_slots, frames};

use crate::frame_mapper;
use crate::repository::{ensure_bulk_result_limit, MAX_BULK_RESULTS};
use crate::sqlite_limits::{insert_chunked, ID_CHUNK};
use crate::{StoreError, StoreResult};

/// Summary of a stored frame, used for `unclip frames`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameInfo {
    pub name: String,
    pub description: Option<String>,
    pub slot_count: usize,
}

/// Persistence boundary for frames.
#[async_trait]
pub trait FrameRepository: Sync {
    /// Insert or replace a frame and all of its slots.
    async fn save_frame(&self, frame: Frame) -> StoreResult<()>;
    /// Insert or replace many frames atomically: a failure on any frame rolls
    /// the entire batch back, so an import never half-applies.
    async fn save_frames(&self, frames: Vec<Frame>) -> StoreResult<()>;
    async fn get_frame(&self, name: &str) -> StoreResult<Option<Frame>>;
    async fn list_frames(&self) -> StoreResult<Vec<FrameInfo>>;
    async fn delete_frame(&self, name: &str) -> StoreResult<()>;
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
    async fn delete_in_txn(txn: &DatabaseTransaction, frame_id: i64) -> anyhow::Result<()> {
        let slot_ids: Vec<i64> = frame_slots::Entity::find()
            .filter(frame_slots::Column::FrameId.eq(frame_id))
            .all(txn)
            .await?
            .into_iter()
            .map(|s| s.id)
            .collect();

        if !slot_ids.is_empty() {
            for chunk in slot_ids.chunks(ID_CHUNK) {
                frame_slot_o2o_values::Entity::delete_many()
                    .filter(frame_slot_o2o_values::Column::SlotId.is_in(chunk.iter().copied()))
                    .exec(txn)
                    .await?;
                frame_slot_o2m_values::Entity::delete_many()
                    .filter(frame_slot_o2m_values::Column::SlotId.is_in(chunk.iter().copied()))
                    .exec(txn)
                    .await?;
            }
        }
        frame_slots::Entity::delete_many()
            .filter(frame_slots::Column::FrameId.eq(frame_id))
            .exec(txn)
            .await?;
        frames::Entity::delete_by_id(frame_id).exec(txn).await?;
        Ok(())
    }

    /// Validate and persist one frame within an existing transaction.
    ///
    /// Shared by `save_frame` (single-frame transaction) and `save_frames`
    /// (one transaction spanning the whole batch), so both paths apply the same
    /// validation and replace-then-insert behavior.
    async fn save_frame_in_txn(txn: &DatabaseTransaction, frame: Frame) -> anyhow::Result<()> {
        if let Some(existing) = frames::Entity::find()
            .filter(frames::Column::Name.eq(&frame.name))
            .one(txn)
            .await?
        {
            Self::delete_in_txn(txn, existing.id).await?;
        }

        let am = frames::ActiveModel {
            id: NotSet,
            name: Set(frame.name),
            description: Set(frame.description),
        };
        let frame_id = frames::Entity::insert(am).exec(txn).await?.last_insert_id;

        // The slots are consumed: this function owns the frame, and nothing
        // reads it after the rows are written, so each slot's name and every
        // constraint value move into SQL instead of being copied there.
        for (position, slot) in frame.slots.into_iter().enumerate() {
            let position = i32::try_from(position).context("frame has too many slots")?;
            Self::insert_slot(txn, frame_id, position, slot).await?;
        }
        Ok(())
    }

    async fn insert_slot(
        txn: &DatabaseTransaction,
        frame_id: i64,
        position: i32,
        slot: Slot,
    ) -> anyhow::Result<()> {
        // Both read the slot whole; take them before it is split apart.
        let count = checked_slot_count(&slot)?;
        let metadata_suggest_json = frame_mapper::metadata_suggest_json(&slot)?;

        let Slot {
            name,
            under,
            require_o2o,
            default_o2o,
            avoid_o2o,
            require_o2m,
            prefer_o2m,
            avoid_o2m,
            avoid_recent,
            weighted,
            ..
        } = slot;

        let am = frame_slots::ActiveModel {
            id: NotSet,
            frame_id: Set(frame_id),
            name: Set(name),
            position: Set(position),
            under_path: Set(under),
            count: Set(count),
            avoid_recent: Set(avoid_recent as i32),
            weighted: Set(weighted as i32),
            metadata_suggest_json: Set(metadata_suggest_json),
        };
        let slot_id = frame_slots::Entity::insert(am)
            .exec(txn)
            .await?
            .last_insert_id;

        let (o2o_rows, o2m_rows) = frame_mapper::SlotValues {
            require_o2o,
            default_o2o,
            avoid_o2o,
            require_o2m,
            prefer_o2m,
            avoid_o2m,
        }
        .into_rows();

        let o2o: Vec<_> = o2o_rows
            .into_iter()
            .map(|(mode, name, value)| frame_slot_o2o_values::ActiveModel {
                slot_id: Set(slot_id),
                mode: Set(mode.to_string()),
                name: Set(name),
                value: Set(value),
            })
            .collect();
        insert_chunked(txn, o2o).await?;

        let o2m: Vec<_> = o2m_rows
            .into_iter()
            .map(|(mode, name, value)| frame_slot_o2m_values::ActiveModel {
                slot_id: Set(slot_id),
                mode: Set(mode.to_string()),
                name: Set(name),
                value: Set(value),
            })
            .collect();
        insert_chunked(txn, o2m).await?;
        Ok(())
    }

    /// Hydrate many slots with a fixed number of queries (no N+1): load all
    /// o2o/o2m value rows for the whole slot-id set at once, then group them per
    /// slot. Slot order is preserved from `models`.
    async fn hydrate_slots(&self, models: Vec<frame_slots::Model>) -> anyhow::Result<Vec<Slot>> {
        if models.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = models.iter().map(|m| m.id).collect();

        let mut o2o = Vec::new();
        let mut o2m = Vec::new();
        for chunk in ids.chunks(ID_CHUNK) {
            o2o.extend(
                frame_slot_o2o_values::Entity::find()
                    .filter(frame_slot_o2o_values::Column::SlotId.is_in(chunk.iter().copied()))
                    .all(&self.db)
                    .await?,
            );
            o2m.extend(
                frame_slot_o2m_values::Entity::find()
                    .filter(frame_slot_o2m_values::Column::SlotId.is_in(chunk.iter().copied()))
                    .all(&self.db)
                    .await?,
            );
        }

        let mut o2o_by_id: HashMap<i64, Vec<frame_slot_o2o_values::Model>> = HashMap::new();
        for row in o2o {
            o2o_by_id.entry(row.slot_id).or_default().push(row);
        }
        let mut o2m_by_id: HashMap<i64, Vec<frame_slot_o2m_values::Model>> = HashMap::new();
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

fn checked_slot_count(slot: &Slot) -> anyhow::Result<i32> {
    ensure!(slot.count > 0, "slot count must be greater than zero");
    i32::try_from(slot.count).context("slot count exceeds SQLite INTEGER range")
}

#[async_trait]
impl FrameRepository for SeaOrmFrameRepository {
    async fn save_frame(&self, frame: Frame) -> StoreResult<()> {
        validate_frame(&frame)?;
        let txn = self.db.begin().await?;
        Self::save_frame_in_txn(&txn, frame).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn save_frames(&self, frames: Vec<Frame>) -> StoreResult<()> {
        let mut names = HashSet::with_capacity(frames.len());
        for frame in &frames {
            validate_frame(frame)?;
            if !names.insert(&frame.name) {
                return Err(StoreError::InvalidRequest {
                    message: format!("duplicate frame name `{}` in import", frame.name),
                });
            }
        }

        let txn = self.db.begin().await?;
        for frame in frames {
            Self::save_frame_in_txn(&txn, frame).await?;
        }
        txn.commit().await?;
        Ok(())
    }

    async fn get_frame(&self, name: &str) -> StoreResult<Option<Frame>> {
        let Some(frame) = frames::Entity::find()
            .filter(frames::Column::Name.eq(name))
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };

        let slot_models = frame_slots::Entity::find()
            .filter(frame_slots::Column::FrameId.eq(frame.id))
            .order_by_asc(frame_slots::Column::Position)
            .order_by_asc(frame_slots::Column::Id)
            .all(&self.db)
            .await?;
        let slots = self.hydrate_slots(slot_models).await?;

        Ok(Some(frame_mapper::assemble_frame(
            frame.name,
            frame.description,
            slots,
        )))
    }

    async fn list_frames(&self) -> StoreResult<Vec<FrameInfo>> {
        let mut frames_list = frames::Entity::find()
            .limit((MAX_BULK_RESULTS + 1) as u64)
            .all(&self.db)
            .await?;
        ensure_bulk_result_limit(frames_list.len())?;
        frames_list.sort_by(|a, b| a.name.cmp(&b.name));

        // Count slots per frame in SQL rather than hydrating every slot row.
        #[derive(FromQueryResult)]
        struct SlotCount {
            frame_id: i64,
            count: i64,
        }
        let rows = SlotCount::find_by_statement(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT frame_id, COUNT(*) AS count FROM frame_slots GROUP BY frame_id",
        ))
        .all(&self.db)
        .await?;
        let slot_counts: HashMap<i64, usize> = rows
            .into_iter()
            .map(|r| (r.frame_id, r.count.max(0) as usize))
            .collect();

        Ok(frames_list
            .into_iter()
            .map(|frame| FrameInfo {
                slot_count: slot_counts.get(&frame.id).copied().unwrap_or(0),
                name: frame.name,
                description: frame.description,
            })
            .collect())
    }

    async fn delete_frame(&self, name: &str) -> StoreResult<()> {
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
