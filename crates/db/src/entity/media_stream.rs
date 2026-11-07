use sea_orm::entity::prelude::*;

/// A video, audio or subtitle track.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "media_stream")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub media_part_id: i64,
    pub kind: String,
    /// The index the transcoder addresses this track by, so it keeps its ffmpeg meaning.
    pub position: i32,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub channels: Option<i32>,
    pub bitrate: Option<i32>,
    pub is_default: bool,
    pub is_forced: bool,
    /// Set for a sidecar subtitle file, null when the track is embedded.
    pub external_path: Option<String>,
}

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
