//! SeaORM entity for persisted leveling engine runs.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "engine_runs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub resolved_plan_json: String,
    #[sea_orm(column_type = "Text")]
    pub status: String,
    #[sea_orm(column_type = "Text")]
    pub started_at: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub completed_at: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub metadata_json: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::provenance::Entity")]
    Provenance,
}

impl Related<super::provenance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Provenance.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
