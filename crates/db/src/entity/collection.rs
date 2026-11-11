use sea_orm::entity::prelude::*;

/// A collection of one library; see `migration::m0027_collection`.
#[allow(
    clippy::struct_excessive_bools,
    reason = "one column per lockable field, as a row has no set type; the domain holds a list"
)]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "collection")]
pub struct Model {
    /// Drawn from the items' numbering, never auto-incremented.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub library_section_id: i64,
    /// The kind of item it holds, as `metadata_item.kind` names it.
    pub kind: String,
    pub guid: String,
    pub tag_number: i64,
    pub title: String,
    pub title_sort: Option<String>,
    pub sort_key: String,
    pub summary: String,
    pub content_rating: Option<String>,
    /// Set on a smart collection, and only there.
    pub rule_listing: Option<String>,
    pub source_uri: Option<String>,
    /// `default`, `hidden`, `instead` or `with`.
    pub mode: String,
    /// `released`, `title` or `custom`.
    pub item_order: String,
    pub title_locked: bool,
    pub title_sort_locked: bool,
    pub summary_locked: bool,
    pub content_rating_locked: bool,
    pub poster_locked: bool,
    pub background_locked: bool,
    pub thumb_url: Option<String>,
    pub art_url: Option<String>,
    pub added_at: i64,
    pub updated_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::collection_entry::Entity")]
    Entry,
}

impl Related<super::collection_entry::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Entry.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
