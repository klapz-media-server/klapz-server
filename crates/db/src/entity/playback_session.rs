use sea_orm::entity::prelude::*;

/// One playback in progress.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "playback_session")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    #[sea_orm(unique)]
    pub session_key: String,
    pub playback_id: Option<String>,
    pub item_id: i64,
    pub part_id: i64,
    pub client: Option<String>,
    pub product: Option<String>,
    pub state: String,
    pub video_decision: String,
    pub audio_decision: String,
    pub position_ms: i64,
    pub started_at: i64,
    pub updated_at: i64,
    pub expires_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
