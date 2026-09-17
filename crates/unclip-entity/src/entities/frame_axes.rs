use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "frame_axes")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub frame_version_id: String,
    #[sea_orm(column_type = "Text")]
    pub domain_version_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub position: i32,
    #[sea_orm(column_type = "Text")]
    pub unit_id: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub label: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::frame_versions::Entity",
        from = "(Column::FrameVersionId, Column::DomainVersionId)",
        to = "(super::frame_versions::Column::Id, super::frame_versions::Column::DomainVersionId)",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    FrameVersion,
    #[sea_orm(
        belongs_to = "super::units::Entity",
        from = "(Column::DomainVersionId, Column::UnitId)",
        to = "(super::units::Column::DomainVersionId, super::units::Column::Id)",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    Unit,
}

impl Related<super::frame_versions::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::FrameVersion.def()
    }
}
impl Related<super::units::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Unit.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
