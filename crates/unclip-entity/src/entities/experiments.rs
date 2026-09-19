use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "experiments")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub engine_run_id: String,
    #[sea_orm(column_type = "Text")]
    pub candidate_id: String,
    #[sea_orm(column_type = "Text")]
    pub domain_version_id: String,
    #[sea_orm(column_type = "Text")]
    pub frame_version_id: String,
    #[sea_orm(column_type = "Text")]
    pub plan_json: String,
    #[sea_orm(column_type = "Text")]
    pub status: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub result_json: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub provenance_id: String,
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
        on_delete = "Restrict"
    )]
    EngineRun,
    #[sea_orm(
        belongs_to = "super::candidates::Entity",
        from = "(Column::CandidateId, Column::DomainVersionId)",
        to = "(super::candidates::Column::Id, super::candidates::Column::DomainVersionId)",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    Candidate,
    #[sea_orm(
        belongs_to = "super::frame_versions::Entity",
        from = "(Column::FrameVersionId, Column::DomainVersionId)",
        to = "(super::frame_versions::Column::Id, super::frame_versions::Column::DomainVersionId)",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    FrameVersion,
    #[sea_orm(
        belongs_to = "super::provenance::Entity",
        from = "Column::ProvenanceId",
        to = "super::provenance::Column::DerivedId",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    Provenance,
}
impl Related<super::engine_runs::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EngineRun.def()
    }
}
impl Related<super::candidates::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Candidate.def()
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
impl ActiveModelBehavior for ActiveModel {}
