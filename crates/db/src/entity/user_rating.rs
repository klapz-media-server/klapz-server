use sea_orm::entity::prelude::*;

/// The last change one viewer made to its rating of one work.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "user_rating")]
pub struct Model {
    /// A user's id.
    #[sea_orm(primary_key, auto_increment = false)]
    pub viewer: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub guid: String,
    /// Out of ten; NULL once cleared.
    pub rating: Option<f64>,
    pub rated_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
