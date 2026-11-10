use sea_orm::entity::prelude::*;

/// One field locked on one item.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_field_lock")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: i64,
    /// Our name for the field, as `write::field_name` spells it.
    #[sea_orm(primary_key, auto_increment = false)]
    pub field: String,
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
