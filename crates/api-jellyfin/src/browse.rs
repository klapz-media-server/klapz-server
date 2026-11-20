//! Browsing by what items share — genres, people, studios, years — and the search box's hints.
//!
//! Each is a page of the official API's values: a library's tags, its items' studios and
//! release years, the people credited, the search. Genres and people are tags with ids of their
//! own; a studio and a year are no entity there, so their Jellyfin ids carry the value itself:
//! a year its number, a studio a hash of its name, found again among the studios listed.

use axum::extract::{Path, RawQuery, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::dto::{item_type, ItemsResult};
use crate::error::JellyfinError;
use crate::gql;
use crate::id;
use crate::query::Query;
use crate::response::json;
use crate::time;
use crate::views::{libraries, Library};
use crate::JellyfinState;

/// The id type of a studio, which the official API knows by name alone.
pub const STUDIO_TYPE: &str = "Studio";
/// The id type of a release year.
pub const YEAR_TYPE: &str = "Year";

pub fn router() -> Router<JellyfinState> {
    Router::new()
        .route("/genres", get(genres))
        .route("/genres/{name}", get(genre))
        .route("/persons", get(persons))
        .route("/persons/{name}", get(person))
        .route("/studios", get(studios))
        .route("/studios/{name}", get(studio))
        .route("/years", get(years))
        .route("/years/{year}", get(year))
        .route("/search/hints", get(search_hints))
}

/// A value items share, as the reference lists one: a folder of nothing but its name.
fn shared_dto(server_id: &str, jellyfin_id: &str, name: &str, kind: &str) -> Map<String, Value> {
    let mut dto = Map::new();
    dto.insert("Name".to_owned(), json!(name));
    dto.insert("ServerId".to_owned(), json!(server_id));
    dto.insert("Id".to_owned(), json!(jellyfin_id));
    dto.insert("ChannelId".to_owned(), Value::Null);
    dto.insert("Type".to_owned(), json!(kind));
    if kind != "Genre" {
        dto.insert(
            "UserData".to_owned(),
            json!({
                "PlaybackPositionTicks": 0,
                "PlayCount": 0,
                "IsFavorite": false,
                "Played": false,
                "Key": format!("{kind}-{name}"),
                "ItemId": jellyfin_id,
            }),
        );
    }
    dto.insert("ImageTags".to_owned(), json!({}));
    dto.insert("BackdropImageTags".to_owned(), json!([]));
    dto.insert("ImageBlurHashes".to_owned(), json!({}));
    dto.insert("LocationType".to_owned(), json!("FileSystem"));
    dto.insert("MediaType".to_owned(), json!("Unknown"));
    dto
}

/// A page of a sorted list, as the reference windows one.
fn window(query: &Query, items: Vec<Value>) -> Result<Response, JellyfinError> {
    let start = query.count("startIndex")?.unwrap_or(0);
    let limit = query
        .count("limit")?
        .unwrap_or(crate::listing::MAX_PAGE)
        .min(crate::listing::MAX_PAGE);
    let total = i64::try_from(items.len()).unwrap_or(i64::MAX);
    let items = items
        .into_iter()
        .skip(usize::try_from(start).unwrap_or(usize::MAX))
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .collect();
    Ok(json(&ItemsResult {
        items,
        total_record_count: total,
        start_index: start,
    }))
}

/// The libraries a request covers: the one `parentId` names, else every one of the caller's
/// whose items are of the types it includes. Filters these pages cannot apply are refused.
async fn covered(state: &JellyfinState, query: &Query) -> Result<Vec<Library>, JellyfinError> {
    gql::viewer(&state.schema)
        .await?
        .require_self(query.get("userId"))?;
    if query.flag("isFavorite")? == Some(true) {
        // Favorites are not kept: none is favorite.
        return Ok(Vec::new());
    }
    for refused in [
        "excludeItemTypes",
        "mediaTypes",
        "nameStartsWith",
        "nameLessThan",
        "nameStartsWithOrGreater",
    ] {
        if query.get(refused).is_some() {
            return Err(JellyfinError::NotImplemented(
                "a filter of a page of shared values",
            ));
        }
    }
    let types: Vec<String> = query
        .list("includeItemTypes")
        .iter()
        .map(|kind| kind.to_ascii_lowercase())
        .collect();
    let holds = |library: &Library| {
        types.is_empty()
            || types.iter().any(|kind| match library.kind.as_str() {
                "MOVIE" => kind == "movie",
                _ => matches!(kind.as_str(), "series" | "season" | "episode"),
            })
    };
    let all = libraries(state).await?;
    match query.get("parentId") {
        Some(parent) => {
            let global = id::global_of_type(parent, open_plex_api::library::LIBRARY_TYPE)
                .map_err(|_| JellyfinError::NotImplemented("shared values below a library"))?;
            Ok(all
                .into_iter()
                .filter(|library| library.id == global && holds(library))
                .collect())
        }
        None => Ok(all.into_iter().filter(|library| holds(library)).collect()),
    }
}

#[derive(Deserialize)]
struct NodeData<T> {
    node: Option<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Genre {
    id: String,
    name: String,
    movies: i64,
    series: i64,
    episodes: i64,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct LibraryGenres {
    tags: Nodes<Genre>,
}

const GENRES: &str = r"query($id: ID!, $term: String) {
    node(id: $id) { ... on Library {
        tags(kind: GENRE, completing: $term, first: 500) { nodes {
            id name
            movies: itemCount(filter: { kind: MOVIE })
            series: itemCount(filter: { kind: SHOW })
            episodes: itemCount(filter: { kind: EPISODE })
        } }
    } }
}";

/// The genres of the libraries a request covers, by name, each once.
async fn genre_list(state: &JellyfinState, query: &Query) -> Result<Vec<Genre>, JellyfinError> {
    let mut genres: Vec<Genre> = Vec::new();
    for library in covered(state, query).await? {
        let data: NodeData<LibraryGenres> = gql::query(
            &state.schema,
            GENRES,
            json!({ "id": library.id, "term": query.get("searchTerm") }),
        )
        .await?;
        for genre in data.node.map(|found| found.tags.nodes).unwrap_or_default() {
            if !genres.iter().any(|known| known.id == genre.id) {
                genres.push(genre);
            }
        }
    }
    genres.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(genres)
}

fn genre_dto(state: &JellyfinState, genre: &Genre, counts: bool) -> Value {
    let jellyfin_id = id::of_global(&genre.id).unwrap_or_default();
    let mut dto = shared_dto(&state.server.id, &jellyfin_id, &genre.name, "Genre");
    if counts {
        // As the reference counts with `Fields=ItemCounts`, music and Live TV holding nothing.
        let total = genre.movies + genre.series + genre.episodes;
        for (key, count) in [
            ("ChildCount", total),
            ("TrailerCount", 0),
            ("MovieCount", genre.movies),
            ("SeriesCount", genre.series),
            ("ProgramCount", 0),
            ("EpisodeCount", genre.episodes),
            ("SongCount", 0),
            ("AlbumCount", 0),
            ("ArtistCount", 0),
            ("MusicVideoCount", 0),
        ] {
            dto.insert(key.to_owned(), json!(count));
        }
    }
    Value::Object(dto)
}

fn wants_counts(query: &Query) -> bool {
    query
        .list("fields")
        .iter()
        .any(|field| field.eq_ignore_ascii_case("ItemCounts"))
}

async fn genres(
    State(state): State<JellyfinState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    let counts = wants_counts(&query);
    let items = genre_list(&state, &query)
        .await?
        .iter()
        .map(|genre| genre_dto(&state, genre, counts))
        .collect();
    window(&query, items)
}

async fn genre(
    State(state): State<JellyfinState>,
    Path(name): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    let found = genre_list(&state, &query)
        .await?
        .into_iter()
        .find(|genre| genre.name.eq_ignore_ascii_case(&name))
        .ok_or(JellyfinError::NotFound)?;
    Ok(json(&genre_dto(&state, &found, true)))
}

/// The roles a request names, in the official API's tag kinds.
fn person_kinds(query: &Query) -> Result<Vec<&'static str>, JellyfinError> {
    let all = ["ACTOR", "DIRECTOR", "WRITER", "PRODUCER"];
    let kind = |name: &str| match name.to_ascii_lowercase().as_str() {
        "actor" | "gueststar" => Ok("ACTOR"),
        "director" => Ok("DIRECTOR"),
        "writer" => Ok("WRITER"),
        "producer" => Ok("PRODUCER"),
        _ => Err(JellyfinError::NotImplemented("a role no credit holds here")),
    };
    let mut kinds: Vec<&'static str> = if query.list("personTypes").is_empty() {
        all.to_vec()
    } else {
        query
            .list("personTypes")
            .iter()
            .map(|name| kind(name))
            .collect::<Result<_, _>>()?
    };
    let excluded = query
        .list("excludePersonTypes")
        .iter()
        .map(|name| kind(name))
        .collect::<Result<Vec<_>, _>>()?;
    kinds.retain(|kind| !excluded.contains(kind));
    kinds.dedup();
    Ok(kinds)
}

