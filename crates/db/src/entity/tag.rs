use open_plex_core::TagKind;
use sea_orm::entity::prelude::*;

/// A genre, a country, or a person in one credit role.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "tag")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub kind: String,
    pub name: String,
    /// `name` with ASCII case folded, the form a filter by name compares in.
    pub name_key: String,
    pub provider_key: Option<String>,
    pub thumb_url: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::item_tag::Entity")]
    ItemTag,
}

impl Related<super::item_tag::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ItemTag.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

/// The stored discriminant of a kind.
pub const fn kind_name(kind: TagKind) -> &'static str {
    match kind {
        TagKind::Genre => "genre",
        TagKind::Country => "country",
        TagKind::Director => "director",
        TagKind::Writer => "writer",
        TagKind::Actor => "actor",
        TagKind::Producer => "producer",
    }
}

/// Reads a stored discriminant back. `None` for one this build does not know, which a row
/// written by a newer version could carry.
pub fn parse_kind(stored: &str) -> Option<TagKind> {
    Some(match stored {
        "genre" => TagKind::Genre,
        "country" => TagKind::Country,
        "director" => TagKind::Director,
        "writer" => TagKind::Writer,
        "actor" => TagKind::Actor,
        "producer" => TagKind::Producer,
        _ => return None,
    })
}
