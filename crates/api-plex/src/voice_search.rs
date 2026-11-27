//! `GET /hubs/search/voice`: titles ranked by edit distance to a spoken term, as shelves.
//!
//! Not derived from `/hubs/search`: a voice assistant hears `kaamelott` as it can, and a word
//! prefix would find nothing. The ranking is the official API's `closestTitles`; this binding
//! cuts it to `limit` and groups it into one shelf per kind, ordered by their closest match,
//! films before series when they tie. Measured on the reference and listed in
//! `docs/plex/spec-gaps.md`.

use std::collections::HashMap;
use std::sync::LazyLock;

use async_graphql::{Name, Value, Variables};
use axum::extract::{RawQuery, State};
use axum::routing::get;
use axum::Router;
use klapz_core::closest::MAX_TERM_BYTES;
use klapz_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{with_shared_fragments, GqlItem};
use crate::items::MetadataEntry;
use crate::language::Language;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/hubs/search/voice", get(voice_search))
}

/// Matches in all when the client does not say: the reference's `limit` counts across the
/// shelves, though the spec says per shelf.
const DEFAULT_LIMIT: u32 = 3;

static VOICE_QUERY: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexVoiceSearch($query: String!, $kinds: [ItemType!]!, $first: Int!) {
  closestTitles(query: $query, kinds: $kinds, first: $first) {
    edges { node { distance item { ...ListedItem } } }
  }
}
",
    )
});

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoiceData {
    closest_titles: MatchEdges,
}

#[derive(Debug, Deserialize)]
struct MatchEdges {
    edges: Vec<MatchEdge>,
}

#[derive(Debug, Deserialize)]
struct MatchEdge {
    node: Match,
}

#[derive(Debug, Deserialize)]
struct Match {
    distance: u32,
    item: GqlItem,
}

/// The kinds a shelf can hold, in the order shelves fall back on when their closest matches
/// tie: `tom hanks` lists the film at 4 before the series at 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Kind {
    Movie,
    Show,
    Season,
    Episode,
}

impl Kind {
    /// The Plex type number, which names the shelf: `results.search.1` holds the films.
    const fn number(self) -> u8 {
        match self {
            Self::Movie => 1,
            Self::Show => 2,
            Self::Season => 3,
            Self::Episode => 4,
        }
    }

    fn of_number(number: i64) -> Option<Self> {
        Some(match number {
            1 => Self::Movie,
            2 => Self::Show,
            3 => Self::Season,
            4 => Self::Episode,
            _ => return None,
        })
    }

    /// The shelf's `title` and `type`, the same word, in English whatever the client's
    /// language.
    const fn word(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Show => "show",
            Self::Season => "season",
            Self::Episode => "episode",
        }
    }

    fn of_word(word: &str) -> Option<Self> {
        [Self::Movie, Self::Show, Self::Season, Self::Episode]
            .into_iter()
            .find(|kind| kind.word() == word)
    }

    /// The official API's name for it.
    const fn item_type(self) -> &'static str {
        match self {
            Self::Movie => "MOVIE",
            Self::Show => "SHOW",
            Self::Season => "SEASON",
            Self::Episode => "EPISODE",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Request {
    term: String,
    limit: u32,
    /// Empty for a type this server holds nothing of, which answers no shelf.
    kinds: Vec<Kind>,
}

impl Request {
    /// What the reference refuses `400` is refused here too: no term or an empty one, a
    /// `limit` or a `type` that is not a number, a list of types. A negative `limit`, which
    /// the reference answers `500`, is a `400`, and so is a term past [`MAX_TERM_BYTES`],
    /// which it would rank. A blank term is a term: ` ` ranks the shortest titles.
    fn parse(query: &str) -> Result<Self, ApiError> {
        let params: HashMap<_, _> = form_urlencoded::parse(query.as_bytes()).collect();
        let term = params
            .get("query")
            .filter(|term| !term.is_empty() && term.len() <= MAX_TERM_BYTES)
            .ok_or(ApiError::BadRequest)?
            .to_string();
        let limit = match params.get("limit") {
            Some(limit) => limit.parse::<u32>().map_err(|_| ApiError::BadRequest)?,
            None => DEFAULT_LIMIT,
        };
        let kinds = match params.get("type") {
            Some(number) => {
                let number: i64 = number.parse().map_err(|_| ApiError::BadRequest)?;
                Kind::of_number(number).into_iter().collect()
            }
            None => vec![Kind::Movie, Kind::Show],
        };
        Ok(Self {
            term,
            limit: limit.min(MAX_ITEMS),
            kinds,
        })
    }
}

/// Response to `GET /hubs/search/voice`: the shelves alone, no other attribute.
#[derive(Debug, Serialize)]
struct VoiceContainer {
    size: usize,
    #[serde(rename = "Hub", skip_serializing_if = "Vec::is_empty")]
    hubs: Vec<VoiceHub>,
}

/// One kind's matches, closest first.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceHub {
    /// Its closest match's.
    distance: u32,
    /// The matches, as one lookup.
    hub_key: String,
    title: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    hub_identifier: String,
    context: &'static str,
    size: usize,
    more: bool,
    style: &'static str,
    #[serde(rename = "Metadata")]
    items: Vec<VoiceItem>,
}

