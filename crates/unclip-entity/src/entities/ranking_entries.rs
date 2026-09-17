use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "ranking_entries")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub ranking_id: String,
    #[sea_orm(column_type = "Text")]
    pub observation_id: String,
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub observed_unit_id: String,
    #[sea_orm(column_type = "Text")]
    pub state: String,
    pub tier: i32,
    pub position: i32,
    #[sea_orm(column_type = "Text")]
    pub provenance_id: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::rankings::Entity",
        from = "(Column::RankingId, Column::ObservationId)",
        to = "(super::rankings::Column::Id, super::rankings::Column::ObservationId)",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Ranking,
    #[sea_orm(
        belongs_to = "super::observed_units::Entity",
        from = "(Column::ObservationId, Column::ObservedUnitId)",
        to = "(super::observed_units::Column::ObservationId, super::observed_units::Column::Id)",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    ObservedUnit,
    #[sea_orm(
        belongs_to = "super::provenance::Entity",
        from = "Column::ProvenanceId",
        to = "super::provenance::Column::DerivedId",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Provenance,
}

impl Related<super::rankings::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Ranking.def()
    }
}
impl Related<super::observed_units::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ObservedUnit.def()
    }
}
impl Related<super::provenance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Provenance.def()
    }
}
impl ActiveModelBehavior for ActiveModel {}
