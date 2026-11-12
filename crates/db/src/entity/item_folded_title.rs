use sea_orm::entity::prelude::*;

/// One item's title as the voice search compares it; see `migration::m0031_folded_title`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_folded_title")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: i64,
    pub library_section_id: i64,
    pub kind: String,
    pub folded_title: String,
    /// In the units `core::closest` measures, which are not characters nor bytes.
    pub folded_length: i32,
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
