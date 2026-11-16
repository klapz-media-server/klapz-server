use sea_orm::entity::prelude::*;

/// One version of an extra, with its one file.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "extra_media")]
pub struct Model {
    /// The version's id in the source, numbered with the items' media.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub extra_id: i64,
    /// The file's id in the source, numbered with the items' files.
    pub part_id: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub aspect_ratio: Option<f64>,
    pub duration_ms: Option<i64>,
    /// Bits per second.
    pub bitrate: Option<i32>,
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<i32>,
    pub optimized_for_streaming: bool,
    /// A file on disk, for a local extra. Never taken from a client.
    pub path: Option<String>,
    pub size_bytes: Option<i64>,
    /// Where a provider streams it from, for an extra that is no file.
    pub stream_url: Option<String>,
    /// What the file's own analysis says, apart from the version's flag.
    pub file_optimized_for_streaming: Option<bool>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::extra::Entity",
        from = "Column::ExtraId",
        to = "super::extra::Column::Id",
        on_delete = "Cascade"
    )]
    Extra,
}

impl ActiveModelBehavior for ActiveModel {}
