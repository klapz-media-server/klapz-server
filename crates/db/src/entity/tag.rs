use klapz_core::TagKind;
use sea_orm::entity::prelude::*;

/// A genre, a country, or a person in one credit role.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "tag")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub kind: String,
    /// The name as credited.
    pub name: String,
    /// Set for a credit, null for a genre or a country.
    pub person_id: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::item_tag::Entity")]
    ItemTag,
    #[sea_orm(
        belongs_to = "super::person::Entity",
        from = "Column::PersonId",
        to = "super::person::Column::Id",
        on_delete = "Cascade"
    )]
    Person,
}

impl Related<super::item_tag::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ItemTag.def()
    }
}

impl Related<super::person::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Person.def()
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
