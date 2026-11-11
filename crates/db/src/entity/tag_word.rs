use sea_orm::entity::prelude::*;

/// One searchable word of one tag's name; see `migration::m0025_tag_word`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "tag_word")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub word: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub tag_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::tag::Entity",
        from = "Column::TagId",
        to = "super::tag::Column::Id",
        on_delete = "Cascade"
    )]
    Tag,
}

impl ActiveModelBehavior for ActiveModel {}
