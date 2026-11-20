//! `GET /library/sections/{sectionId}/categories`: a library's genres as picture tiles.
//!
//! The list is the library's genres, `Library.tags(kind: GENRE)` on the official API, each
//! opening the listing it filters. What the reference adds is presentation: a tile color and
//! a backdrop. Measured on ten libraries and on genres added to a test library;
//! `docs/spec-gaps.md` has the rules.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use klapz_api::library::LIBRARY_TYPE;
use klapz_api::GlobalId;
use klapz_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::database_id;
use crate::items::{
    ViewStyle, GRID_VIEW_MODE, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION,
};
use crate::menus::window;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

const GENRES_QUERY: &str = r"
query PlexCategories($id: ID!, $first: Int!, $offset: Int!) {
  node(id: $id) {
    ... on Library {
      title
      kind
      tags(kind: GENRE, first: $first, offset: $offset) { edges { node { id name } } }
      stats { total: tagCount(kind: GENRE) }
    }
  }
}
";

#[derive(Debug, Deserialize)]
struct GenresData {
    node: Option<GqlLibrary>,
}

#[derive(Debug, Deserialize)]
struct GqlLibrary {
    title: String,
    kind: String,
    tags: GqlEdges,
    stats: GqlTotal,
}

#[derive(Debug, Deserialize)]
struct GqlEdges {
    edges: Vec<GqlEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlEdge {
    node: GqlGenre,
}

#[derive(Debug, Deserialize)]
struct GqlGenre {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct GqlTotal {
    total: u64,
}

/// Genres the reference paints with a color of their own: the thirteen it ships a
/// `category-art` image for, by that image's name. A title is matched in lower case with its
/// spaces as hyphens, which is how `Science Fiction` and `Science-Fiction` share a color.
const NAMED_COLORS: [(&str, &str); 13] = [
    ("action", "2B717E"),
    ("animation", "347431"),
    ("comedy", "51284C"),
    ("crime", "33486E"),
    ("documentary", "884624"),
    ("drama", "846E49"),
    ("history", "37397A"),
    ("horror", "7A281A"),
    ("musical", "3B8265"),
    ("romance", "892959"),
    ("science-fiction", "673E8B"),
    ("thriller", "57222B"),
    ("western", "704A23"),
];

/// The colors every other genre takes in turn, by ascending tag id over the whole library,
/// so that a page shows the colors the whole list would. Adventure, tag 122, is the lowest
/// such genre in every library measured, which is why it always looks named.
const CYCLE: [&str; 7] = [
    "494616", "91293D", "4C561B", "883E7D", "8B6122", "8B4D43", "23533C",
];

/// The image the reference falls back on for a genre none of whose items has background
/// art, encoded as it encodes it. Every tile here: this server keeps no artwork.
const FALLBACK_ART: &str = "%2F%3A%2Fresources%2Fcategory-art%2Fanimation%2Epng";

fn named_color(title: &str) -> Option<&'static str> {
    let name = title.to_ascii_lowercase().replace(' ', "-");
    NAMED_COLORS
        .iter()
        .find(|(named, _)| *named == name)
        .map(|(_, color)| *color)
}

/// One color per genre, in the order given, each genre being `(tag id, name)`.
fn blend_colors(genres: &[(i64, &str)]) -> Vec<&'static str> {
    let mut colors: Vec<Option<&'static str>> =
        genres.iter().map(|(_, name)| named_color(name)).collect();
    let mut unnamed: Vec<usize> = (0..genres.len())
        .filter(|&position| colors[position].is_none())
        .collect();
    unnamed.sort_by_key(|&position| genres[position].0);
    for (turn, position) in unnamed.into_iter().enumerate() {
        colors[position] = Some(CYCLE[turn % CYCLE.len()]);
    }
    colors.into_iter().flatten().collect()
}

/// Response body: a `secondary` view whose one `Meta` type names the view itself.
///
/// Whole unless the client asked for a window, as on the reference: a library holds a few
/// dozen genres. Past [`MAX_ITEMS`] it is cut, and says so.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoriesContainer {
    size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<usize>,
    allow_sync: bool,
    art: &'static str,
    content: &'static str,
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
    #[serde(rename = "Meta")]
    meta: CategoriesMeta,
    #[serde(rename = "Directory", skip_serializing_if = "Vec::is_empty")]
    directory: Vec<Category>,
}

#[derive(Debug, Serialize)]
struct CategoriesMeta {
    #[serde(rename = "Type")]
    types: [ViewType; 1],
}

#[derive(Debug, Serialize)]
struct ViewType {
    key: String,
    #[serde(rename = "type")]
    kind: &'static str,
    title: &'static str,
    active: bool,
}

#[derive(Debug, Serialize)]
struct Category {
    thumb: String,
    #[serde(rename = "type")]
    kind: &'static str,
    key: String,
    title: String,
}

