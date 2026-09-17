use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "frame_versions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub frame_id: String,
    #[sea_orm(column_type = "Text")]
    pub version: String,
    #[sea_orm(column_type = "Text")]
    pub domain_version_id: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub predecessor_id: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::measurement_frames::Entity",
        from = "Column::FrameId",
        to = "super::measurement_frames::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Frame,
    #[sea_orm(
        belongs_to = "super::domain_versions::Entity",
        from = "Column::DomainVersionId",
        to = "super::domain_versions::Column::Id",
        on_update = "NoAction",
        on_delete = "Restrict"
    )]
    DomainVersion,
    #[sea_orm(has_many = "super::frame_axes::Entity")]
    Axes,
}

impl Related<super::measurement_frames::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Frame.def()
    }
}
impl Related<super::domain_versions::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::DomainVersion.def()
    }
}
impl Related<super::frame_axes::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Axes.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
