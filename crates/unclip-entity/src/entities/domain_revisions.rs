use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "domain_revisions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub candidate_id: String,
    #[sea_orm(column_type = "Text")]
    pub experiment_id: String,
    #[sea_orm(column_type = "Text")]
    pub from_version_id: String,
    #[sea_orm(column_type = "Text")]
    pub to_version_id: String,
    #[sea_orm(column_type = "Text")]
    pub reason: String,
    #[sea_orm(column_type = "Text")]
    pub evidence_json: String,
    #[sea_orm(column_type = "Text")]
    pub provenance_id: String,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::experiments::Entity",
        from = "(Column::ExperimentId, Column::CandidateId, Column::FromVersionId)",
        to = "(super::experiments::Column::Id, super::experiments::Column::CandidateId, super::experiments::Column::DomainVersionId)",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    Experiment,
    #[sea_orm(
        belongs_to = "super::domain_versions::Entity",
        from = "Column::FromVersionId",
        to = "super::domain_versions::Column::Id",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    FromVersion,
    #[sea_orm(
        belongs_to = "super::domain_versions::Entity",
        from = "Column::ToVersionId",
        to = "super::domain_versions::Column::Id",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    ToVersion,
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
