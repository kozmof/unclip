use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "measurement_profiles")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub engine_run_id: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub observation_id: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub frame_version_id: String,
    #[sea_orm(column_type = "Text")]
    pub provenance_id: String,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
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
    #[sea_orm(
        belongs_to = "super::observations::Entity",
        from = "Column::ObservationId",
        to = "super::observations::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Observation,
    #[sea_orm(
        belongs_to = "super::frame_versions::Entity",
        from = "Column::FrameVersionId",
        to = "super::frame_versions::Column::Id",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    FrameVersion,
    #[sea_orm(
        belongs_to = "super::provenance::Entity",
        from = "Column::ProvenanceId",
        to = "super::provenance::Column::DerivedId",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Provenance,
    #[sea_orm(has_many = "super::measurements::Entity")]
    Measurements,
    #[sea_orm(has_many = "super::empirical_structures::Entity")]
    EmpiricalStructures,
}

impl Related<super::engine_runs::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EngineRun.def()
    }
}
impl Related<super::observations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Observation.def()
    }
}
impl Related<super::frame_versions::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::FrameVersion.def()
    }
}
impl Related<super::provenance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Provenance.def()
    }
}
impl Related<super::measurements::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Measurements.def()
    }
}
impl Related<super::empirical_structures::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EmpiricalStructures.def()
    }
}
impl ActiveModelBehavior for ActiveModel {}
