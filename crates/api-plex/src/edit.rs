//! Editing items, and locking their fields: `PUT /library/sections/{sectionId}/all`, which
//! Plex Web's metadata editor sends, and `PUT /library/metadata/{ids}`, the spec's.
//!
//! The editor sends `type`, `id` and, per changed field, `{field}.value` and `{field}.locked`;
//! it locks a field itself when the user types into it. Measured on the reference's test
//! libraries:
//!
//! - a value does not lock its field; `.locked=1` locks, `.locked=0` unlocks, anything else
//!   is a `400`, as is a field name the reference does not know, and a missing `type`;
//! - `originallyAvailableAt` locks the year with it on a film or a series, where
//!   `year.locked` does nothing; on an episode the two lock apart;
//! - an id of another type or another library, or one that names nothing, is ignored;
//! - the answer is an empty `200`.
//!
//! `/library/metadata/{ids}` reads the same fields, and also takes a field bare, `title=…`, as
//! the spec writes it, `.value` winning when both come. It edits every item it names whatever
//! its type or library, `type` ignored, and differs again in three answers: a lock other than
//! `0` and `1` is ignored, a request with no parameter is a `400`, and one naming no item a
//! `404`.
//!
//! Each item is edited through the official API's `updateMovie`, `updateShow` or
//! `updateEpisode`. What the reference does and we cannot is refused with `501`: an edit with
//! no `id`, which the reference applies to every item of the library; tags, artwork, and the
//! other fields we do not model; seasons and music. A value the field cannot hold is a `400`
//! where the reference stores it or fails with a `500`.
//!
//! A collection, `type=18`, is edited through `updateCollection`: its title, sort title,
//! summary and content rating, and the locks of those and of its artwork. The reference takes
//! every item field on a collection; the others answer `501` here.

use async_graphql::Variables;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use open_plex_api::collection::COLLECTION_TYPE;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::Deserialize;
use serde_json::{json, Map, Value as Json};

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::PlexState;

/// The fields Plex locks that the official API holds, in the reference's numbering order,
/// which is the order it lists `Field` elements in: its name, then the `ItemField` value.
const LOCKABLE: [(&str, &str); 20] = [
    ("title", "TITLE"),
    ("titleSort", "TITLE_SORT"),
    ("originalTitle", "ORIGINAL_TITLE"),
    ("studio", "STUDIO"),
    ("rating", "RATING"),
    ("tagline", "TAGLINE"),
    ("summary", "SUMMARY"),
    ("contentRating", "CONTENT_RATING"),
    ("thumb", "POSTER"),
    ("art", "BACKGROUND"),
    ("originallyAvailableAt", "RELEASED_ON"),
    ("year", "YEAR"),
    ("genre", "GENRES"),
    ("collection", "COLLECTIONS"),
    ("director", "DIRECTORS"),
    ("writer", "WRITERS"),
    ("actor", "CAST"),
    ("producer", "PRODUCERS"),
    ("country", "COUNTRIES"),
    ("audienceRating", "AUDIENCE_RATING"),
];

/// Fields the reference locks and we do not model, found by locking every attribute name a
/// client sends. Refused with `501` rather than taken for unknown names.
const UNMODELED: [&str; 16] = [
    "banner",
    "theme",
    "label",
    "similar",
    "mood",
    "style",
    "index",
    "parentIndex",
    "absoluteIndex",
    "contentRatingAge",
    "clearLogo",
    "squareArt",
    "editionTitle",
    "guid",
    "network",
    "userRating",
];

/// The `Field` names of an item's locks, in the reference's order.
pub fn plex_locked_fields(locked: &[String]) -> impl Iterator<Item = &'static str> + '_ {
    LOCKABLE
        .iter()
        .filter(move |(_, field)| locked.iter().any(|name| name == field))
        .map(|(name, _)| *name)
}

/// What a value is written as, by the official API's input field it goes to.
#[derive(Debug, Clone, Copy)]
enum ValueKind {
    Text,
    Year,
    Score,
}

/// The fields a value can be written to: the Plex name, the input field, its kind.
const EDITABLE: [(&str, &str, ValueKind); 11] = [
    ("title", "title", ValueKind::Text),
    ("titleSort", "titleSort", ValueKind::Text),
    ("originalTitle", "originalTitle", ValueKind::Text),
    ("summary", "summary", ValueKind::Text),
    ("tagline", "tagline", ValueKind::Text),
    ("studio", "studio", ValueKind::Text),
    ("contentRating", "contentRating", ValueKind::Text),
    ("year", "year", ValueKind::Year),
    ("originallyAvailableAt", "releasedOn", ValueKind::Text),
    ("rating", "rating", ValueKind::Score),
    ("audienceRating", "audienceRating", ValueKind::Score),
];

