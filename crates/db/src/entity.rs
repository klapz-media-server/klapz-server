//! `SeaORM` entities for our own schema.
//!
//! Deliberately not a copy of Plex's schema: `docs/plex-db-schema.md` records that one as a
//! reference, including the parts we reject — packed key/value columns, denormalized display
//! columns, and a column named `index`.
//!
//! These types never leave this crate. The repository maps them to domain types, which is
//! what keeps `core` independent of storage.

pub mod item_external_id;
pub mod item_rating;
pub mod item_tag;
pub mod item_title_word;
pub mod library_section;
pub mod media;
pub mod media_part;
pub mod media_stream;
pub mod metadata_item;
pub mod person;
pub mod playback_session;
pub mod section_root;
pub mod tag;
