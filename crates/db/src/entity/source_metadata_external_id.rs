use sea_orm::entity::prelude::*;

/// An identifier a record has at a provider, its own provider's included.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "source_metadata_external_id")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub source_metadata_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub source: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub value: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
