use sea_orm::entity::prelude::*;

/// A record's age rating in one country.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "source_metadata_certification")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub source_metadata_id: i64,
    /// ISO 3166-1 alpha-2.
    #[sea_orm(primary_key, auto_increment = false)]
    pub country: String,
    pub rating: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
