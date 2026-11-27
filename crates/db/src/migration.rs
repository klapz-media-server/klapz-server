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
mod m0022_viewing_history;
mod m0023_session_termination;
mod m0025_tag_word;
mod m0026_marker;
mod m0027_collection;
mod m0028_play_queue_edit;
mod m0029_release_midnight;
mod m0030_every_caller_a_user;
mod m0031_title_key;
mod m0032_folded_title;
mod m0033_extras;
mod m0034_user_rating;
mod m0035_ultrablur;
mod m0036_subtitle_offset;
mod m0037_tree_fields;
mod m0038_chapter;
mod m0039_extension_device;
mod m0040_stated_probe;
mod m0041_user_favorite;
mod m0042_source_metadata;
mod m0043_artwork;
mod m0044_media_analysis;
mod m0045_teletext_pages;
mod m0046_teletext_tracks;
mod m0047_server_setting;
mod m0048_extension_token;
mod m0049_canonical_languages;

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
            Box::new(m0022_viewing_history::Migration),
            Box::new(m0023_session_termination::Migration),
            Box::new(m0025_tag_word::Migration),
            Box::new(m0026_marker::Migration),
            Box::new(m0027_collection::Migration),
            Box::new(m0028_play_queue_edit::Migration),
            Box::new(m0029_release_midnight::Migration),
            Box::new(m0030_every_caller_a_user::Migration),
            Box::new(m0031_title_key::Migration),
            Box::new(m0032_folded_title::Migration),
            Box::new(m0033_extras::Migration),
            Box::new(m0034_user_rating::Migration),
            Box::new(m0035_ultrablur::Migration),
            Box::new(m0036_subtitle_offset::Migration),
            Box::new(m0037_tree_fields::Migration),
            Box::new(m0038_chapter::Migration),
            Box::new(m0039_extension_device::Migration),
            Box::new(m0040_stated_probe::Migration),
            Box::new(m0041_user_favorite::Migration),
            Box::new(m0042_source_metadata::Migration),
            Box::new(m0043_artwork::Migration),
            Box::new(m0044_media_analysis::Migration),
            Box::new(m0045_teletext_pages::Migration),
            Box::new(m0046_teletext_tracks::Migration),
            Box::new(m0047_server_setting::Migration),
            Box::new(m0048_extension_token::Migration),
            Box::new(m0049_canonical_languages::Migration),
        ]
    }
}
