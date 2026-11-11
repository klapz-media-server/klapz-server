use sea_orm::entity::prelude::*;

/// One item in one collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "collection_entry")]
pub struct Model {
    /// Ascending in the order items joined.
    #[sea_orm(primary_key)]
    pub id: i64,
    pub collection_id: i64,
    pub item_id: i64,
    pub position: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::collection::Entity",
        from = "Column::CollectionId",
        to = "super::collection::Column::Id",
        on_delete = "Cascade"
    )]
    Collection,
    #[sea_orm(
        belongs_to = "super::metadata_item::Entity",
        from = "Column::ItemId",
        to = "super::metadata_item::Column::Id"
    )]
    Item,
}

impl Related<super::collection::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Collection.def()
    }
}

impl Related<super::metadata_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Item.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
