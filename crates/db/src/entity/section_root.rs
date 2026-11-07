use sea_orm::entity::prelude::*;

/// A filesystem root a section scans.
///
/// Separate from the section rather than a delimited string, because these are compared
/// against real paths on every file request and a packed column cannot be joined or indexed.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "section_root")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub section_id: i64,
    pub path: String,
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::library_section::Entity",
        from = "Column::SectionId",
        to = "super::library_section::Column::Id",
        on_delete = "Cascade"
    )]
    LibrarySection,
}

impl Related<super::library_section::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::LibrarySection.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
