use sea_orm::entity::prelude::*;

/// A work as one provider describes it: a film, a series, a season or an episode.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "source_metadata")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    /// `tmdb`, `tvdb`.
    pub agent: String,
    pub kind: String,
    pub provider_id: String,
    /// The series of a season, the season of an episode.
    pub parent_id: Option<i64>,
    pub position: Option<i32>,
    pub original_language: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub released_on: Option<i64>,
    pub duration_ms: Option<i64>,
    pub studio: Option<String>,
    pub rating: Option<f64>,
    pub refreshed_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
