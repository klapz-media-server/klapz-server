use sea_orm::entity::prelude::*;

/// A playlist, owned by one viewer: 0 is the operator.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "playlist")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub owner: i64,
    pub guid: String,
    pub title: String,
    pub sort_key: String,
    pub summary: String,
    /// `video`, `audio` or `photo`.
    pub kind: String,
    /// Set together with `rule_listing` on a smart playlist, and only there.
    pub rule_section_id: Option<i64>,
    pub rule_listing: Option<String>,
    pub source_uri: Option<String>,
    pub title_locked: bool,
    pub summary_locked: bool,
    pub view_count: i64,
    pub last_viewed_at: i64,
    pub added_at: i64,
    pub updated_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
