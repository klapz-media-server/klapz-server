//! A user's libraries, as Jellyfin's views: one `CollectionFolder` per library it may read.

use axum::extract::{Path, RawQuery, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::dto::ItemsResult;
use crate::error::JellyfinError;
use crate::gql;
use crate::id;
use crate::query::Query;
use crate::response::json;
use crate::time;
use crate::JellyfinState;

pub fn router() -> Router<JellyfinState> {
    Router::new()
        .route("/userviews", get(user_views))
        .route("/users/{user}/views", get(legacy_user_views))
        .route("/library/mediafolders", get(media_folders))
}

const LIBRARIES: &str =
    "{ libraries(first: 100) { nodes { id kind title titleSort createdAt stats { itemCount } } } }";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub title_sort: Option<String>,
    pub created_at: String,
    pub stats: LibraryStats,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub item_count: i64,
}

#[derive(Deserialize)]
struct LibrariesData {
    libraries: Nodes,
}

#[derive(Deserialize)]
struct Nodes {
    nodes: Vec<Library>,
}

/// The folder every view hangs from, which Jellyfin calls the user's root. One for the whole
/// server: each user's views are what it may read.
pub fn user_root_id() -> String {
    id::of(klapz_api::server::SERVER_TYPE, 0).unwrap_or_default()
}

/// Jellyfin's collection type for a kind of library.
pub fn collection_type(kind: &str) -> &'static str {
    match kind {
        "MOVIE" => "movies",
        "SHOW" => "tvshows",
        "ARTIST" => "music",
        _ => "photos",
    }
}

/// A library as a `CollectionFolder`.
pub fn library_dto(library: &Library, server_id: &str) -> Value {
    let jellyfin_id = id::of_global(&library.id).unwrap_or_default();
    json!({
        "Name": library.title,
        "ServerId": server_id,
        "Id": jellyfin_id,
        "DateCreated": time::date(&library.created_at),
        "CanDelete": false,
        "CanDownload": false,
        "SortName": library
            .title_sort
            .clone()
            .unwrap_or_else(|| library.title.clone())
            .to_lowercase(),
        "ExternalUrls": [],
        "EnableMediaSourceDisplay": true,
        "ChannelId": null,
        "Taglines": [],
        "Genres": [],
        "PlayAccess": "Full",
        "RemoteTrailers": [],
        "ProviderIds": {},
        "IsFolder": true,
        "ParentId": user_root_id(),
        "Type": "CollectionFolder",
        "People": [],
        "Studios": [],
        "GenreItems": [],
        "LocalTrailerCount": 0,
        "UserData": {
            "PlaybackPositionTicks": 0,
            "PlayCount": 0,
            "IsFavorite": false,
            "Played": false,
            "Key": id::dashed(&jellyfin_id),
            "ItemId": jellyfin_id,
        },
        "ChildCount": library.stats.item_count,
        "SpecialFeatureCount": 0,
        "DisplayPreferencesId": jellyfin_id,
        "Tags": [],
        "CollectionType": collection_type(&library.kind),
        "ImageTags": {},
        "BackdropImageTags": [],
        "ImageBlurHashes": {},
        "Chapters": [],
        "LocationType": "FileSystem",
        "MediaType": "Unknown",
        "LockedFields": [],
        "LockData": false,
    })
}

/// The caller's libraries. Music and photos are held nowhere here, so their libraries, empty,
/// are left out.
pub async fn libraries(state: &JellyfinState) -> Result<Vec<Library>, JellyfinError> {
    let data: LibrariesData = gql::query(&state.schema, LIBRARIES, Value::Null).await?;
    Ok(data
        .libraries
        .nodes
        .into_iter()
        .filter(|library| matches!(library.kind.as_str(), "MOVIE" | "SHOW"))
        .collect())
}

async fn views(state: &JellyfinState, user: Option<&str>) -> Result<Response, JellyfinError> {
    let viewer = gql::viewer(&state.schema).await?;
    viewer.require_self(user)?;
    let mut items: Vec<Value> = libraries(state)
        .await?
        .iter()
        .map(|library| library_dto(library, &state.server.id))
        .collect();
    // The reference lists a view of the user's playlists once there is one, among the
    // libraries by name.
    let playlists = crate::playlists::playlists(state, None).await?;
    if !playlists.is_empty() {
        items.push(crate::playlists::view_dto(
            &state.server.id,
            playlists.len(),
        ));
    }
    let collections = crate::collections::collections(state, None).await?;
    if !collections.is_empty() {
        items.push(crate::collections::view_dto(
            &state.server.id,
            collections.len(),
        ));
    }
    items.sort_by_key(|view| view["Name"].as_str().unwrap_or_default().to_lowercase());
    Ok(json(&ItemsResult {
        total_record_count: i64::try_from(items.len()).unwrap_or(i64::MAX),
        items,
        start_index: 0,
    }))
}

async fn user_views(
    State(state): State<JellyfinState>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(query.as_deref());
    views(&state, query.get("userId")).await
}

async fn legacy_user_views(
    State(state): State<JellyfinState>,
    Path(user): Path<String>,
) -> Result<Response, JellyfinError> {
    views(&state, Some(&user)).await
}

async fn media_folders(State(state): State<JellyfinState>) -> Result<Response, JellyfinError> {
    views(&state, None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn films() -> Library {
        Library {
            id: klapz_api::GlobalId::encode("Library", 1),
            kind: "MOVIE".to_owned(),
            title: "Films".to_owned(),
            title_sort: None,
            created_at: "2026-11-20T08:31:54Z".to_owned(),
            stats: LibraryStats { item_count: 3 },
        }
    }

    #[test]
    fn a_library_is_a_collection_folder_under_the_user_root() {
        let dto = library_dto(&films(), "s");
        assert_eq!(dto["Type"], "CollectionFolder");
        assert_eq!(dto["CollectionType"], "movies");
        assert_eq!(dto["Id"], id::of("Library", 1).unwrap_or_default());
        assert_eq!(dto["ParentId"], user_root_id());
        assert_eq!(dto["ChildCount"], 3);
        assert_eq!(dto["SortName"], "films");
        assert_eq!(dto["DateCreated"], "2026-11-20T08:31:54.0000000Z");
        assert_eq!(dto["ImageTags"], json!({}));
    }

    #[test]
    fn each_kind_of_library_has_its_collection_type() {
        assert_eq!(collection_type("MOVIE"), "movies");
        assert_eq!(collection_type("SHOW"), "tvshows");
        assert_eq!(collection_type("ARTIST"), "music");
    }
}