/// The parameters a client sends `/library/metadata/{ids}` beside the fields, which it ignores:
/// any other bare name is a field, and one it does not know a `400`.
const CLIENT_PARAMETERS: [&str; 3] = ["type", "id", "includeExternalMedia"];

/// Where the fields of an edit come from, which reads them a little differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// `/library/sections/{sectionId}/all`.
    Section,
    /// `/library/metadata/{ids}`.
    Item,
}

/// The types that can be edited, by Plex's `type` number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditedType {
    Movie,
    Show,
    Episode,
    Collection,
}

/// What a collection holds of the fields above: measured on the reference, which takes
/// `year`, `studio` and the rest on a collection too and publishes them, where a collection
/// here keeps these alone.
const COLLECTION_VALUES: [&str; 4] = ["title", "titleSort", "summary", "contentRating"];
const COLLECTION_LOCKS: [&str; 6] = [
    "title",
    "titleSort",
    "summary",
    "contentRating",
    "thumb",
    "art",
];

impl EditedType {
    fn of(plex_type: &str) -> Result<Self, ApiError> {
        match plex_type.parse::<u32>() {
            Ok(1) => Ok(Self::Movie),
            Ok(2) => Ok(Self::Show),
            Ok(4) => Ok(Self::Episode),
            Ok(18) => Ok(Self::Collection),
            Ok(_) => Err(ApiError::NotImplemented),
            Err(_) => Err(ApiError::BadRequest),
        }
    }

    /// By the official API's type name; another kind of item cannot be edited here.
    fn named(typename: &str) -> Result<Self, ApiError> {
        match typename {
            "Movie" => Ok(Self::Movie),
            "Show" => Ok(Self::Show),
            "Episode" => Ok(Self::Episode),
            "Collection" => Ok(Self::Collection),
            _ => Err(ApiError::NotImplemented),
        }
    }

    const fn typename(self) -> &'static str {
        match self {
            Self::Movie => "Movie",
            Self::Show => "Show",
            Self::Episode => "Episode",
            Self::Collection => "Collection",
        }
    }

    const fn mutation(self) -> &'static str {
        match self {
            Self::Movie => UPDATE_MOVIE,
            Self::Show => UPDATE_SHOW,
            Self::Episode => UPDATE_EPISODE,
            Self::Collection => UPDATE_COLLECTION,
        }
    }

    /// The global id an edited object of this type is named by.
    fn global_id(self, key: i64) -> String {
        match self {
            Self::Collection => GlobalId::encode(COLLECTION_TYPE, key),
            Self::Movie | Self::Show | Self::Episode => GlobalId::encode(ITEM_TYPE, key),
        }
    }

    /// Whether a field the reference edits is one this type holds here.
    fn holds(self, name: &str, of_collection: &[&str]) -> bool {
        self != Self::Collection || of_collection.contains(&name)
    }

    /// A film's or a series' year is its release date's, so the two lock together.
    const fn year_follows_date(self) -> bool {
        matches!(self, Self::Movie | Self::Show)
    }
}

const UPDATE_MOVIE: &str = r"
mutation PlexEditMovie($input: UpdateMovieInput!) {
  result: updateMovie(input: $input) { userErrors { code } }
}";
const UPDATE_SHOW: &str = r"
mutation PlexEditShow($input: UpdateShowInput!) {
  result: updateShow(input: $input) { userErrors { code } }
}";
const UPDATE_EPISODE: &str = r"
mutation PlexEditEpisode($input: UpdateEpisodeInput!) {
  result: updateEpisode(input: $input) { userErrors { code } }
}";
const UPDATE_COLLECTION: &str = r"
mutation PlexEditCollection($input: UpdateCollectionInput!) {
  result: updateCollection(input: $input) { userErrors { code } }
}";

/// The input every item of the request is edited with, less its `id`.
#[derive(Debug, Default)]
struct Changes {
    values: Map<String, Json>,
    lock: Vec<&'static str>,
    unlock: Vec<&'static str>,
}

