//! Metadata agents: what klapz asks of TMDB, TVDB and the providers that follow, and the keys it
//! asks with. Each implements `klapz_core::source_metadata::MetadataSource`, which searches and
//! fetches; what is done with the answers is decided in `core`.

mod credentials;
mod http;
mod language;
mod tmdb;
mod tvdb;

pub use credentials::{KeySource, ProviderCredentials, TmdbCredentials, TvdbCredentials};
pub use tmdb::Tmdb;
pub use tvdb::Tvdb;
