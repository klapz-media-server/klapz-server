//! Domain layer: library entities, and the ports storage must implement.
//!
//! Nothing here knows about HTTP, XML, or Plex naming. A binding translates these types into
//! whatever its protocol expects, so adding a second binding costs no change in this crate.

pub mod access;
pub mod auth;
pub mod identity;
pub mod ids;
pub mod library;
pub mod metadata;
pub mod page;
pub mod play_queue;
pub mod playback;
pub mod recent;
pub mod search;
pub mod similar;

#[cfg(feature = "testing")]
pub mod testing;

pub use auth::{Accounts, Credential, Principal, Scope};
pub use identity::{
    now_epoch, ServerCapabilities, ServerFeatures, ServerIdentity, TranscoderCapabilities,
};
pub use ids::{MediaId, PartId, PersonId, RatingKey, SectionId, StreamId, TagId, UserId};
pub use library::{LibraryError, LibraryRepository, LibrarySection, SectionKind, SectionRoot};
pub use metadata::{
    Credit, EpisodeCredit, ExternalId, ItemFilter, ItemKind, ItemQuery, ItemSort, ItemTag,
    MediaFile, MediaStream, MediaVersion, MetadataError, MetadataItem, MetadataRepository, Person,
    Rating, RatingKind, SearchQuery, StreamKind, Tag, TagCondition, TagKind, TagRef,
};
pub use page::{Bound, Cursor, CursorKey, Direction, Page, Window, WindowError};