impl Changes {
    fn read(
        params: &[(String, String)],
        edited: EditedType,
        route: Route,
    ) -> Result<Self, ApiError> {
        let mut changes = Self::default();
        // Bare fields first, so that a `.value` of the same field wins whatever the order.
        if route == Route::Item {
            for (key, value) in params {
                let client =
                    key.starts_with("X-Plex-") || CLIENT_PARAMETERS.contains(&key.as_str());
                if !key.contains(['.', '[']) && !client {
                    changes.set(key, value)?;
                }
            }
        }
        for (key, value) in params {
            if let Some(name) = key.strip_suffix(".value") {
                if !edited.holds(name, &COLLECTION_VALUES) {
                    return Err(unhandled(name));
                }
                changes.set(name, value)?;
            } else if let Some(name) = key.strip_suffix(".locked") {
                match changes.set_lock(name, value, edited) {
                    // `/library/metadata` lets such a lock pass, where the section refuses it.
                    Err(InvalidLock) if route == Route::Item => {}
                    Err(InvalidLock) => return Err(ApiError::BadRequest),
                    Ok(result) => result?,
                }
            } else if key.contains('[') || key.starts_with("artist.") || key.starts_with("album.") {
                // Tags added and removed, and music reparented.
                return Err(ApiError::NotImplemented);
            }
        }
        Ok(changes)
    }

    fn set(&mut self, name: &str, value: &str) -> Result<(), ApiError> {
        let Some((_, input, kind)) = EDITABLE.iter().find(|(plex, ..)| *plex == name) else {
            return Err(unhandled(name));
        };
        // Empty clears, as on the reference.
        let written = match (kind, value) {
            (_, "") => Json::Null,
            (ValueKind::Text, text) => json!(text),
            (ValueKind::Year, year) => {
                json!(year.parse::<i32>().map_err(|_| ApiError::BadRequest)?)
            }
            (ValueKind::Score, score) => {
                json!(score.parse::<f64>().map_err(|_| ApiError::BadRequest)?)
            }
        };
        self.values.insert((*input).to_owned(), written);
        Ok(())
    }

    /// A lock or an unlock, or [`InvalidLock`] for a value that is neither: the two routes
    /// answer it differently.
    fn set_lock(
        &mut self,
        name: &str,
        value: &str,
        edited: EditedType,
    ) -> Result<Result<(), ApiError>, InvalidLock> {
        let locks = match value {
            "1" => &mut self.lock,
            "0" => &mut self.unlock,
            _ => return Err(InvalidLock),
        };
        if !edited.holds(name, &COLLECTION_LOCKS) {
            return Ok(Err(unhandled(name)));
        }
        let Some((_, field)) = LOCKABLE.iter().find(|(plex, _)| *plex == name) else {
            return Ok(Err(unhandled(name)));
        };
        match *field {
            "YEAR" if edited.year_follows_date() => {}
            "RELEASED_ON" if edited.year_follows_date() => locks.extend(["RELEASED_ON", "YEAR"]),
            field => locks.push(field),
        }
        Ok(Ok(()))
    }

    fn input(&self, edited: EditedType, key: i64) -> Json {
        let mut input = self.values.clone();
        input.insert("id".to_owned(), json!(edited.global_id(key)));
        input.insert("lock".to_owned(), json!(self.lock));
        input.insert("unlock".to_owned(), json!(self.unlock));
        Json::Object(input)
    }
}

/// A `.locked` value other than `0` or `1`.
#[derive(Debug)]
struct InvalidLock;

/// A field the reference knows and we do not model is `501`; one it does not know, `400`.
fn unhandled(name: &str) -> ApiError {
    if UNMODELED.contains(&name) || LOCKABLE.iter().any(|(plex, _)| *plex == name) {
        ApiError::NotImplemented
    } else {
        ApiError::BadRequest
    }
}

const TARGETS_QUERY: &str = r"
query PlexEditTargets($library: ID!, $ids: [ID!]!) {
  library: node(id: $library) { ... on Library { viewerRole } }
  items: nodes(ids: $ids) {
    __typename
    ... on Item { library { id } }
    ... on Collection { library { id } }
  }
}";