pub async fn categories(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(section): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<CategoriesContainer>, ApiError> {
    // A section that is no number is one that does not exist, `404` on the reference.
    let section: i64 = section.parse().map_err(|_| ApiError::NotFound)?;
    let (title, kind, genres) = library_genres(&state, section).await?;
    let named: Vec<(i64, &str)> = genres
        .iter()
        .map(|(id, name)| (*id, name.as_str()))
        .collect();
    let colors = blend_colors(&named);

    let total = genres.len();
    let window = window(&pairs)
        .map(|(offset, size)| (offset, size.min(MAX_ITEMS as usize)))
        .or_else(|| (total > MAX_ITEMS as usize).then_some((0, MAX_ITEMS as usize)));
    let (skip, take) = window.unwrap_or((0, total));
    let directory: Vec<Category> = genres
        .into_iter()
        .zip(colors)
        .skip(skip)
        .take(take)
        .map(|((id, name), color)| Category {
            thumb: format!(
                "/photo/:/transcode?blendColor={color}&width=2560&height=1440&url={FALLBACK_ART}"
            ),
            kind: "directory",
            key: format!("/library/sections/{section}/all?genre={id}"),
            title: name,
        })
        .collect();

    let style = ViewStyle::for_kind(&kind);
    let body = CategoriesContainer {
        size: directory.len(),
        total_size: window.map(|_| total),
        offset: window.map(|(offset, _)| offset),
        allow_sync: false,
        art: style.art,
        content: "secondary",
        identifier: LIBRARY_IDENTIFIER,
        media_tag_prefix: MEDIA_TAG_PREFIX,
        media_tag_version: MEDIA_TAG_VERSION,
        nocache: (kind == "SHOW").then_some(true),
        thumb: style.thumb,
        title1: title,
        title2: "",
        view_group: "secondary",
        view_mode: GRID_VIEW_MODE,
        meta: CategoriesMeta {
            types: [ViewType {
                key: format!("/library/sections/{section}/categories"),
                kind: "directory",
                title: "Categories",
                active: true,
            }],
        },
        directory,
    };
    let response = PlexResponse::new(body, format);
    Ok(match window {
        Some((offset, _)) => response.paged(offset as u64, total as u64),
        None => response,
    })
}

/// The library's title and kind, and every genre it carries by name, each with its tag id.
///
/// Every page is read, since a genre's color depends on all those with a lower id.
async fn library_genres(
    state: &PlexState,
    section: i64,
) -> Result<(String, String, Vec<(i64, String)>), ApiError> {
    let mut genres = Vec::new();
    loop {
        let mut variables = Variables::default();
        variables.insert(
            Name::new("id"),
            Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
        );
        variables.insert(Name::new("first"), Value::Number(MAX_ITEMS.into()));
        variables.insert(Name::new("offset"), Value::Number(genres.len().into()));
        let data: GenresData = gql::execute(&state.schema, GENRES_QUERY, variables).await?;
        let library = data.node.ok_or(ApiError::NotFound)?;
        let fetched = library.tags.edges.len();
        genres.extend(
            library
                .tags
                .edges
                .into_iter()
                .map(|edge| (database_id(&edge.node.id), edge.node.name)),
        );
        if fetched == 0 || genres.len() as u64 >= library.stats.total {
            return Ok((library.title, library.kind, genres));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_genres_keep_their_color_and_the_others_take_turns_by_id() {
        // Library 9 of the reference, by name, with the colors it answered.
        let genres = [
            (38, "Action"),
            (122, "Adventure"),
            (960, "Animation"),
            (5365, "Biography"),
            (1345, "Children"),
            (4025, "Comédie"),
            (198, "Crime"),
            (4023, "Drame"),
            (4026, "Familial"),
            (4034, "Fantastique"),
            (8058, "Guerre"),
            (6898, "Histoire"),
            (5820, "Horreur"),
            (13_776, "Musical"),
            (6899, "Musique"),
            (4028, "Mystère"),
            (75, "Romance"),
            (379, "Science-Fiction"),
            (1002, "Sport"),
            (311, "Suspense"),
            (9708, "Téléfilm"),
            (39, "Thriller"),
        ];
        assert_eq!(
            blend_colors(&genres),
            [
                "2B717E", "494616", "347431", "4C561B", "883E7D", "8B4D43", "33486E", "8B6122",
                "23533C", "91293D", "23533C", "8B6122", "883E7D", "3B8265", "8B4D43", "494616",
                "892959", "673E8B", "4C561B", "91293D", "494616", "57222B",
            ]
        );
    }

    #[test]
    fn a_genre_without_a_color_of_its_own_starts_the_cycle() {
        // Genres given to the show of the reference's test library 28, which has no art:
        // Fantasy, absent from the libraries above, takes the turn Adventure takes there.
        let genres = [
            (38, "Action"),
            (58, "Comedy"),
            (123, "Fantasy"),
            (988_939, "Kung Fu"),
            (988_940, "Zorglub"),
        ];
        assert_eq!(
            blend_colors(&genres),
            ["2B717E", "51284C", "494616", "91293D", "4C561B"]
        );
        assert_eq!(blend_colors(&[(988_939, "Kung Fu")]), ["494616"]);
    }

    #[test]
    fn matches_a_name_in_lower_case_with_hyphens_for_spaces() {
        assert_eq!(named_color("Science Fiction"), Some("673E8B"));
        assert_eq!(named_color("Science-Fiction"), Some("673E8B"));
        assert_eq!(named_color("Adventure"), None);
    }
}
