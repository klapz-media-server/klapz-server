use sea_orm::entity::prelude::*;

/// One whole title of one item, folded, as a search ranks it.
///
/// Keyed by item first: ranking looks up the keys of a row already matched, never searches
/// them. See `open_plex_core::search::title_keys` and `migration::m0031_title_key`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_title_key")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub key: String,
    /// Read for a title equal to the term: the title or the sort title.
    pub exact: bool,
    /// Read for a title opening with the term: the title or the original title.
    pub leading: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::metadata_item::Entity",
        from = "Column::ItemId",
        to = "super::metadata_item::Column::Id",
        on_delete = "Cascade"
    )]
    MetadataItem,
}

impl ActiveModelBehavior for ActiveModel {}
