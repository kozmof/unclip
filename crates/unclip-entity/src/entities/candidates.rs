use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "candidates")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub domain_version_id: String,
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
        belongs_to = "super::domain_versions::Entity",
        from = "Column::DomainVersionId",
        to = "super::domain_versions::Column::Id",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    DomainVersion,
    #[sea_orm(
        belongs_to = "super::provenance::Entity",
        from = "Column::ProvenanceId",
        to = "super::provenance::Column::DerivedId",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    Provenance,
}
impl Related<super::domain_versions::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::DomainVersion.def()
    }
}
impl Related<super::provenance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Provenance.def()
    }
}
impl ActiveModelBehavior for ActiveModel {}