#[derive(Debug, Deserialize)]
struct TargetsData {
    library: Option<TargetLibrary>,
    items: Vec<Option<TargetItem>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetLibrary {
    viewer_role: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TargetItem {
    #[serde(rename = "__typename")]
    typename: String,
    library: Option<TargetItemLibrary>,
}

#[derive(Debug, Deserialize)]
struct TargetItemLibrary {
    id: String,
}

#[derive(Debug, Deserialize)]
struct EditData {
    result: EditResult,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditResult {
    user_errors: Vec<EditUserError>,
}

#[derive(Debug, Deserialize)]
struct EditUserError {
    code: String,
}

fn value_of<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

pub async fn edit_items(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<impl IntoResponse, ApiError> {
    let edited = EditedType::of(value_of(&params, "type").ok_or(ApiError::BadRequest)?)?;
    // Without ids the reference edits the whole library, or what `filters` selects.
    let Some(ids) = value_of(&params, "id") else {
        return Err(ApiError::NotImplemented);
    };
    if value_of(&params, "filters").is_some() {
        return Err(ApiError::NotImplemented);
    }
    let changes = Changes::read(&params, edited, Route::Section)?;
    // An id that is not a number names nothing, which the reference ignores.
    let ids: Vec<i64> = ids.split(',').filter_map(|id| id.parse().ok()).collect();
    if ids.len() > MAX_ITEMS as usize {
        return Err(ApiError::BadRequest);
    }

    let library = GlobalId::encode(LIBRARY_TYPE, section);
    let variables = Variables::from_json(json!({
        "library": library,
        "ids": ids.iter().map(|id| edited.global_id(*id)).collect::<Vec<_>>(),
    }));
    let targets: TargetsData = gql::execute(&state.schema, TARGETS_QUERY, variables).await?;
    // Told apart from an unknown library no more than the settings route does.
    match targets.library.and_then(|library| library.viewer_role) {
        Some(role) if role == "ADMIN" => {}
        _ => return Err(ApiError::NotFound),
    }

    let in_scope = ids.iter().zip(&targets.items).filter_map(|(id, item)| {
        let item = item.as_ref()?;
        let here = item.library.as_ref().is_some_and(|l| l.id == library);
        (here && item.typename == edited.typename()).then_some(*id)
    });
    for item in in_scope {
        apply(&state, &changes, edited, item).await?;
    }
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/html")]))
}

/// Edits one item through the official API, its refusal turned into the status Plex answers.
async fn apply(
    state: &PlexState,
    changes: &Changes,
    edited: EditedType,
    item: i64,
) -> Result<(), ApiError> {
    let variables = Variables::from_json(json!({ "input": changes.input(edited, item) }));
    let data: EditData = gql::execute(&state.schema, edited.mutation(), variables).await?;
    if let Some(error) = data.result.user_errors.first() {
        tracing::info!(code = error.code, item, "refused an edit");
        return Err(match error.code.as_str() {
            "NOT_APPLICABLE" => ApiError::NotImplemented,
            "ITEM_NOT_FOUND" => ApiError::NotFound,
            _ => ApiError::BadRequest,
        });
    }
    Ok(())
}

const ITEMS_QUERY: &str = r"
query PlexEditItems($ids: [ID!]!) {
  items: nodes(ids: $ids) {
    __typename
    ... on Item { library { viewerRole } }
    ... on Collection { library { viewerRole } }
  }
}";

#[derive(Debug, Deserialize)]
struct ItemsData {
    items: Vec<Option<EditableItem>>,
}

#[derive(Debug, Deserialize)]
struct EditableItem {
    #[serde(rename = "__typename")]
    typename: String,
    library: Option<TargetLibrary>,
}

/// `PUT /library/metadata/{ids}`: every item named, each edited as what it is. One the caller
/// may not edit counts as one that does not exist, as a library the caller is not an admin of
/// answers on the section's route.
pub async fn edit_metadata(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<impl IntoResponse, ApiError> {
    if params.is_empty() {
        return Err(ApiError::BadRequest);
    }
    let ids: Vec<i64> = ids.split(',').filter_map(|id| id.parse().ok()).collect();
    if ids.len() > MAX_ITEMS as usize {
        return Err(ApiError::BadRequest);
    }
    // Each number may name an item or a collection, which share the numbering: both are
    // asked for, items first.
    let encoded: Vec<String> = ids
        .iter()
        .map(|id| GlobalId::encode(ITEM_TYPE, *id))
        .chain(ids.iter().map(|id| GlobalId::encode(COLLECTION_TYPE, *id)))
        .collect();
    let variables = Variables::from_json(json!({ "ids": encoded }));
    let found: ItemsData = gql::execute(&state.schema, ITEMS_QUERY, variables).await?;
    let (items, collections) = found.items.split_at(ids.len().min(found.items.len()));
    let mut edited_any = false;
    for (index, id) in ids.iter().enumerate() {
        let Some(item) = items
            .get(index)
            .and_then(Option::as_ref)
            .or_else(|| collections.get(index).and_then(Option::as_ref))
        else {
            continue;
        };
        let role = item
            .library
            .as_ref()
            .and_then(|library| library.viewer_role.as_deref());
        if role != Some("ADMIN") {
            continue;
        }
        let edited = EditedType::named(&item.typename)?;
        let changes = Changes::read(&params, edited, Route::Item)?;
        apply(&state, &changes, edited, *id).await?;
        edited_any = true;
    }
    if !edited_any {
        return Err(ApiError::NotFound);
    }
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/html")]))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn lists_locks_in_the_references_order() {
        // Item 429849 of the reference, as it lists them.
        let locked = [
            "WRITERS",
            "YEAR",
            "TITLE",
            "POSTER",
            "STUDIO",
            "RELEASED_ON",
            "TITLE_SORT",
            "ORIGINAL_TITLE",
        ]
        .map(ToOwned::to_owned);
        assert_eq!(
            plex_locked_fields(&locked).collect::<Vec<_>>(),
            [
                "title",
                "titleSort",
                "originalTitle",
                "studio",
                "thumb",
                "originallyAvailableAt",
                "year",
                "writer"
            ]
        );
    }

    #[test]
    fn a_films_date_locks_its_year_and_its_year_alone_locks_nothing() {
        let film = Changes::read(
            &params(&[("originallyAvailableAt.locked", "1"), ("year.locked", "1")]),
            EditedType::Movie,
            Route::Section,
        )
        .unwrap();
        assert_eq!(film.lock, ["RELEASED_ON", "YEAR"]);
        let episode = Changes::read(
            &params(&[("year.locked", "1")]),
            EditedType::Episode,
            Route::Section,
        )
        .unwrap();
        assert_eq!(episode.lock, ["YEAR"]);
    }

    #[test]
    fn values_are_typed_and_empty_clears() {
        let changes = Changes::read(
            &params(&[
                ("title.value", "The Soul"),
                ("year.value", ""),
                ("rating.value", "8.1"),
                ("originallyAvailableAt.value", "2019-05-01"),
                ("includeExternalMedia", "1"),
            ]),
            EditedType::Movie,
            Route::Section,
        )
        .unwrap();
        assert_eq!(
            Json::Object(changes.values),
            json!({ "title": "The Soul", "year": null, "rating": 8.1, "releasedOn": "2019-05-01" })
        );
    }

    #[test]
    fn refuses_what_it_cannot_do_apart_from_what_makes_no_sense() {
        let read = |pairs: &[(&str, &str)]| {
            Changes::read(&params(pairs), EditedType::Movie, Route::Section)
                .map(|_| ())
                .map_err(|e| e.to_string())
        };
        let not_implemented = Err("not implemented".to_owned());
        let bad = Err("bad request".to_owned());
        assert_eq!(read(&[("thumb.value", "x")]), not_implemented);
        assert_eq!(read(&[("banner.locked", "1")]), not_implemented);
        assert_eq!(read(&[("genre[0].tag.tag", "Drama")]), not_implemented);
        assert_eq!(read(&[("bogus.value", "x")]), bad);
        assert_eq!(read(&[("bogus.locked", "1")]), bad);
        assert_eq!(read(&[("summary.locked", "2")]), bad);
        assert_eq!(read(&[("year.value", "abc")]), bad);
    }

    #[test]
    fn the_items_route_takes_bare_fields_and_lets_a_strange_lock_pass() {
        let read = |pairs: &[(&str, &str)]| {
            Changes::read(&params(pairs), EditedType::Movie, Route::Item).map_err(|e| e.to_string())
        };
        // As measured: the bare form of the spec, `.value` winning in either order.
        let changes = read(&[
            ("title.value", "Value"),
            ("title", "Bare"),
            ("tagline", "A tagline"),
            ("type", "2"),
            ("includeExternalMedia", "1"),
            ("X-Plex-Product", "Plex Web"),
            ("summary.locked", "2"),
        ])
        .unwrap();
        assert_eq!(
            Json::Object(changes.values),
            json!({ "title": "Value", "tagline": "A tagline" })
        );
        assert!(changes.lock.is_empty() && changes.unlock.is_empty());
        assert_eq!(
            read(&[("nonsense", "x")]).err(),
            Some("bad request".to_owned())
        );
        assert_eq!(
            read(&[("thumb", "x")]).err(),
            Some("not implemented".to_owned())
        );
    }
}
