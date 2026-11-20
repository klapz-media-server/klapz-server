//! `/Items/Filters` and `/Items/Filters2`: what a library's items can be filtered by, as the
//! filter dialogs of Jellyfin's clients list it.
//!
//! Every value is the official API's: a library's genres, its items' release years and content
//! ratings, and the languages its tracks state. Tags (keywords) come from a metadata provider,
//! which nothing here consults: the list is empty, as the reference's is for an item no
//! provider matched.

use std::collections::BTreeSet;

use axum::extract::{RawQuery, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::JellyfinError;
use crate::gql;
use crate::id;
use crate::language;
use crate::query::Query;
use crate::response::json;
use crate::views::libraries;
use crate::JellyfinState;

pub fn router() -> Router<JellyfinState> {
    Router::new()
        .route("/items/filters", get(legacy_filters))
        .route("/items/filters2", get(filters))
}

const FILTER_VALUES: &str = r"query($id: ID!) {
    node(id: $id) { ... on Library {
        kind
        genres: tags(kind: GENRE, first: 500) { nodes { id name } }
        years(first: 500) { nodes }
        ratings: fieldValues(field: CONTENT_RATING, first: 500) { nodes }
        audio: trackValues(field: AUDIO_LANGUAGE, first: 500) { nodes }
        subtitles: trackValues(field: SUBTITLE_LANGUAGE, first: 500) { nodes }
    } }
}";

#[derive(Deserialize)]
struct NodeData {
    node: Option<LibraryValues>,
}

#[derive(Debug, Default, Deserialize)]
struct LibraryValues {
    genres: Nodes<Genre>,
    years: Nodes<i32>,
    ratings: Nodes<String>,
    audio: Nodes<String>,
    subtitles: Nodes<String>,
}

#[derive(Debug, Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

