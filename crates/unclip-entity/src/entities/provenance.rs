//! SeaORM entity for operation provenance.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "provenance")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub derived_id: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub run_id: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub operation: String,
    #[sea_orm(column_type = "Text")]
    pub producer: String,
    #[sea_orm(column_type = "Text")]
    pub algorithm: String,
    #[sea_orm(column_type = "Text")]
    pub version: String,
    #[sea_orm(column_type = "Text")]
    pub params_json: String,
    #[sea_orm(column_type = "Text")]
    pub params_hash: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub source: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub timestamp: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub domain_version: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub frame_version: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub model: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::engine_runs::Entity",
        from = "Column::RunId",
        to = "super::engine_runs::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    EngineRun,
    #[sea_orm(
        has_many = "super::provenance_inputs::Entity",
        from = "Column::DerivedId",
        to = "super::provenance_inputs::Column::DerivedId"
    )]
    DerivedInputs,
    #[sea_orm(
        has_many = "super::provenance_inputs::Entity",
        from = "Column::DerivedId",
        to = "super::provenance_inputs::Column::InputDerivedId"
    )]
    InputUses,
}

impl Related<super::engine_runs::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EngineRun.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
