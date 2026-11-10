use sea_orm::entity::prelude::*;

/// The Dolby Vision configuration of a video track.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "stream_dolby_vision")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub stream_id: i64,
    /// The configuration record's version, as written: `1.0`.
    pub version: String,
    pub profile: i32,
    pub level: i32,
    /// Which format the base layer plays as without Dolby Vision: 1 HDR10, 2 SDR, 4 HLG,
    /// 6 Blu-ray HDR10. Null when the record does not say.
    pub base_layer_compatibility: Option<i32>,
    pub has_base_layer: bool,
    pub has_enhancement_layer: bool,
    /// Whether the track carries the reference processing unit, the dynamic metadata.
    pub has_rpu: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::media_stream::Entity",
        from = "Column::StreamId",
        to = "super::media_stream::Column::Id",
        on_delete = "Cascade"
    )]
    MediaStream,
}

impl Related<super::media_stream::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MediaStream.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
