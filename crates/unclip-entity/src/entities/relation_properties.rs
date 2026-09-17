use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "relation_properties")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub domain_version_id: String,
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub relation_id: String,
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub name: String,
    #[sea_orm(column_type = "Text")]
    pub value_kind: String,
    pub boolean_value: Option<bool>,
    pub integer_value: Option<i64>,
    pub number_value: Option<f64>,
    #[sea_orm(column_type = "Text", nullable)]
    pub text_value: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub structured_json: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::relations::Entity",
        from = "(Column::DomainVersionId, Column::RelationId)",
        to = "(super::relations::Column::DomainVersionId, super::relations::Column::Id)",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Relation,
}

impl Related<super::relations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Relation.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
