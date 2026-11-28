use sea_orm::entity::prelude::*;

/// The value an owner chose for one library in place of the server's.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "library_setting")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub library_section_id: i64,
    /// The setting's dotted key.
    #[sea_orm(primary_key, auto_increment = false)]
    pub name: String,
    pub value: String,
    pub updated_at: i64,
    /// A user's id.
    pub updated_by: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
