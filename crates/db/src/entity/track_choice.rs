use sea_orm::entity::prelude::*;

/// The tracks one viewer chose on one file.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "track_choice")]
pub struct Model {
    /// A user's id.
    #[sea_orm(primary_key, auto_increment = false)]
    pub viewer: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub media_part_id: i64,
    pub audio_stream_id: Option<i64>,
    pub subtitle_stream_id: Option<i64>,
    pub subtitle_off: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