impl<T> Default for Nodes<T> {
    fn default() -> Self {
        Self { nodes: Vec::new() }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Genre {
    id: String,
    name: String,
}

/// What every library the request names holds, merged.
#[derive(Debug, Default)]
struct Values {
    genres: Vec<Genre>,
    years: BTreeSet<i32>,
    ratings: BTreeSet<String>,
    audio: BTreeSet<String>,
    subtitles: BTreeSet<String>,
}

impl Values {
    fn add(&mut self, library: LibraryValues) {
        for genre in library.genres.nodes {
            if !self.genres.iter().any(|known| known.name == genre.name) {
                self.genres.push(genre);
            }
        }
        self.years.extend(library.years.nodes);
        self.ratings.extend(library.ratings.nodes);
        self.audio.extend(library.audio.nodes);
        self.subtitles.extend(library.subtitles.nodes);
    }

    /// Genres by name, as the reference sorts them.
    fn genres(&self) -> Vec<&Genre> {
        let mut genres: Vec<&Genre> = self.genres.iter().collect();
        genres.sort_by(|a, b| a.name.cmp(&b.name));
        genres
    }
}

/// The libraries a request covers: the one `parentId` names, else every one of the caller's.
/// What the official API cannot narrow to — below a library, to some types of its items, to
/// what is on the air — is refused rather than answered for the whole library.
async fn values(state: &JellyfinState, query: &Query) -> Result<Values, JellyfinError> {
    gql::viewer(&state.schema)
        .await?
        .require_self(query.get("userId"))?;
    for flag in [
        "isAiring", "isMovie", "isSports", "isKids", "isNews", "isSeries",
    ] {
        if query.flag(flag)?.is_some() {
            return Err(JellyfinError::NotImplemented(
                "filters of what is on the air",
            ));
        }
    }
    let media_types = query.list("mediaTypes");
    if !media_types.is_empty()
        && !media_types
            .iter()
            .any(|kind| kind.eq_ignore_ascii_case("video"))
    {
        // Music and photos are held nowhere.
        return Ok(Values::default());
    }
    let mut libraries_named = match query.get("parentId") {
        Some(parent) => {
            let global = id::global_of_type(parent, klapz_api::library::LIBRARY_TYPE)
                .map_err(|_| JellyfinError::NotImplemented("filters below a library"))?;
            vec![(global, None)]
        }
        None => libraries(state)
            .await?
            .into_iter()
            .map(|library| (library.id, Some(library.kind)))
            .collect(),
    };
    let types = query.list("includeItemTypes");
    let mut values = Values::default();
    for (library, kind) in libraries_named.drain(..) {
        let data: NodeData =
            gql::query(&state.schema, FILTER_VALUES, json!({ "id": library })).await?;
        let Some(found) = data.node else {
            return Err(JellyfinError::NotFound);
        };
        let kind = kind.unwrap_or_default();
        let top_level = |wanted: &str| {
            matches!(
                (kind.as_str(), wanted.to_ascii_lowercase().as_str()),
                (_, "") | ("MOVIE" | "", "movie") | ("SHOW" | "", "series")
            )
        };
        if !types.is_empty() && !types.iter().all(|wanted| top_level(wanted)) {
            return Err(JellyfinError::NotImplemented(
                "filters of a kind of item below a library's own",
            ));
        }
        values.add(found);
    }
    Ok(values)
}

/// `/Items/Filters`: names only, as the clients before 10.6 read them.
async fn legacy_filters(
    State(state): State<JellyfinState>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(query.as_deref());
    let values = values(&state, &query).await?;
    let genres: Vec<&str> = values
        .genres()
        .iter()
        .map(|genre| genre.name.as_str())
        .collect();
    Ok(json(&json!({
        "Genres": genres,
        "Tags": [],
        "OfficialRatings": values.ratings,
        "Years": values.years,
    })))
}

/// A language a track states, as the reference names it in a filter: its English name and the
/// code as written, `French (fre)`; the code alone when it names no language it knows.
fn language_choice(code: &str) -> Value {
    let name = language::localized_name(language::three_letters(code))
        .filter(|_| !code.eq_ignore_ascii_case("und"))
        .map_or_else(|| code.to_owned(), |name| format!("{name} ({code})"));
    json!({ "Name": name, "Value": code })
}

/// `/Items/Filters2`: genres with their ids, and the languages tracks state.
async fn filters(
    State(state): State<JellyfinState>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(query.as_deref());
    let values = values(&state, &query).await?;
    let genres: Vec<Value> = values
        .genres()
        .iter()
        .map(|genre| json!({ "Name": genre.name, "Id": id::of_global(&genre.id).unwrap_or_default() }))
        .collect();
    let audio: Vec<Value> = values
        .audio
        .iter()
        .map(|code| language_choice(code))
        .collect();
    let subtitles: Vec<Value> = values
        .subtitles
        .iter()
        .map(|code| language_choice(code))
        .collect();
    Ok(json(&json!({
        "Genres": genres,
        "Tags": [],
        "AudioLanguages": audio,
        "SubtitleLanguages": subtitles,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stated_language_is_named_with_its_code_as_written() {
        assert_eq!(language_choice("fre")["Name"], "French (fre)");
        assert_eq!(language_choice("fra")["Name"], "French (fra)");
        assert_eq!(language_choice("eng")["Name"], "English (eng)");
        assert_eq!(language_choice("qaa")["Name"], "qaa");
        assert_eq!(language_choice("fra,fra")["Name"], "fra,fra");
        assert_eq!(language_choice("fre")["Value"], "fre");
    }

    #[test]
    fn genres_are_merged_by_name_and_sorted() {
        let mut values = Values::default();
        let genre = |id: &str, name: &str| Genre {
            id: id.to_owned(),
            name: name.to_owned(),
        };
        values.add(LibraryValues {
            genres: Nodes {
                nodes: vec![genre("a", "Drame"), genre("b", "Animation")],
            },
            ..LibraryValues::default()
        });
        values.add(LibraryValues {
            genres: Nodes {
                nodes: vec![genre("c", "Drame")],
            },
            years: Nodes {
                nodes: vec![2025, 1997],
            },
            ..LibraryValues::default()
        });
        let names: Vec<&str> = values
            .genres()
            .iter()
            .map(|genre| genre.name.as_str())
            .collect();
        assert_eq!(names, ["Animation", "Drame"]);
        assert_eq!(
            values.years.iter().copied().collect::<Vec<_>>(),
            [1997, 2025]
        );
    }
}
