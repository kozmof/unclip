//! unclip-store — repository traits and SeaORM-backed persistence.
//!
//! Integration tests for the repositories live in `tests/store.rs`, compiled
//! against the public API exactly as downstream crates see it.

#![forbid(unsafe_code)]

mod error;
mod frame_mapper;
mod frame_repository;
mod history;
mod mapper;
mod pattern_repository;
mod repository;
mod seaorm;
mod sqlite_limits;

pub use error::{StoreError, StoreResult};
pub use frame_repository::{FrameInfo, FrameRepository, SeaOrmFrameRepository};
pub use history::{
    now, HistoryRepository, PacketUsageRecord, SeaOrmHistoryRepository, UsageSummary,
};
pub use pattern_repository::{SeaOrmPatternRepository, StoredPattern};
pub use repository::{
    BranchHeader, BranchReader, BranchRepository, BranchRepositoryError, BranchRepositoryResult,
    BranchWriter, IndexedValue, PageCursor, SeaOrmBranchRepository, MAX_BULK_RESULTS,
    STREAM_PAGE_SIZE,
};
pub use seaorm::{
    connect, connect_and_migrate, connect_and_migrate_with_options, connect_with_options,
};
