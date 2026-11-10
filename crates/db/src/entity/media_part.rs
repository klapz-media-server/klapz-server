use sea_orm::entity::prelude::*;

/// One file on disk. Almost always exactly one per media — 244,064 of 244,092 in the
/// reference library — but a stacked release has several.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "media_part")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub media_id: i64,
    pub position: i32,
    /// The only column in the schema that becomes a filesystem access. Never exposed by the
    /// API, and never taken from a client.
    pub path: String,
    pub size_bytes: Option<i64>,
    pub duration_ms: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    /// An embedded cover image.
    pub has_thumbnail: bool,
    /// Whether an MP4 uses 64-bit chunk offsets. Null when the container is not MP4.
    #[sea_orm(column_name = "has_64bit_offsets")]
    pub has_64bit_offsets: Option<bool>,
    /// Whether an MP4's index precedes its data. Null when the container is not MP4.
    pub optimized_for_streaming: Option<bool>,
    /// The transport packet size of an MPEG-TS file: 188, or 192 for Blu-ray's M2TS.
    pub packet_length: Option<i32>,
    pub has_chapter_text_stream: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::media::Entity",
        from = "Column::MediaId",
        to = "super::media::Column::Id",
        on_delete = "Cascade"
    )]
    Media,
    #[sea_orm(has_many = "super::media_stream::Entity")]
    MediaStream,
}

impl Related<super::media::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Media.def()
    }
}

impl Related<super::media_stream::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MediaStream.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
