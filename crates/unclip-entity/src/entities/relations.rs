use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "relations")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub domain_version_id: String,
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub source_unit_id: String,
    #[sea_orm(column_type = "Text")]
    pub target_unit_id: String,
    #[sea_orm(column_type = "Text")]
    pub kind: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::domain_versions::Entity",
        from = "Column::DomainVersionId",
        to = "super::domain_versions::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    DomainVersion,
    #[sea_orm(has_many = "super::relation_properties::Entity")]
    Properties,
}

impl Related<super::domain_versions::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::DomainVersion.def()
    }
}
impl Related<super::relation_properties::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Properties.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
