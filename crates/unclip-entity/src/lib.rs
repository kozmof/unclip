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
//! - `selection_packets::Model::seed`: `Option<i32>` -> `Option<i64>` (seeds
//!   are 64-bit; SQLite INTEGER holds them).
//! - `frame_slot_o2o_values` / `frame_slot_o2m_values` have no DB primary key;
//!   declare a composite `primary_key` over all four columns.

#![forbid(unsafe_code)]

mod entities;

pub use entities::*;

/// Assertions for the hand-edits the module documentation lists.
///
/// `sea-orm-cli generate entity` does not produce these types exactly: it maps
/// SQLite `REAL` to `Decimal`, makes every auto-increment key `Option`, and
/// leaves two junction tables without a primary key. The fixups are applied by
/// hand after each regeneration, and a regeneration that misses one still
/// compiles — a 64-bit seed silently narrowed to `i32` is a data bug, not a type
/// error. These tests fail instead.
#[cfg(test)]
mod generated_fixups {
    /// `weight` is `f64`, not `Decimal`: sampling multiplies it.
    #[test]
    fn branch_weight_is_a_float() {
        let model = super::branches::Model {
            id: 1,
            path: "/x".into(),
            parent_path: None,
            title: None,
            description: None,
            weight: 0.5,
            metadata_json: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let _: f64 = model.weight;
        // `id` is a plain i64, matching SQLite's INTEGER PRIMARY KEY rowid.
        let _: i64 = model.id;
    }

    /// Packet seeds are 64-bit; `i32` would wrap two thirds of the seed space.
    #[test]
    fn selection_packet_id_and_seed_keep_their_widths() {
        let model = super::selection_packets::Model {
            id: "packet".into(),
            frame_name: None,
            seed: Some(i64::MAX),
            created_at: String::new(),
            query_json: None,
            packet_json: String::new(),
        };
        let _: String = model.id;
        assert_eq!(model.seed, Some(i64::MAX));
    }

    /// The two frame-slot value tables have no DB primary key, so the entities
    /// declare a composite one over all four columns. Without it SeaORM treats
    /// the first column as the key and silently collapses distinct rows.
    #[test]
    fn frame_slot_value_tables_declare_composite_primary_keys() {
        use sea_orm::Iterable;

        for columns in [
            super::frame_slot_o2o_values::PrimaryKey::iter().count(),
            super::frame_slot_o2m_values::PrimaryKey::iter().count(),
        ] {
            assert_eq!(columns, 4, "composite primary key lost in regeneration");
        }
    }
}
