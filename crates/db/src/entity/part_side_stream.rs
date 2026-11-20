use sea_orm::entity::prelude::*;

/// What a file carries besides the tracks a player plays: a cover picture stored as a video
/// track, a data track, a file attached to a Matroska file.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "part_side_stream")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub media_part_id: i64,
    /// ffmpeg's index of the stream.
    pub position: i32,
    /// `cover`, `data` or `attachment`.
    pub kind: String,
    pub codec: Option<String>,
    /// As ffprobe prints it.
    pub codec_tag: Option<String>,
    pub profile: Option<String>,
    /// As the file states it.
    pub language: Option<String>,
    pub time_base: Option<String>,
    pub mime_type: Option<String>,
    pub filename: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub pixel_format: Option<String>,
    pub bit_depth: Option<i32>,
    pub color_space: Option<String>,
    pub level: Option<i32>,
    pub real_frame_rate: Option<String>,
    pub average_frame_rate: Option<String>,
    pub sample_aspect_ratio: Option<String>,
    pub display_aspect_ratio: Option<String>,
    pub is_interlaced: bool,
    pub is_default: bool,
    pub is_forced: bool,
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
