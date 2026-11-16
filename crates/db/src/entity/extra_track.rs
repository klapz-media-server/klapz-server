use sea_orm::entity::prelude::*;

/// One track of an extra's file.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "extra_track")]
pub struct Model {
    /// The track's id in the source, numbered with the items' tracks.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub extra_media_id: i64,
    /// `video`, `audio` or `subtitle`.
    pub kind: String,
    pub position: i32,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub channels: Option<i32>,
    /// Bits per second.
    pub bitrate: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub is_default: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::extra_media::Entity",
        from = "Column::ExtraMediaId",
        to = "super::extra_media::Column::Id",
        on_delete = "Cascade"
    )]
    ExtraMedia,
}

impl ActiveModelBehavior for ActiveModel {}
