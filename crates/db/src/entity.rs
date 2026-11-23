//! `SeaORM` entities for our own schema.
//!
//! Deliberately not a copy of Plex's schema: `docs/plex-db-schema.md` records that one as a
//! reference, including the parts we reject — packed key/value columns, denormalized display
//! columns, and a column named `index`.
//!
//! These types never leave this crate. The repository maps them to domain types, which is
//! what keeps `core` independent of storage.

pub mod collection;
pub mod collection_entry;
pub mod collection_word;
pub mod extension_account;
pub mod extension_device;
pub mod extra;
pub mod extra_media;
pub mod extra_track;
pub mod image_file;
pub mod item_chapter;
pub mod item_corner_colors;
pub mod item_external_id;
pub mod item_field_lock;
pub mod item_folded_title;
pub mod item_image;
pub mod item_marker;
pub mod item_rating;
pub mod item_source;
pub mod item_tag;
pub mod item_title_key;
pub mod item_title_word;
pub mod library_permission;
pub mod library_section;
pub mod media;
pub mod media_analysis;
pub mod media_part;
pub mod media_stream;
pub mod metadata_item;
pub mod part_side_stream;
pub mod person;
pub mod play_queue;
pub mod play_queue_entry;
pub mod play_queue_source;
pub mod playback_session;
pub mod playlist;
pub mod playlist_entry;
pub mod section_root;
pub mod source_metadata;
pub mod source_metadata_certification;
pub mod source_metadata_external_id;
pub mod source_metadata_image;
pub mod source_metadata_translation;
pub mod stream_dolby_vision;
pub mod stream_loudness;
pub mod subtitle_offset;
pub mod tag;
pub mod tag_word;
pub mod track_choice;
pub mod user_extension;
pub mod user_favorite;
pub mod user_password;
pub mod user_rating;
pub mod users;
pub mod viewing;
pub mod viewing_history;
pub mod watch_state;
