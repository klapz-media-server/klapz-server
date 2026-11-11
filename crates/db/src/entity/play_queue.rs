use sea_orm::entity::prelude::*;

/// A list of what to play, as it was when it was created.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "play_queue")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    #[sea_orm(column_type = "Text")]
    pub user_id: Option<i64>,
    pub source_uri: String,
    pub shuffled: bool,
    pub version: i32,
    /// `None` for an empty queue.
    pub selected_position: Option<i64>,
    pub created_at: i64,
    /// The playlist it plays, when it plays one.
    pub playlist_id: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
