//! Hand-written entity for `pattern_entries` (DRAFT §18).
//!
//! Not produced by sea-orm-codegen, so it needs no post-generation fixups.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "pattern_entries")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(column_type = "Text")]
    pub pattern: String,
    #[sea_orm(column_type = "Text")]
    pub target_kind: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub target_name: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub target_value: Option<String>,
    pub branch_id: Option<i32>,
    pub enabled: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::branches::Entity",
        from = "Column::BranchId",
        to = "super::branches::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Branches,
}

impl Related<super::branches::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Branches.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
