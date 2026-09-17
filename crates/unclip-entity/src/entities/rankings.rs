use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "rankings")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub observation_id: String,
    #[sea_orm(column_type = "Text")]
    pub provenance_id: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::observations::Entity",
        from = "Column::ObservationId",
        to = "super::observations::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Observation,
    #[sea_orm(
        belongs_to = "super::provenance::Entity",
        from = "Column::ProvenanceId",
        to = "super::provenance::Column::DerivedId",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Provenance,
    #[sea_orm(has_many = "super::ranking_entries::Entity")]
    Entries,
}

impl Related<super::observations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Observation.def()
    }
}
impl Related<super::provenance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Provenance.def()
    }
}
impl Related<super::ranking_entries::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Entries.def()
    }
}
impl ActiveModelBehavior for ActiveModel {}
