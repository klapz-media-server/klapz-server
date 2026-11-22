use sea_orm::entity::prelude::*;

/// A record's texts in one language.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "source_metadata_translation")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub source_metadata_id: i64,
    /// `fr-FR`, or `fr` when the provider names no region.
    #[sea_orm(primary_key, auto_increment = false)]
    pub language: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub tagline: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
