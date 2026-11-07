//! `GET /hubs/sections/{id}`: the shelves of a library's "Recommended" tab.
//!
//! The reference builds up to eight shelves for a film library. Most rest on what this
//! server does not have yet: what each user watched (continue watching, recently watched,
//! top unwatched), and orderings by release date or rating. A shelf is served when it can
//! be answered truthfully, and absent otherwise — an invented shelf would show wrong films
//! under a title that promises something.
//!
//! Served, in the reference's order:
//!
//! - **Continue watching**, empty: nothing is in progress, since nothing can be played yet.
//!   The reference shows the shelf empty too when that is the case.
//! - **Recently added**: a film library's films by date added; a series library's new
//!   episodes grouped into their season or series, by the rule `core::recent` measured.
//! - **Recent playlists**, empty, when the client asks for library playlists: there are
//!   none.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::Router;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::Serialize;

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{GqlRecentlyAdded, RecentlyAddedData, RECENTLY_ADDED_QUERY};
use crate::items::{MetadataEntry, LIBRARY_IDENTIFIER};
use crate::language::Language;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/hubs/sections/{sectionId}", get(section_hubs))
}

/// Items per shelf when the client does not say, or says something that is not a number:
/// `count=abc` gets six on the reference.
const DEFAULT_COUNT: u32 = 6;

/// Response to `GET /hubs/sections/{id}`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HubsContainer {
    pub size: u32,
    pub allow_sync: bool,
    pub identifier: &'static str,
    #[serde(rename = "librarySectionID")]
    pub library_section_id: i64,
    pub library_section_title: String,
    #[serde(rename = "librarySectionUUID")]
    pub library_section_uuid: String,
    #[serde(rename = "Hub", skip_serializing_if = "Vec::is_empty")]
    pub hub: Vec<Hub>,
}

/// One shelf.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hub {
    /// The shelf's items as one lookup, `/library/metadata/1,2,3`. Absent on an empty shelf.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub_key: Option<String>,
    /// Where the client goes for the whole list behind the shelf.
    pub key: String,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub hub_identifier: String,
    pub context: &'static str,
    pub size: u32,
    /// Whether the list behind the shelf holds more than the shelf shows.
    pub more: bool,
    pub style: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub promoted: Option<bool>,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<MetadataEntry>,
}

/// The parameters this endpoint reads. The many others Plex Web sends —
/// `includeStations`, `includeExternalMetadata` and the like — ask for content this server
/// does not have, and an absent shelf is already the answer.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubParams {
    count: Option<String>,
    #[serde(rename = "includeLibraryPlaylists")]
    include_library_playlists: Option<String>,
    #[serde(rename = "excludeFields")]
    exclude_fields: Option<String>,
}

impl HubParams {
    fn count(&self) -> u32 {
        self.count
            .as_deref()
            .and_then(|count| count.parse().ok())
            .unwrap_or(DEFAULT_COUNT)
            .min(MAX_ITEMS)
    }

    fn includes_playlists(&self) -> bool {
        self.include_library_playlists.as_deref() == Some("1")
    }

    /// `excludeFields=summary` drops the synopsis from every item, which the reference
    /// honors and Plex Web asks for: a shelf shows no synopsis.
    fn excludes_summary(&self) -> bool {
        self.exclude_fields
            .as_deref()
            .is_some_and(|fields| fields.split(',').any(|field| field == "summary"))
    }
}

/// `GET /hubs/sections/{id}`.
async fn section_hubs(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(params): Query<HubParams>,
    client: ClientIdentity,
    format: ResponseFormat,
) -> Result<PlexResponse<HubsContainer>, ApiError> {
    let count = params.count();
    let language = Language::of(&client);
    let library = recently_added(&state, section, count).await?;
    let is_film_library = library.kind == "MOVIE";

    let mut items: Vec<MetadataEntry> = library
        .recently_added
        .edges
        .iter()
        .filter_map(|edge| {
            let mut item = MetadataEntry::from_graphql(&edge.node, true, language)?;
            // The entry's time, which for a season gathered from a batch is its newest
            // episode's rather than the season's own.
            item.added_at = edge.added_at;
            Some(item)
        })
        .collect();
    if params.excludes_summary() {
        for item in &mut items {
            item.summary = None;
        }
    }
    let shelf = RecentShelf {
        section,
        library_title: &library.title,
        is_film_library,
        more: library.recently_added.page_info.has_next_page,
    };

    let mut hubs = vec![continue_watching(section, is_film_library, language)];
    hubs.push(shelf.hub(items, language));
    if params.includes_playlists() {
        hubs.push(recent_playlists(section, language));
    }

    Ok(PlexResponse::new(
        HubsContainer {
            size: u32::try_from(hubs.len()).unwrap_or(u32::MAX),
            allow_sync: true,
            identifier: LIBRARY_IDENTIFIER,
            library_section_id: section,
            library_section_title: library.title.clone(),
            library_section_uuid: library.uuid.clone(),
            hub: hubs,
        },
        format,
    ))
}

