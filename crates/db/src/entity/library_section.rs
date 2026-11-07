use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "library_section")]
pub struct Model {
    /// Assigned by us, not auto-incremented: section ids are part of the API surface and
    /// must survive an import so client-side links keep resolving.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub title_sort: Option<String>,
    pub language: String,
    pub uuid: String,
    /// Epoch seconds. One representation across the whole stack.
    pub created_at: i64,
    pub updated_at: i64,
    pub scanned_at: Option<i64>,
    /// A monotonic change counter rather than a time, which is why it is not an epoch field.
    pub content_changed_at: i64,
    pub hidden: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::section_root::Entity")]
    SectionRoot,
}

impl Related<super::section_root::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SectionRoot.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
