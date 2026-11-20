use klapz_core::RatingKind;
use sea_orm::entity::prelude::*;

/// A score an external provider gives an item.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "item_rating")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub source: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub kind: String,
    pub value: f64,
    pub image: String,
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

/// The stored discriminant of a kind.
pub const fn kind_name(kind: RatingKind) -> &'static str {
    match kind {
        RatingKind::Critic => "critic",
        RatingKind::Audience => "audience",
    }
}

/// Reads a stored discriminant back. `None` for one this build does not know.
pub fn parse_kind(stored: &str) -> Option<RatingKind> {
    match stored {
        "critic" => Some(RatingKind::Critic),
        "audience" => Some(RatingKind::Audience),
        _ => None,
    }
}
