//! The official open-plex API: GraphQL.
//!
//! This is the project's primary interface, and the Plex REST surface is an adapter that
//! executes queries against this schema. That ordering is deliberate: it makes it impossible
//! for the Plex adapter to expose something the official API cannot, because the adapter has
//! no other source of data.
//!
//! Like any binding, this crate owns its wire types and maps them from `core`. It depends on
//! `core` and never on `open-plex-db`.

pub mod access;
pub mod cursor;
pub mod filter;
pub mod item;
pub mod library;
pub mod loader;
pub mod media;
pub mod node;
pub mod play_queue;
pub mod playback;
pub mod schema;
pub mod server;
pub mod tag;
pub mod watch;

pub use cursor::{CursorError, OpaqueCursor};
pub use filter::{ItemFilterInput, ItemOrder, ItemType, SortDirection};
pub use item::{Episode, Item, ItemConnection, Movie, Season, Show};
pub use library::{Library, LibraryConnection, LibraryKind, Root};
pub use media::{File, Track, Version};
pub use node::{GlobalId, GlobalIdError};
pub use schema::{build_schema, ApiContext, ApiSchema, Node, Query};
pub use server::{Features, Server, Transcoder};
pub use tag::{
    Attachment, ExternalIdObject, ItemTags, PersonObject, RatingObject, RatingType, TagFilterInput,
    TagObject, TagType,
};
