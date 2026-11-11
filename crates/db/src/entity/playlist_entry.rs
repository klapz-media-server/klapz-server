use sea_orm::entity::prelude::*;

/// One item at one place in a playlist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "playlist_entry")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub playlist_id: i64,
    pub item_id: i64,
    pub position: i64,
    pub added_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::metadata_item::Entity",
        from = "Column::ItemId",
        to = "super::metadata_item::Column::Id"
    )]
    Item,
}

impl Related<super::metadata_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Item.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
