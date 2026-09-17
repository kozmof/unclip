use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "alignment_candidates")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub alignment_id: String,
    #[sea_orm(column_type = "Text")]
    pub observation_id: String,
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub observed_unit_id: String,
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub domain_version_id: String,
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub domain_unit_id: String,
    pub position: i32,
    pub confidence: f64,
    #[sea_orm(column_type = "Text")]
    pub evidence_json: String,
    #[sea_orm(column_type = "Text")]
    pub provenance_id: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::alignments::Entity",
        from = "(Column::AlignmentId, Column::ObservationId)",
        to = "(super::alignments::Column::Id, super::alignments::Column::ObservationId)",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Alignment,
    #[sea_orm(
        belongs_to = "super::observed_units::Entity",
        from = "(Column::ObservationId, Column::ObservedUnitId)",
        to = "(super::observed_units::Column::ObservationId, super::observed_units::Column::Id)",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    ObservedUnit,
    #[sea_orm(
        belongs_to = "super::units::Entity",
        from = "(Column::DomainVersionId, Column::DomainUnitId)",
        to = "(super::units::Column::DomainVersionId, super::units::Column::Id)",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    DomainUnit,
    #[sea_orm(
        belongs_to = "super::provenance::Entity",
        from = "Column::ProvenanceId",
        to = "super::provenance::Column::DerivedId",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Provenance,
}

impl Related<super::alignments::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Alignment.def()
    }
}
impl Related<super::observed_units::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ObservedUnit.def()
    }
}
impl Related<super::units::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::DomainUnit.def()
    }
}
impl Related<super::provenance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Provenance.def()
    }
}
impl ActiveModelBehavior for ActiveModel {}