/// The library and its `count` most recently added items, as the official API groups them.
async fn recently_added(
    state: &PlexState,
    section: i64,
    count: u32,
) -> Result<GqlRecentlyAdded, ApiError> {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    variables.insert(Name::new("first"), Value::Number(count.into()));

    let data: RecentlyAddedData =
        gql::execute(&state.schema, &RECENTLY_ADDED_QUERY, variables).await?;
    data.node.ok_or(ApiError::NotFound)
}

fn continue_watching(section: i64, is_film_library: bool, language: Language) -> Hub {
    let (kind, prefix) = if is_film_library {
        ("movie", "movie")
    } else {
        ("episode", "tv")
    };
    Hub {
        hub_key: None,
        key: format!("/hubs/sections/{section}/continueWatching/items"),
        title: language.continue_watching().to_owned(),
        kind,
        hub_identifier: format!("{prefix}.inprogress.{section}"),
        context: if is_film_library {
            "hub.movie.inprogress"
        } else {
            "hub.tv.inprogress"
        },
        size: 0,
        more: false,
        style: "shelf",
        promoted: None,
        metadata: Vec::new(),
    }
}

/// The "recently added" shelf, in the form the library's kind calls for.
struct RecentShelf<'a> {
    section: i64,
    library_title: &'a str,
    is_film_library: bool,
    more: bool,
}

impl RecentShelf<'_> {
    /// A film library's shelf holds films; a series library's mixes series, seasons and
    /// episodes, and its full list lives under the home hub rather than the library listing.
    fn hub(&self, items: Vec<MetadataEntry>, language: Language) -> Hub {
        let section = self.section;
        let size = u32::try_from(items.len()).unwrap_or(u32::MAX);
        let keys: Vec<&str> = items.iter().map(|item| item.rating_key.as_str()).collect();
        let (key, kind, prefix) = if self.is_film_library {
            (
                format!("/library/sections/{section}/all?sort=addedAt:desc"),
                "movie",
                "movie",
            )
        } else {
            (
                format!("/hubs/home/recentlyAdded?type=2&sectionID={section}"),
                "mixed",
                "tv",
            )
        };
        Hub {
            hub_key: (!keys.is_empty()).then(|| format!("/library/metadata/{}", keys.join(","))),
            key,
            title: language.recently_added_in(self.library_title),
            kind,
            hub_identifier: format!("{prefix}.recentlyadded.{section}"),
            context: if self.is_film_library {
                "hub.movie.recentlyadded"
            } else {
                "hub.tv.recentlyadded"
            },
            size,
            more: self.more,
            style: "shelf",
            promoted: Some(true),
            metadata: items,
        }
    }
}

fn recent_playlists(section: i64, language: Language) -> Hub {
    Hub {
        hub_key: None,
        key: format!(
            "/playlists/all?type=15&playlistType=video&sort=lastViewedAt:desc&sectionID={section}"
        ),
        title: language.recent_playlists().to_owned(),
        kind: "playlist",
        hub_identifier: format!("recent.library.playlists.{section}"),
        context: "hub.recent.library.playlists",
        size: 0,
        more: false,
        style: "shelf",
        promoted: None,
        metadata: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(count: Option<&str>) -> HubParams {
        HubParams {
            count: count.map(ToOwned::to_owned),
            ..HubParams::default()
        }
    }

    #[test]
    fn a_count_that_is_not_a_number_gets_six() {
        // Measured: `count=abc` and no `count` both give six items per shelf.
        assert_eq!(params(None).count(), 6);
        assert_eq!(params(Some("abc")).count(), 6);
        assert_eq!(params(Some("12")).count(), 12);
        assert_eq!(params(Some("100000")).count(), MAX_ITEMS);
    }
}
