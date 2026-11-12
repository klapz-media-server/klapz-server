//! `GET /library/sections/{sectionId}/{filter}` — the choices of one filter.
//!
//! What a client shows when the user opens "Genre" or "Year" above a library: the values the
//! library's items actually hold, each with the listing URL that applies it. The filter names
//! are the ones `includeMeta` advertises; none of these paths is in the spec.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::database_id;
use crate::items::{
    ViewStyle, GRID_VIEW_MODE, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION,
};
use crate::library::item_type;
use crate::listing;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

/// Which list a path segment names, and how its entries are spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    /// `kind` is the schema's `TagType` value.
    Tag {
        kind: &'static str,
    },
    Year,
    /// Years grouped by ten, which the adapter does: a decade is a presentation of years.
    Decade,
    /// `field` is the schema's `TextField` value.
    Text {
        field: &'static str,
    },
    /// The classes of [`crate::resolution`] some version of the library falls in.
    Resolution,
    /// The collections holding an item, by the number the filter takes.
    Collection,
    /// `field` is the schema's `TrackField` value.
    Track {
        field: &'static str,
    },
}

impl Choice {
    fn named(filter: &str) -> Option<Self> {
        Some(match filter {
            "genre" => Self::Tag { kind: "GENRE" },
            "country" => Self::Tag { kind: "COUNTRY" },
            "director" => Self::Tag { kind: "DIRECTOR" },
            "writer" => Self::Tag { kind: "WRITER" },
            "actor" => Self::Tag { kind: "ACTOR" },
            "producer" => Self::Tag { kind: "PRODUCER" },
            "year" => Self::Year,
            "decade" => Self::Decade,
            "contentRating" => Self::Text {
                field: "CONTENT_RATING",
            },
            "studio" => Self::Text { field: "STUDIO" },
            "resolution" => Self::Resolution,
            "collection" => Self::Collection,
            "videoCodec" => Self::Track {
                field: "VIDEO_CODEC",
            },
            "audioCodec" => Self::Track {
                field: "AUDIO_CODEC",
            },
            "audioLayout" => Self::Track {
                field: "AUDIO_LAYOUT",
            },
            "subtitleCodec" => Self::Track {
                field: "SUBTITLE_CODEC",
            },
            _ => return None,
        })
    }
}

/// How far down a cast the "starring" list reaches: the reference lists an actor billed
/// first, second or third on some film — 10,075 of its 10,251, the rest stale — and none of
/// the 102,000 billed lower.
const STARRING: u32 = 3;

/// The view's name, in English whatever the client's language, as the reference sends it.
fn view_title(filter: &str) -> &'static str {
    match filter {
        "genre" => "By Genre",
        "country" => "By Country",
        "director" => "By Director",
        "writer" => "By Writer",
        "actor" => "By Starring Actor",
        "producer" => "By Producer",
        "year" => "By Year",
        "decade" => "By Decade",
        "contentRating" => "By Content Rating",
        "resolution" => "By Resolution",
        "collection" => "By Collection",
        "videoCodec" => "By Video Codec",
        "audioCodec" => "By Audio Codec",
        "audioLayout" => "By Audio Layout",
        "subtitleCodec" => "By Subtitle Codec",
        _ => "By Studio",
    }
}

const TAGS_QUERY: &str = r"
query PlexTagChoices($id: ID!, $kind: TagType!, $billed: Int, $filter: ItemFilterInput, $first: Int!, $offset: Int!) {
  node(id: $id) {
    ... on Library {
      title
      kind
      tags(kind: $kind, billedWithin: $billed, filter: $filter, first: $first, offset: $offset) {
        edges { node { id name person { thumbUrl } } }
      }
      stats(filter: $filter) { total: tagCount(kind: $kind, billedWithin: $billed) }
    }
  }
}
";

