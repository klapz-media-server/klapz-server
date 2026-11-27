use sea_orm::entity::prelude::*;

/// The value an owner chose for one server setting.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "server_setting")]
pub struct Model {
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
