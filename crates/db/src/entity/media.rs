use sea_orm::entity::prelude::*;

/// One encoding of an item. Several when the library holds more than one cut.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "media")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub metadata_item_id: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub aspect_ratio: Option<f64>,
    pub duration_ms: Option<i64>,
    pub bitrate: Option<i32>,
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<i32>,
    /// The measured rate, e.g. 23.976. The Plex surface turns it into a label like `24p`.
    pub frame_rate: Option<f64>,
    pub interlaced: bool,
    pub optimized_for_streaming: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::metadata_item::Entity",
        from = "Column::MetadataItemId",
        to = "super::metadata_item::Column::Id",
        on_delete = "Cascade"
    )]
    MetadataItem,
    #[sea_orm(has_many = "super::media_part::Entity")]
    MediaPart,
}

impl Related<super::metadata_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MetadataItem.def()
    }
}

impl Related<super::media_part::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MediaPart.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
