use sea_orm::entity::prelude::*;

/// One marker on one item: the work's, or a viewer's bookmark.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_marker")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub item_id: i64,
    /// Whose bookmark: a user's id. Null for the work's own markers.
    pub owner: Option<i64>,
    /// `intro`, `credits`, `commercial` or `bookmark`.
    pub kind: String,
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    pub is_final: bool,
    pub version: Option<i32>,
    pub title: Option<String>,
    pub color: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
