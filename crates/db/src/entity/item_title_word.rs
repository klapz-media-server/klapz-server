use sea_orm::entity::prelude::*;

/// One searchable word of one item's titles.
///
/// The composite primary key is `(word, item_id)` so a prefix becomes a single range scan;
/// see `migration::m0004_title_word` for why this is a plain table rather than a full-text
/// index.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_title_word")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub word: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: i64,
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
