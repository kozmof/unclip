use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "observed_units")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub observation_id: String,
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub label: String,
    pub salience: Option<f64>,
    pub uncertainty: Option<f64>,
    #[sea_orm(column_type = "Text")]
    pub context_json: String,
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
impl ActiveModelBehavior for ActiveModel {}
