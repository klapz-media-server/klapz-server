use sea_orm::entity::prelude::*;

/// An item's identifier at an external provider.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_external_id")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub source: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub value: String,
    pub position: i32,
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
