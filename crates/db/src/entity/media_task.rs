use sea_orm::entity::prelude::*;

/// A task left on a file. No row for a file: nothing is left on it.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "media_task")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub media_part_id: i64,
    /// [`HEADERS`], [`PACKETS`], [`SUBTITLES`] or [`PREVIEW_THUMBNAILS`]: the order the queue
    /// takes them in.
    #[sea_orm(primary_key, auto_increment = false)]
    pub task: i32,
    pub library_section_id: i64,
    pub queued_at: i64,
}

/// Its headers are unread: nothing of its media is known.
pub const HEADERS: i32 = 0;
/// The rates measured over its packets.
pub const PACKETS: i32 = 1;
/// Its embedded subtitle tracks, extracted.
pub const SUBTITLES: i32 = 2;
/// Its BIF.
pub const PREVIEW_THUMBNAILS: i32 = 3;

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
