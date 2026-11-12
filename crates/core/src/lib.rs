//! Domain layer: library entities, and the ports storage must implement.
//!
//! Nothing here knows about HTTP, XML, or Plex naming. A binding translates these types into
//! whatever its protocol expects, so adding a second binding costs no change in this crate.

pub mod access;
pub mod account;
pub mod auth;
pub mod closest;
pub mod collection;
pub mod common;
pub mod completion;
pub mod credited;
pub mod edit;
pub mod history;
pub mod identity;
pub mod ids;
pub mod library;
pub mod lifecycle;
pub mod marker;
pub mod metadata;
pub mod page;
pub mod play_queue;
pub mod playback;
pub mod playlist;
pub mod recent;
pub mod search;
pub mod similar;
pub mod slug;
pub mod stream;
pub mod tracks;
pub mod watch;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use access::ServerRole;
pub use auth::{Principal, PublishedServer};
pub use identity::{
    now_epoch, ServerCapabilities, ServerFeatures, ServerIdentity, TranscoderCapabilities,
};
pub use ids::{
    CollectionId, HistoryId, MarkerId, MediaId, PartId, PersonId, PlaylistEntryId, PlaylistId,
    RatingKey, SectionId, StreamId, TagId, UserId,
};
pub use library::{LibraryError, LibraryRepository, LibrarySection, SectionKind, SectionRoot};
pub use metadata::{
    Credit, EpisodeCredit, ExternalId, FieldValue, ItemFilter, ItemKind, ItemQuery, ItemSort,
    ItemTag, Listed, MediaFile, MediaStream, MediaVersion, MetadataError, MetadataItem,
    MetadataRepository, Person, Rating, RatingKind, SearchQuery, StreamKind, Tag, TagCondition,
    TagKind, TagRef, TagScope, TagSearch, TagUse, ValueField,
};
pub use page::{Bound, Cursor, CursorKey, Direction, Page, Window, WindowError};
