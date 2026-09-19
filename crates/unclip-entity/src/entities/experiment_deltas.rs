use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "experiment_deltas")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub experiment_id: String,
    #[sea_orm(column_type = "Text")]
    pub before_profile_id: String,
    #[sea_orm(column_type = "Text")]
    pub after_profile_id: String,
    #[sea_orm(column_type = "Text")]
    pub comparator_id: String,
    #[sea_orm(column_type = "Text")]
    pub comparator_version: String,
    #[sea_orm(column_type = "Text")]
    pub kind: String,
    #[sea_orm(column_type = "Text")]
    pub value_json: String,
    #[sea_orm(column_type = "Text")]
    pub provenance_id: String,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::experiments::Entity",
        from = "Column::ExperimentId",
        to = "super::experiments::Column::Id",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    Experiment,
    #[sea_orm(
        belongs_to = "super::measurement_profiles::Entity",
        from = "Column::BeforeProfileId",
        to = "super::measurement_profiles::Column::Id",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    BeforeProfile,
    #[sea_orm(
        belongs_to = "super::measurement_profiles::Entity",
        from = "Column::AfterProfileId",
        to = "super::measurement_profiles::Column::Id",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    AfterProfile,
    #[sea_orm(
        belongs_to = "super::provenance::Entity",
        from = "Column::ProvenanceId",
        to = "super::provenance::Column::DerivedId",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    Provenance,
}
impl Related<super::experiments::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Experiment.def()
    }
}
impl Related<super::provenance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Provenance.def()
    }
}
impl ActiveModelBehavior for ActiveModel {}