/// The collections holding an item, by title: the reference lists 1,024 of the 1,116
/// collections of a film library, the others being empty, and no smart one.
const COLLECTIONS_QUERY: &str = r"
query PlexCollectionChoices($id: ID!, $first: Int!, $offset: Int!) {
  node(id: $id) {
    ... on Library {
      title
      kind
      collections(withEntries: true, smart: false, first: $first, offset: $offset) {
        totalCount
        edges { node { tagNumber title thumbUrl } }
      }
    }
  }
}
";

#[derive(Debug, Deserialize)]
struct CollectionChoicesData {
    node: Option<GqlCollectionChoices>,
}

#[derive(Debug, Deserialize)]
struct GqlCollectionChoices {
    title: String,
    kind: String,
    collections: GqlCollectionPage,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlCollectionPage {
    total_count: u64,
    edges: Vec<GqlEdge<GqlCollectionChoice>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlCollectionChoice {
    tag_number: String,
    title: String,
    thumb_url: Option<String>,
}

const YEARS_QUERY: &str = r"
query PlexYearChoices($id: ID!, $filter: ItemFilterInput, $first: Int!, $offset: Int!) {
  node(id: $id) {
    ... on Library {
      title
      kind
      years(filter: $filter, first: $first, offset: $offset) { edges { node } }
      stats(filter: $filter) { total: yearCount }
    }
  }
}
";

const TEXT_QUERY: &str = r"
query PlexTextChoices($id: ID!, $field: TextField!, $filter: ItemFilterInput, $first: Int!, $offset: Int!) {
  node(id: $id) {
    ... on Library {
      title
      kind
      fieldValues(field: $field, filter: $filter, first: $first, offset: $offset) { edges { node } }
      stats(filter: $filter) { total: fieldValueCount(field: $field) }
    }
  }
}
";

/// The values of the library's tracks. Every version's, whatever `type` says: a series
/// library lists its episodes', as the reference does, shows and seasons having no media.
const TRACKS_QUERY: &str = r"
query PlexTrackChoices($id: ID!, $field: TrackField!, $filter: ItemFilterInput, $first: Int!, $offset: Int!) {
  node(id: $id) {
    ... on Library {
      title
      kind
      trackValues(field: $field, filter: $filter, first: $first, offset: $offset) { edges { node } }
      stats(filter: $filter) { total: trackValueCount(field: $field) }
    }
  }
}
";

/// How many items each resolution class holds, one alias per class in
/// [`crate::resolution::keys`] order: a library lists the classes that hold any.
const RESOLUTIONS_QUERY: &str = r"
query PlexResolutions(
  $id: ID!
  $f0: ItemFilterInput
  $f1: ItemFilterInput
  $f2: ItemFilterInput
  $f3: ItemFilterInput
  $f4: ItemFilterInput
  $f5: ItemFilterInput
  $f6: ItemFilterInput
  $f7: ItemFilterInput
  $f8: ItemFilterInput
) {
  node(id: $id) {
    ... on Library {
      title
      kind
      r0: stats(filter: $f0) { itemCount }
      r1: stats(filter: $f1) { itemCount }
      r2: stats(filter: $f2) { itemCount }
      r3: stats(filter: $f3) { itemCount }
      r4: stats(filter: $f4) { itemCount }
      r5: stats(filter: $f5) { itemCount }
      r6: stats(filter: $f6) { itemCount }
      r7: stats(filter: $f7) { itemCount }
      r8: stats(filter: $f8) { itemCount }
    }
  }
}
";

#[derive(Debug, Deserialize)]
struct ResolutionsData {
    node: Option<GqlResolutions>,
}

#[derive(Debug, Deserialize)]
struct GqlResolutions {
    title: String,
    kind: String,
    #[serde(flatten)]
    counts: std::collections::HashMap<String, GqlItemCount>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlItemCount {
    item_count: u64,
}

/// An `ItemFilterInput` with more fields set, null standing for an empty one.
pub(crate) fn with_fields<const N: usize>(filter: Value, fields: [(&str, Value); N]) -> Value {
    let mut object = match filter {
        Value::Object(object) => object,
        _ => async_graphql::indexmap::IndexMap::new(),
    };
    for (name, value) in fields {
        object.insert(Name::new(name), value);
    }
    Value::Object(object)
}

#[derive(Debug, Deserialize)]
struct ChoicesData<T> {
    node: Option<GqlChoices<T>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlChoices<T> {
    title: String,
    kind: String,
    #[serde(alias = "years", alias = "fieldValues", alias = "trackValues")]
    tags: GqlEdges<T>,
    stats: GqlTotal,
}

#[derive(Debug, Deserialize)]
struct GqlEdges<T> {
    edges: Vec<GqlEdge<T>>,
}

#[derive(Debug, Deserialize)]
struct GqlEdge<T> {
    node: T,
}

#[derive(Debug, Deserialize)]
struct GqlTotal {
    total: u64,
}

#[derive(Debug, Deserialize)]
struct GqlTag {
    id: String,
    name: String,
    person: Option<GqlPerson>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPerson {
    thumb_url: Option<String>,
}

/// Response body: a "secondary" view, a list of values to jump into.
///
/// Always windowed, with `totalSize` and `offset`, for the reason `/all` is: the reference
/// returns 10,251 actors in one response, and `MAX_ITEMS` forbids that.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChoicesContainer {
    size: u32,
    total_size: u64,
    offset: u64,
    allow_sync: bool,
    art: &'static str,
    /// Absent from the actor list alone, on the reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'static str>,
    identifier: &'static str,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    nocache: Option<bool>,
    thumb: &'static str,
    title1: String,
    title2: &'static str,
    view_group: &'static str,
    view_mode: u32,
    #[serde(rename = "Directory", skip_serializing_if = "Vec::is_empty")]
    directory: Vec<ChoiceDirectory>,
}

/// One value, and the listing that applies it.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ChoiceDirectory {
    /// Absent from the resolution list, on the reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumb: Option<String>,
    key: String,
    title: String,
    /// Set on genres only.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<&'static str>,
}

pub async fn filter_values(
    _caller: Caller,
    State(state): State<PlexState>,
    Path((section, filter)): Path<(i64, String)>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<ChoicesContainer>, ApiError> {
    let Some(choice) = Choice::named(&filter) else {
        return Err(ApiError::NotFound);
    };
    let request = listing::parse(&pairs)?;
    let offset = request.start;
    let size = request.size.unwrap_or(MAX_ITEMS).min(MAX_ITEMS);

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    if let Choice::Track { .. } = choice {
        variables.insert(
            Name::new("filter"),
            with_fields(
                request.filters.to_graphql(None),
                [("depth", Value::Enum(Name::new("ALL")))],
            ),
        );
    } else if choice == Choice::Resolution {
        // A series library lists its episodes' classes whatever `type` says, as the
        // reference does: shows and seasons have no media.
        for (position, class) in crate::resolution::keys().enumerate() {
            variables.insert(
                Name::new(format!("f{position}")),
                with_fields(
                    request.filters.to_graphql(None),
                    [
                        ("depth", Value::Enum(Name::new("ALL"))),
                        ("frame", crate::resolution::label_range(class)),
                    ],
                ),
            );
        }
    } else {
        variables.insert(
            Name::new("filter"),
            request.filters.to_graphql(item_type(request.kind)),
        );
    }
    // Decades are grouped from every year, which fit in one page: a century of releases is
    // a hundred rows.
    let (first, skip) = if choice == Choice::Decade {
        (MAX_ITEMS, 0)
    } else {
        (size, offset)
    };
    variables.insert(Name::new("first"), Value::Number(first.into()));
    variables.insert(Name::new("offset"), Value::Number(skip.into()));

    let listing = |value: &str| format!("/library/sections/{section}/all?{filter}={value}");
    let Choices {
        title,
        kind,
        entries,
        total,
    } = choices(&state, choice, variables, (offset, size), &listing).await?;

    let style = ViewStyle::for_kind(&kind);
    let body = ChoicesContainer {
        size: u32::try_from(entries.len()).unwrap_or(u32::MAX),
        total_size: total,
        offset,
        allow_sync: false,
        art: style.art,
        content: (filter != "actor").then_some("secondary"),
        identifier: LIBRARY_IDENTIFIER,
        media_tag_prefix: MEDIA_TAG_PREFIX,
        media_tag_version: MEDIA_TAG_VERSION,
        nocache: (kind == "SHOW").then_some(true),
        thumb: style.thumb,
        title1: title,
        title2: view_title(&filter),
        view_group: "secondary",
        view_mode: GRID_VIEW_MODE,
        directory: entries,
    };
    Ok(PlexResponse::new(body, format).paged(offset, total))
}

/// A page of one list, with what the container says about the library.
struct Choices {
    title: String,
    kind: String,
    entries: Vec<ChoiceDirectory>,
    total: u64,
}

async fn choices(
    state: &PlexState,
    choice: Choice,
    mut variables: Variables,
    (offset, size): (u64, u32),
    listing: &impl Fn(&str) -> String,
) -> Result<Choices, ApiError> {
    Ok(match choice {
        Choice::Resolution => resolution_choices(state, variables, (offset, size)).await?,
        Choice::Collection => collection_choices(state, variables, listing).await?,
        Choice::Tag { kind } => {
            variables.insert(Name::new("kind"), Value::Enum(Name::new(kind)));
            if kind == "ACTOR" {
                variables.insert(Name::new("billed"), Value::Number(STARRING.into()));
            }
            let library = fetch::<GqlTag>(state, TAGS_QUERY, variables).await?;
            let entries = library
                .tags
                .edges
                .iter()
                .map(|edge| {
                    let id = database_id(&edge.node.id).to_string();
                    ChoiceDirectory {
                        fast_key: Some(listing(&id)),
                        thumb: edge
                            .node
                            .person
                            .as_ref()
                            .and_then(|person| person.thumb_url.clone()),
                        key: id,
                        title: edge.node.name.clone(),
                        kind: (kind == "GENRE").then_some("genre"),
                    }
                })
                .collect();
            let total = library.stats.total;
            Choices {
                title: library.title,
                kind: library.kind,
                entries,
                total,
            }
        }
        Choice::Year => {
            let library = fetch::<i32>(state, YEARS_QUERY, variables).await?;
            let entries = library
                .tags
                .edges
                .iter()
                .map(|edge| plain(edge.node.to_string(), listing))
                .collect();
            let total = library.stats.total;
            Choices {
                title: library.title,
                kind: library.kind,
                entries,
                total,
            }
        }
        Choice::Decade => {
            let library = fetch::<i32>(state, YEARS_QUERY, variables).await?;
            let decades = decades(library.tags.edges.iter().map(|edge| edge.node));
            let total = decades.len() as u64;
            let entries = decades
                .into_iter()
                .skip(usize::try_from(offset).unwrap_or(usize::MAX))
                .take(size as usize)
                .map(|decade| ChoiceDirectory {
                    fast_key: Some(listing(&decade.to_string())),
                    thumb: None,
                    key: decade.to_string(),
                    title: format!("{decade}s"),
                    kind: None,
                })
                .collect();
            Choices {
                title: library.title,
                kind: library.kind,
                entries,
                total,
            }
        }
        Choice::Track { field } => track_choices(state, field, variables).await?,
        Choice::Text { field } => {
            variables.insert(Name::new("field"), Value::Enum(Name::new(field)));
            let library = fetch::<String>(state, TEXT_QUERY, variables).await?;
            let studio = field == "STUDIO";
            let entries = library
                .tags
                .edges
                .iter()
                .map(|edge| text_choice(&edge.node, studio, listing))
                .collect();
            let total = library.stats.total;
            Choices {
                title: library.title,
                kind: library.kind,
                entries,
                total,
            }
        }
    })
}

/// The values the library's tracks hold in a field.
async fn track_choices(
    state: &PlexState,
    field: &'static str,
    mut variables: Variables,
) -> Result<Choices, ApiError> {
    variables.insert(Name::new("field"), Value::Enum(Name::new(field)));
    let library = fetch::<String>(state, TRACKS_QUERY, variables).await?;
    let entries = library
        .tags
        .edges
        .into_iter()
        .map(|edge| track_choice(edge.node))
        .collect();
    Ok(Choices {
        title: library.title,
        kind: library.kind,
        entries,
        total: library.stats.total,
    })
}

/// The collections holding an item, by the number a listing filters on.
async fn collection_choices(
    state: &PlexState,
    variables: Variables,
    listing: &impl Fn(&str) -> String,
) -> Result<Choices, ApiError> {
    let data: CollectionChoicesData =
        gql::execute(&state.schema, COLLECTIONS_QUERY, variables).await?;
    let library = data.node.ok_or(ApiError::NotFound)?;
    let entries = library
        .collections
        .edges
        .into_iter()
        .map(|edge| ChoiceDirectory {
            fast_key: Some(listing(&edge.node.tag_number)),
            thumb: edge.node.thumb_url,
            key: edge.node.tag_number,
            title: edge.node.title,
            kind: None,
        })
        .collect();
    Ok(Choices {
        title: library.title,
        kind: library.kind,
        entries,
        total: library.collections.total_count,
    })
}

/// The resolution classes some version falls in, from one count per class.
async fn resolution_choices(
    state: &PlexState,
    variables: Variables,
    (offset, size): (u64, u32),
) -> Result<Choices, ApiError> {
    let data: ResolutionsData = gql::execute(&state.schema, RESOLUTIONS_QUERY, variables).await?;
    let library = data.node.ok_or(ApiError::NotFound)?;
    let present: Vec<&str> = crate::resolution::keys()
        .enumerate()
        .filter(|(position, _)| {
            library
                .counts
                .get(&format!("r{position}"))
                .is_some_and(|stats| stats.item_count > 0)
        })
        .map(|(_, class)| class)
        .collect();
    let total = present.len() as u64;
    let entries = present
        .into_iter()
        .skip(usize::try_from(offset).unwrap_or(usize::MAX))
        .take(size as usize)
        .map(|class| ChoiceDirectory {
            fast_key: None,
            thumb: None,
            key: class.to_owned(),
            title: crate::resolution::title(class).to_owned(),
            kind: None,
        })
        .collect();
    Ok(Choices {
        title: library.title,
        kind: library.kind,
        entries,
        total,
    })
}

async fn fetch<T: serde::de::DeserializeOwned>(
    state: &PlexState,
    query: &str,
    variables: Variables,
) -> Result<GqlChoices<T>, ApiError> {
    let data: ChoicesData<T> = gql::execute(&state.schema, query, variables).await?;
    data.node.ok_or(ApiError::NotFound)
}

fn plain(value: String, listing: &impl Fn(&str) -> String) -> ChoiceDirectory {
    ChoiceDirectory {
        fast_key: Some(listing(&value)),
        thumb: None,
        key: value.clone(),
        title: value,
        kind: None,
    }
}

/// A content rating or a studio, encoded as the reference encodes each.
///
/// A content rating's key and listing URL are percent-encoded once. A studio's key is
/// encoded twice — `10.80 Films` becomes `10%252E80%2520Films` — while its listing URL is
/// not encoded at all. Measured over all 3,253 studios and 49 ratings of a film library.
fn text_choice(value: &str, studio: bool, listing: &impl Fn(&str) -> String) -> ChoiceDirectory {
    let encoded = encode(value);
    let (key, fast_key) = if studio {
        (encoded.replace('%', "%25"), listing(value))
    } else {
        (encoded.clone(), listing(&encoded))
    };
    ChoiceDirectory {
        fast_key: Some(fast_key),
        thumb: None,
        key,
        title: value.to_owned(),
        kind: None,
    }
}

/// A codec or a layout: its key encoded once, as a content rating's, and no listing URL,
/// which the reference leaves the client to build from the key.
fn track_choice(value: String) -> ChoiceDirectory {
    ChoiceDirectory {
        fast_key: None,
        thumb: None,
        key: encode(&value),
        title: value,
        kind: None,
    }
}

/// Percent-encodes everything but letters, digits, `-` and `_`.
///
/// Narrower than RFC 3986's unreserved set: the reference encodes `.` (`A.J. Films` is
/// `A%2EJ%2E%20Films`). Whether it keeps `~` is unmeasured, and encoding it is harmless.
fn encode(text: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            out.push(char::from(byte));
        } else {
            // Writing into a `String` cannot fail.
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// The decades of a list of years, newest first, each once.
fn decades(years: impl Iterator<Item = i32>) -> Vec<i32> {
    let mut decades: Vec<i32> = years.map(|year| year - year.rem_euclid(10)).collect();
    decades.sort_unstable_by(|a, b| b.cmp(a));
    decades.dedup();
    decades
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn listing(value: &str) -> String {
        format!("/library/sections/4/all?studio={value}")
    }

    #[test]
    fn encodes_a_studio_key_twice_and_leaves_its_listing_raw() {
        let choice = text_choice("10.80 Films", true, &listing);
        assert_eq!(choice.key, "10%252E80%2520Films");
        assert_eq!(
            choice.fast_key.as_deref(),
            Some("/library/sections/4/all?studio=10.80 Films")
        );

        assert_eq!(
            text_choice("3ème Oeil Productions", true, &listing).key,
            "3%25C3%25A8me%2520Oeil%2520Productions"
        );
        assert_eq!(
            text_choice("A-1 Pictures", true, &listing).key,
            "A-1%2520Pictures"
        );
    }

    #[test]
    fn encodes_a_content_rating_once_in_both_places() {
        let rating = |value: &str| format!("/library/sections/4/all?contentRating={value}");
        let choice = text_choice("fr/Tous publics (2019)", false, &rating);
        assert_eq!(choice.key, "fr%2FTous%20publics%20%282019%29");
        assert_eq!(
            choice.fast_key.as_deref(),
            Some("/library/sections/4/all?contentRating=fr%2FTous%20publics%20%282019%29")
        );
        assert_eq!(text_choice("PG-13", false, &rating).key, "PG-13");
        assert_eq!(text_choice("16+", false, &rating).key, "16%2B");
    }

    #[test]
    fn encodes_a_layout_key_once_and_gives_no_listing() {
        // Captured from the reference's `audioLayout` list.
        let choice = track_choice("8 channels (FL+FR+FC+LFE+SL+SR+TFL+TFR)".to_owned());
        assert_eq!(
            choice.key,
            "8%20channels%20%28FL%2BFR%2BFC%2BLFE%2BSL%2BSR%2BTFL%2BTFR%29"
        );
        assert_eq!(choice.fast_key, None);
        assert_eq!(track_choice("5.1(side)".to_owned()).key, "5%2E1%28side%29");
        assert_eq!(track_choice("pcm_bluray".to_owned()).key, "pcm_bluray");
    }

    #[test]
    fn groups_years_into_decades_newest_first() {
        assert_eq!(
            decades([1915, 2026, 2020, 1999, 2019, 1910].into_iter()),
            vec![2020, 2010, 1990, 1910]
        );
    }

    #[test]
    fn names_only_the_lists_it_can_honor() {
        assert_eq!(Choice::named("genre"), Some(Choice::Tag { kind: "GENRE" }));
        assert_eq!(Choice::named("decade"), Some(Choice::Decade));
        assert_eq!(Choice::named("resolution"), Some(Choice::Resolution));
        assert_eq!(
            Choice::named("audioLayout"),
            Some(Choice::Track {
                field: "AUDIO_LAYOUT"
            })
        );
        assert_eq!(Choice::named("bitrate"), None);
        assert_eq!(Choice::named("Genre"), None);
    }
}
