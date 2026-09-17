use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "observations")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub provenance_id: String,
    #[sea_orm(column_type = "Text")]
    pub source: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub observed_at: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub context_json: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::provenance::Entity",
        from = "Column::ProvenanceId",
        to = "super::provenance::Column::DerivedId",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Provenance,
    #[sea_orm(has_many = "super::observed_units::Entity")]
    Units,
    #[sea_orm(has_many = "super::observed_relations::Entity")]
    Relations,
    #[sea_orm(has_many = "super::alignments::Entity")]
    Alignments,
    #[sea_orm(has_many = "super::rankings::Entity")]
    Rankings,
}

impl Related<super::provenance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Provenance.def()
    }
}
impl Related<super::observed_units::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Units.def()
    }
}
impl Related<super::observed_relations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Relations.def()
    }
}
impl Related<super::alignments::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Alignments.def()
    }
}
impl Related<super::rankings::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Rankings.def()
    }
}
impl ActiveModelBehavior for ActiveModel {}
