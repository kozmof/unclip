use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "unit_properties")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub domain_version_id: String,
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub unit_id: String,
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
        belongs_to = "super::units::Entity",
        from = "(Column::DomainVersionId, Column::UnitId)",
        to = "(super::units::Column::DomainVersionId, super::units::Column::Id)",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Unit,
}

impl Related<super::units::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Unit.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
