use sea_orm::entity::prelude::*;

/// One extra of one item: a trailer, a deleted scene, a featurette.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "extra")]
pub struct Model {
    /// Preserved from the source library, where an extra is numbered with the items.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub item_id: i64,
    /// `trailer`, `deleted_scene`, `behind_the_scenes`…
    pub kind: String,
    pub position: Option<i32>,
    pub guid: String,
    pub title: Option<String>,
    pub title_sort: Option<String>,
    pub summary: Option<String>,
    pub year: Option<i32>,
    /// The release date's midnight UTC, in seconds since the Unix epoch.
    pub released_on: Option<i64>,
    pub duration_ms: Option<i64>,
    pub added_at: Option<i64>,
    pub updated_at: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::metadata_item::Entity",
        from = "Column::ItemId",
        to = "super::metadata_item::Column::Id",
        on_delete = "Cascade"
    )]
    MetadataItem,
}

impl ActiveModelBehavior for ActiveModel {}
