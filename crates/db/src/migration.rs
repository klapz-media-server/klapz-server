//! Schema migrations.
//!
//! Forward-only and never edited once shipped: an edited migration produces two different
//! schemas bearing the same version, which is the hardest class of data bug to diagnose.

mod m0001_library;
mod m0002_metadata;
mod m0003_top_level_index;
mod m0004_title_word;
mod m0005_tags;
mod m0006_references;

use sea_orm_migration::prelude::*;

#[derive(Debug)]
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m0001_library::Migration),
            Box::new(m0002_metadata::Migration),
            Box::new(m0003_top_level_index::Migration),
            Box::new(m0004_title_word::Migration),
            Box::new(m0005_tags::Migration),
            Box::new(m0006_references::Migration),
        ]
    }
}
