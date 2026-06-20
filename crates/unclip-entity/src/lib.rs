//! unclip-entity — SeaORM entities generated from the SQLite schema.
//!
//! Regenerate with:
//! ```text
//! cargo run -p unclip-migration --example build_schema_db
//! sea-orm-cli generate entity -u "sqlite:///tmp/unclip_schema.db" \
//!     -o crates/unclip-entity/src/entities --with-serde both
//! ```
//! Manual fixups required after each regeneration:
//! - `branches::Model::weight`: `Decimal` -> `f64`, and drop `Eq` from its
//!   derive (codegen maps SQLite `REAL` to `Decimal`).
//! - Auto-increment primary keys come out as `Option<i32>`; change to `i32`.
//! - `selection_packets::Model::id`: `Option<String>` -> `String`.
//! - `frame_slot_o2o_values` / `frame_slot_o2m_values` have no DB primary key;
//!   declare a composite `primary_key` over all four columns.

mod entities;

pub use entities::*;
