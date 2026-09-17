//! SeaORM entity for ordered provenance dependency edges.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "provenance_inputs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub derived_id: String,
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub input_derived_id: String,
    pub position: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::provenance::Entity",
        from = "Column::DerivedId",
        to = "super::provenance::Column::DerivedId",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Derived,
    #[sea_orm(
        belongs_to = "super::provenance::Entity",
        from = "Column::InputDerivedId",
        to = "super::provenance::Column::DerivedId",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Input,
}

impl Related<super::provenance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Derived.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
