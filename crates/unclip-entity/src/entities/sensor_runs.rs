use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "sensor_runs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub engine_run_id: String,
    #[sea_orm(column_type = "Text")]
    pub sensor_id: String,
    #[sea_orm(column_type = "Text")]
    pub sensor_version: String,
    #[sea_orm(column_type = "Text")]
    pub params_json: String,
    #[sea_orm(column_type = "Text")]
    pub params_hash: String,
    #[sea_orm(column_type = "Text")]
    pub status: String,
    #[sea_orm(column_type = "Text")]
    pub started_at: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub completed_at: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::engine_runs::Entity",
        from = "Column::EngineRunId",
        to = "super::engine_runs::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    EngineRun,
    #[sea_orm(has_many = "super::measurements::Entity")]
    Measurements,
}

impl Related<super::engine_runs::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EngineRun.def()
    }
}
impl Related<super::measurements::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Measurements.def()
    }
}
impl ActiveModelBehavior for ActiveModel {}
