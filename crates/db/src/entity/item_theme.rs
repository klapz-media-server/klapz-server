use sea_orm::entity::prelude::*;

/// A series' theme music, kept in the store.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_theme")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: i64,
    /// `core::source_metadata::theme::ThemeSource`, as `as_str` spells it.
    pub source: String,
    /// The service's address, or the path of the library file it was copied from.
    pub origin: String,
    /// The store's name for it: `themes/ab/abcd….mp3`.
    pub file: String,
    pub content_type: String,
    pub size_bytes: i64,
    /// When the item took it: what a client's cached copy is told apart by.
    pub chosen_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
