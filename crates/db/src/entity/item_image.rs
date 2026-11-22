use sea_orm::entity::prelude::*;

/// The image of one kind an item shows.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_image")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: i64,
    /// `core::source_metadata::ImageKind`, as `as_str` spells it.
    #[sea_orm(primary_key, auto_increment = false)]
    pub kind: String,
    pub file: String,
    /// When the item took it: what a client's cached copy is told apart by.
    pub chosen_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::image_file::Entity",
        from = "Column::File",
        to = "super::image_file::Column::File"
    )]
    ImageFile,
}

impl Related<super::image_file::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ImageFile.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