/// An item as `/library/search` lists it, with its library and its distance to the term.
#[derive(Debug, Serialize)]
struct VoiceItem {
    distance: u32,
    #[serde(flatten)]
    metadata: MetadataEntry,
}

/// The shelves of a ranking, each kind's matches in rank order.
fn shelves(matches: Vec<(Kind, u32, MetadataEntry)>) -> Vec<VoiceHub> {
    let mut grouped: Vec<(Kind, Vec<VoiceItem>)> = Vec::new();
    for (kind, distance, metadata) in matches {
        let item = VoiceItem { distance, metadata };
        match grouped.iter_mut().find(|(shelf, _)| *shelf == kind) {
            Some((_, items)) => items.push(item),
            None => grouped.push((kind, vec![item])),
        }
    }
    let mut hubs: Vec<(Kind, VoiceHub)> = grouped
        .into_iter()
        .map(|(kind, items)| {
            let keys: Vec<&str> = items
                .iter()
                .map(|item| item.metadata.rating_key.as_str())
                .collect();
            (
                kind,
                VoiceHub {
                    distance: items.first().map_or(0, |item| item.distance),
                    hub_key: format!("/library/metadata/{}", keys.join(",")),
                    title: kind.word(),
                    kind: kind.word(),
                    hub_identifier: format!("results.search.{}", kind.number()),
                    context: "hub.results.search",
                    size: items.len(),
                    more: false,
                    style: "shelf",
                    items,
                },
            )
        })
        .collect();
    hubs.sort_by_key(|(kind, hub)| (hub.distance, *kind));
    hubs.into_iter().map(|(_, hub)| hub).collect()
}

async fn voice_search(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    RawQuery(query): RawQuery,
    format: ResponseFormat,
) -> Result<PlexResponse<VoiceContainer>, ApiError> {
    let request = Request::parse(query.as_deref().unwrap_or_default())?;
    if request.kinds.is_empty() || request.limit == 0 {
        return Ok(PlexResponse::new(
            VoiceContainer {
                size: 0,
                hubs: Vec::new(),
            },
            format,
        ));
    }

    let mut variables = Variables::default();
    variables.insert(Name::new("query"), Value::String(request.term));
    variables.insert(
        Name::new("kinds"),
        Value::List(
            request
                .kinds
                .iter()
                .map(|kind| Value::Enum(Name::new(kind.item_type())))
                .collect(),
        ),
    );
    variables.insert(Name::new("first"), Value::Number(request.limit.into()));
    let data: VoiceData = gql::execute(&state.schema, &VOICE_QUERY, variables).await?;

    let language = Language::of(&client);
    let matches = data
        .closest_titles
        .edges
        .into_iter()
        .filter_map(|edge| {
            let mut metadata = MetadataEntry::from_graphql(&edge.node.item, true, language)?;
            let kind = Kind::of_word(metadata.kind)?;
            // Empty rather than absent, as on `/hubs/search`.
            if kind == Kind::Movie {
                metadata.summary.get_or_insert_with(String::new);
            }
            Some((kind, edge.node.distance, metadata))
        })
        .collect();
    let hubs = shelves(matches);
    Ok(PlexResponse::new(
        VoiceContainer {
            size: hubs.len(),
            hubs,
        },
        format,
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_search_needs_a_term_and_numbers() {
        for query in [
            "",
            "query=",
            "query=dune&limit=abc",
            "query=dune&limit=-1",
            "query=dune&type=abc",
            "query=dune&type=1,2",
        ] {
            assert!(
                matches!(Request::parse(query), Err(ApiError::BadRequest)),
                "{query}"
            );
        }
        let long = format!("query={}", "a".repeat(MAX_TERM_BYTES + 1));
        assert!(matches!(Request::parse(&long), Err(ApiError::BadRequest)));
    }

    #[test]
    fn ranks_films_and_series_three_at_a_time_by_default() {
        let request = Request::parse("query=%20&sectionId=abc").unwrap();
        assert_eq!(
            request,
            Request {
                term: " ".to_owned(),
                limit: 3,
                kinds: vec![Kind::Movie, Kind::Show],
            }
        );
        let request = Request::parse("query=pilot&type=4&limit=9999").unwrap();
        assert_eq!(
            (request.limit, request.kinds),
            (MAX_ITEMS, vec![Kind::Episode])
        );
        // A type the server holds nothing of is not an error, and finds nothing.
        assert!(Request::parse("query=dune&type=8")
            .unwrap()
            .kinds
            .is_empty());
    }
}
