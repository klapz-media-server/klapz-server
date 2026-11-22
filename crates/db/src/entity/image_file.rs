use sea_orm::entity::prelude::*;

/// A file the artwork store wrote, with what was worked out from it.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "image_file")]
pub struct Model {
    /// The store's name for it: `provider/ab/abcd….jpg`.
    #[sea_orm(primary_key, auto_increment = false)]
    pub file: String,
    pub source_url: String,
    pub content_type: String,
    pub width: i32,
    pub height: i32,
    pub size_bytes: i64,
    pub blur_hash: String,
    pub stored_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
