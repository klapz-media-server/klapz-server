use sea_orm::entity::prelude::*;

/// An image a provider offers for a record, known by its address.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "source_metadata_image")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub source_metadata_id: i64,
    /// `core::source_metadata::ImageKind`, as `as_str` spells it.
    #[sea_orm(primary_key, auto_increment = false)]
    pub kind: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub url: String,
    /// `None` for an image without text.
    pub language: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i32>,
    /// The provider's order.
    pub position: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
