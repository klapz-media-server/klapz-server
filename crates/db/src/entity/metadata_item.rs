use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "metadata_item")]
pub struct Model {
    /// Preserved from the source library: ids are part of the API surface.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub library_section_id: i64,
    /// The season for an episode, the show for a season, null for a movie or a show.
    pub parent_id: Option<i64>,
    pub kind: String,
    pub guid: String,
    /// Absent for almost every season; the source stores none.
    pub title: Option<String>,
    pub title_sort: Option<String>,
    /// What the listing orders by and what search matches: the sort title, lowercased.
    ///
    /// Folded rather than raw so one index serves both ordering and case-insensitive prefix
    /// search — and because case-insensitive ordering is what a reader of a title list
    /// expects anyway.
    pub sort_key: String,
    pub original_title: Option<String>,
    pub summary: Option<String>,
    pub tagline: Option<String>,
    pub studio: Option<String>,
    pub content_rating: Option<String>,
    pub content_rating_age: Option<i32>,
    pub position: Option<i32>,
    pub year: Option<i32>,
    pub released_on: Option<String>,
    pub duration_ms: Option<i64>,
    pub rating: Option<f64>,
    pub audience_rating: Option<f64>,
    pub added_at: i64,
    pub updated_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::library_section::Entity",
        from = "Column::LibrarySectionId",
        to = "super::library_section::Column::Id",
        on_delete = "Cascade"
    )]
    LibrarySection,
    #[sea_orm(has_many = "super::media::Entity")]
    Media,
    /// The self-join that walks up the hierarchy: an episode to its season, a season to its
    /// show.
    #[sea_orm(
        belongs_to = "Entity",
        from = "Column::ParentId",
        to = "Column::Id",
        on_delete = "Cascade"
    )]
    Parent,
}

impl Related<super::library_section::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::LibrarySection.def()
    }
}

impl Related<super::media::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Media.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
