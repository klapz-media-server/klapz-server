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
mod m0007_added_index;
mod m0008_playback_session;
mod m0009_play_queue;
mod m0010_playback_id;
mod m0011_playback_output;
mod m0012_access;
mod m0013_extension_enabled;
mod m0014_extension_account;
mod m0015_stream_analysis;
mod m0016_watch_state;
mod m0017_guid_index;
mod m0018_track_choice;
mod m0019_field_lock;
mod m0020_sign_in;
mod m0021_playlist;

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
            Box::new(m0007_added_index::Migration),
            Box::new(m0008_playback_session::Migration),
            Box::new(m0009_play_queue::Migration),
            Box::new(m0010_playback_id::Migration),
            Box::new(m0011_playback_output::Migration),
            Box::new(m0012_access::Migration),
            Box::new(m0013_extension_enabled::Migration),
            Box::new(m0014_extension_account::Migration),
            Box::new(m0015_stream_analysis::Migration),
            Box::new(m0016_watch_state::Migration),
            Box::new(m0017_guid_index::Migration),
            Box::new(m0018_track_choice::Migration),
            Box::new(m0019_field_lock::Migration),
            Box::new(m0020_sign_in::Migration),
            Box::new(m0021_playlist::Migration),
        ]
    }
}
