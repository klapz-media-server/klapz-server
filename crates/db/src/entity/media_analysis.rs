use sea_orm::entity::prelude::*;

/// A file whose media is still to be read. No row: read as far as it can be.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "media_analysis")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub media_part_id: i64,
    pub library_section_id: i64,
    /// What is left: [`HEADERS`], then [`PACKETS`].
    pub stage: i32,
    pub queued_at: i64,
}

/// Its headers are unread: nothing of its media is known.
pub const HEADERS: i32 = 0;
/// Its headers are read; the rates measured over its packets are still to come.
pub const PACKETS: i32 = 1;

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::media_part::Entity",
        from = "Column::MediaPartId",
        to = "super::media_part::Column::Id",
        on_delete = "Cascade"
    )]
    MediaPart,
}

impl Related<super::media_part::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MediaPart.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
