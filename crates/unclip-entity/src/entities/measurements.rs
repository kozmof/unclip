use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "measurements")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub profile_id: String,
    #[sea_orm(column_type = "Text")]
    pub sensor_run_id: String,
    #[sea_orm(column_type = "Text")]
    pub provenance_id: String,
    #[sea_orm(column_type = "Text")]
    pub kind: String,
    #[sea_orm(column_type = "Text")]
    pub status: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub value_json: Option<String>,
    pub confidence: Option<f64>,
    pub sample_count: Option<i64>,
    #[sea_orm(column_type = "Text")]
    pub context_json: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::measurement_profiles::Entity",
        from = "Column::ProfileId",
        to = "super::measurement_profiles::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Profile,
    #[sea_orm(
        belongs_to = "super::sensor_runs::Entity",
        from = "Column::SensorRunId",
        to = "super::sensor_runs::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    SensorRun,
    #[sea_orm(
        belongs_to = "super::provenance::Entity",
        from = "Column::ProvenanceId",
        to = "super::provenance::Column::DerivedId",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Provenance,
}

impl Related<super::measurement_profiles::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Profile.def()
    }
}
impl Related<super::sensor_runs::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SensorRun.def()
    }
}
impl Related<super::provenance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Provenance.def()
    }
}
impl ActiveModelBehavior for ActiveModel {}
