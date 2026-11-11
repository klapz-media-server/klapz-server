use sea_orm::entity::prelude::*;

/// One searchable word of one collection's titles; see `migration::m0027_collection`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "collection_word")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub word: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub collection_id: i64,
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
}

impl ActiveModelBehavior for ActiveModel {}
