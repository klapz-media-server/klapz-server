use sea_orm::entity::prelude::*;

/// How far one viewer shifts one subtitle.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "subtitle_offset")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub media_stream_id: i64,
    /// A user's id.
    #[sea_orm(primary_key, auto_increment = false)]
    pub viewer: i64,
    /// Never 0: no offset has no row.
    pub offset_ms: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
