use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "experiment_observations")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub experiment_id: String,
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub observation_id: String,
    #[sea_orm(column_type = "Text")]
    pub split: String,
    pub position: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::experiments::Entity",
        from = "Column::ExperimentId",
        to = "super::experiments::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Experiment,
    #[sea_orm(
        belongs_to = "super::observations::Entity",
        from = "Column::ObservationId",
        to = "super::observations::Column::Id",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    Observation,
}
impl Related<super::experiments::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Experiment.def()
    }
}
impl Related<super::observations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Observation.def()
    }
}
impl ActiveModelBehavior for ActiveModel {}