#[derive(Debug, Deserialize)]
struct PersonRef {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct TagWithPerson {
    person: Option<PersonRef>,
}

#[derive(Deserialize)]
struct TagUse {
    tag: TagWithPerson,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchedTags {
    search_tags: Nodes<TagUse>,
}

#[derive(Deserialize)]
struct KindTags {
    tags: Nodes<TagWithPerson>,
}

/// People credited in the roles a request names, each once, by name: those a term finds, or
/// every one of a single role. Every role at once, with no term, is more people than one page
/// of the official API's credits lists by name.
async fn person_list(
    state: &JellyfinState,
    query: &Query,
) -> Result<Vec<PersonRef>, JellyfinError> {
    gql::viewer(&state.schema)
        .await?
        .require_self(query.get("userId"))?;
    let kinds = person_kinds(query)?;
    let tags = match (query.get("searchTerm"), kinds.as_slice()) {
        (Some(term), _) => {
            let data: SearchedTags = gql::query(
                &state.schema,
                r"query($term: String!, $kinds: [TagType!]) {
                    searchTags(query: $term, kinds: $kinds, first: 500) {
                        nodes { tag { person { id name } } }
                    }
                }",
                json!({ "term": term, "kinds": kinds }),
            )
            .await?;
            data.search_tags
                .nodes
                .into_iter()
                .map(|used| used.tag)
                .collect()
        }
        (None, [kind]) => {
            let data: KindTags = gql::query(
                &state.schema,
                "query($kind: TagType!) { tags(kind: $kind, first: 500) { nodes { person { id name } } } }",
                json!({ "kind": kind }),
            )
            .await?;
            data.tags.nodes
        }
        (None, _) => {
            return Err(JellyfinError::NotImplemented(
                "people of every role at once, without a search term",
            ))
        }
    };
    let mut people: Vec<PersonRef> = Vec::new();
    for person in tags.into_iter().filter_map(|tag| tag.person) {
        if !people.iter().any(|known| known.id == person.id) {
            people.push(person);
        }
    }
    people.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(people)
}

fn person_dto(state: &JellyfinState, person: &PersonRef) -> Value {
    let jellyfin_id = id::of_global(&person.id).unwrap_or_default();
    Value::Object(shared_dto(
        &state.server.id,
        &jellyfin_id,
        &person.name,
        "Person",
    ))
}

async fn persons(
    State(state): State<JellyfinState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    if query.flag("isFavorite")? == Some(true) {
        return window(&query, Vec::new());
    }
    let items = person_list(&state, &query)
        .await?
        .iter()
        .map(|person| person_dto(&state, person))
        .collect();
    window(&query, items)
}

async fn person(
    State(state): State<JellyfinState>,
    Path(name): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let mut query = Query::parse(raw.as_deref());
    query.set("searchTerm", &name);
    let found = person_list(&state, &query)
        .await?
        .into_iter()
        .find(|person| person.name.eq_ignore_ascii_case(&name))
        .ok_or(JellyfinError::NotFound)?;
    Ok(json(&person_dto(&state, &found)))
}

/// A studio's id: a 64-bit FNV-1a hash of its name, which only the studios listed can resolve.
fn studio_id(name: &str) -> String {
    let hash = name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    id::of(STUDIO_TYPE, i64::from_be_bytes(hash.to_be_bytes())).unwrap_or_default()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryValues {
    field_values: Nodes<String>,
    years: Nodes<i32>,
}

const VALUES: &str = r"query($id: ID!) {
    node(id: $id) { ... on Library {
        fieldValues(field: STUDIO, first: 500) { nodes }
        years(first: 500) { nodes }
    } }
}";

/// The studios and the release years of the libraries a request covers.
async fn values(
    state: &JellyfinState,
    query: &Query,
) -> Result<(Vec<String>, Vec<i32>), JellyfinError> {
    let mut studios = Vec::new();
    let mut years = Vec::new();
    for library in covered(state, query).await? {
        let data: NodeData<LibraryValues> =
            gql::query(&state.schema, VALUES, json!({ "id": library.id })).await?;
        if let Some(found) = data.node {
            studios.extend(found.field_values.nodes);
            years.extend(found.years.nodes);
        }
    }
    studios.sort_by_key(|studio| studio.to_lowercase());
    studios.dedup();
    years.sort_unstable();
    years.dedup();
    Ok((studios, years))
}

fn studio_dto(state: &JellyfinState, name: &str) -> Value {
    Value::Object(shared_dto(
        &state.server.id,
        &studio_id(name),
        name,
        "Studio",
    ))
}

async fn studios(
    State(state): State<JellyfinState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    let term = query.get("searchTerm").map(str::to_lowercase);
    let items = values(&state, &query)
        .await?
        .0
        .iter()
        .filter(|studio| {
            term.as_deref()
                .is_none_or(|term| studio.to_lowercase().contains(term))
        })
        .map(|studio| studio_dto(&state, studio))
        .collect();
    window(&query, items)
}

async fn studio(
    State(state): State<JellyfinState>,
    Path(name): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    let found = values(&state, &query)
        .await?
        .0
        .into_iter()
        .find(|studio| studio.eq_ignore_ascii_case(&name))
        .ok_or(JellyfinError::NotFound)?;
    Ok(json(&studio_dto(&state, &found)))
}

fn year_dto(state: &JellyfinState, year: i32) -> Value {
    let jellyfin_id = id::of(YEAR_TYPE, i64::from(year)).unwrap_or_default();
    Value::Object(shared_dto(
        &state.server.id,
        &jellyfin_id,
        &year.to_string(),
        "Year",
    ))
}

async fn years(
    State(state): State<JellyfinState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    let descending = query
        .get("sortOrder")
        .is_some_and(|order| order.eq_ignore_ascii_case("descending"));
    let mut years = values(&state, &query).await?.1;
    if descending {
        years.reverse();
    }
    let items = years.iter().map(|year| year_dto(&state, *year)).collect();
    window(&query, items)
}

async fn year(
    State(state): State<JellyfinState>,
    Path(year): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    let year: i32 = year
        .parse()
        .map_err(|_| JellyfinError::BadRequest("a year is not a number"))?;
    if !values(&state, &query).await?.1.contains(&year) {
        return Err(JellyfinError::NotFound);
    }
    Ok(json(&year_dto(&state, year)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hit {
    #[serde(rename = "__typename")]
    type_name: String,
    id: String,
    title: Option<String>,
    year: Option<i32>,
    duration_ms: Option<i64>,
    number: Option<i32>,
    season: Option<SeasonRef>,
    show: Option<ShowRef>,
}

#[derive(Debug, Deserialize)]
struct SeasonRef {
    number: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct ShowRef {
    title: Option<String>,
}

#[derive(Deserialize)]
struct Searched {
    search: Nodes<Hit>,
}

const SEARCH: &str = r"query($term: String!, $kinds: [ItemType!], $first: Int) {
    search(query: $term, kinds: $kinds, first: $first) { nodes {
        __typename id title
        ... on Movie { year durationMs }
        ... on Show { year }
        ... on Season { number show { title } }
        ... on Episode { number year durationMs season { number } show { title } }
    } }
}";

/// One item as the search box hints it.
fn item_hint(hit: &Hit) -> Value {
    let jellyfin_id = id::of_global(&hit.id).unwrap_or_default();
    let playable = matches!(hit.type_name.as_str(), "Movie" | "Episode");
    let mut shaped = Map::new();
    shaped.insert("ItemId".to_owned(), json!(jellyfin_id));
    shaped.insert("Id".to_owned(), json!(jellyfin_id));
    shaped.insert(
        "Name".to_owned(),
        json!(hit.title.clone().unwrap_or_default()),
    );
    if let Some(year) = hit.year {
        shaped.insert("ProductionYear".to_owned(), json!(year));
    }
    if hit.type_name == "Episode" {
        if let Some(number) = hit.number {
            shaped.insert("IndexNumber".to_owned(), json!(number));
        }
        if let Some(season) = hit.season.as_ref().and_then(|season| season.number) {
            shaped.insert("ParentIndexNumber".to_owned(), json!(season));
        }
    }
    if let Some(series) = hit.show.as_ref().and_then(|show| show.title.clone()) {
        shaped.insert("Series".to_owned(), json!(series));
    }
    shaped.insert("Type".to_owned(), json!(item_type(&hit.type_name)));
    if let Some(duration) = hit.duration_ms {
        shaped.insert("RunTimeTicks".to_owned(), json!(time::ticks(duration)));
    }
    shaped.insert(
        "MediaType".to_owned(),
        json!(if playable { "Video" } else { "Unknown" }),
    );
    shaped.insert("Artists".to_owned(), json!([]));
    shaped.insert("ChannelId".to_owned(), Value::Null);
    Value::Object(shaped)
}

fn person_hint(person: &PersonRef) -> Value {
    let jellyfin_id = id::of_global(&person.id).unwrap_or_default();
    json!({
        "ItemId": jellyfin_id,
        "Id": jellyfin_id,
        "Name": person.name,
        "Type": "Person",
        "MediaType": "Unknown",
        "Artists": [],
        "ChannelId": null,
    })
}

/// `/Search/Hints`: items whose title the term begins a word of, ranked by the official API's
/// search, then the people it names unless `includePeople=false`.
async fn search_hints(
    State(state): State<JellyfinState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    gql::viewer(&state.schema)
        .await?
        .require_self(query.get("userId"))?;
    for refused in [
        "parentId",
        "mediaTypes",
        "isMovie",
        "isSeries",
        "isNews",
        "isKids",
        "isSports",
    ] {
        if query.get(refused).is_some() {
            return Err(JellyfinError::NotImplemented("a search hint filter"));
        }
    }
    if query.count("startIndex")?.unwrap_or(0) > 0 {
        return Err(JellyfinError::NotImplemented(
            "a page of hints past the first",
        ));
    }
    let limit = query
        .count("limit")?
        .unwrap_or(crate::listing::MAX_PAGE)
        .min(crate::listing::MAX_PAGE);
    let term = query.get("searchTerm").unwrap_or_default().to_owned();
    let listing = crate::listing::Listing::parse(&Query::parse(Some(&format!(
        "includeItemTypes={}&excludeItemTypes={}",
        query.list("includeItemTypes").join(","),
        query.list("excludeItemTypes").join(","),
    ))))?;
    let mut hints: Vec<Value> = Vec::new();
    if !term.trim().is_empty() && !listing.kinds.is_empty() {
        let data: Searched = gql::query(
            &state.schema,
            SEARCH,
            json!({ "term": term, "kinds": listing.kinds, "first": limit }),
        )
        .await?;
        hints.extend(data.search.nodes.iter().map(item_hint));
    }
    let types = query.list("includeItemTypes");
    let people_asked = query.flag("includePeople")? != Some(false)
        && (types.is_empty() || types.iter().any(|kind| kind.eq_ignore_ascii_case("person")));
    if people_asked && !term.trim().is_empty() {
        let room = usize::try_from(limit)
            .unwrap_or(usize::MAX)
            .saturating_sub(hints.len());
        let mut people = Query::default();
        people.set("searchTerm", &term);
        hints.extend(
            person_list(&state, &people)
                .await?
                .iter()
                .take(room)
                .map(person_hint),
        );
    }
    Ok(json(&json!({
        "SearchHints": hints,
        "TotalRecordCount": hints.len(),
    })))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_studio_is_known_by_a_hash_of_its_name() {
        let amblin = studio_id("Amblin Entertainment");
        assert_eq!(amblin, studio_id("Amblin Entertainment"));
        assert_ne!(amblin, studio_id("Universal Pictures"));
        assert_eq!(id::decode(&amblin).unwrap().type_name, STUDIO_TYPE);
    }

    #[test]
    fn a_shared_value_is_a_bare_folder_and_a_genre_carries_no_user_data() {
        let year = shared_dto("s", "y", "1997", "Year");
        assert_eq!(year["UserData"]["Key"], "Year-1997");
        let genre = shared_dto("s", "g", "Drame", "Genre");
        assert!(genre.get("UserData").is_none());
        assert_eq!(genre["MediaType"], "Unknown");
    }

    #[test]
    fn roles_are_read_and_excluded() {
        let query = Query::parse(Some(
            "personTypes=Actor,Director&excludePersonTypes=Director",
        ));
        assert_eq!(person_kinds(&query).unwrap(), ["ACTOR"]);
        assert_eq!(
            person_kinds(&Query::default()).unwrap(),
            ["ACTOR", "DIRECTOR", "WRITER", "PRODUCER"]
        );
        assert!(person_kinds(&Query::parse(Some("personTypes=Composer"))).is_err());
    }
}
