//! What a listing request asks, read into the official API's filter and order.
//!
//! The reference takes some sixty parameters on `/Items`. Those that only shape the answer —
//! images, user data, `Fields` — are read elsewhere or tolerated. Those that narrow the result
//! are honored when the official API can express them, and refused `501` when it cannot:
//! answering without them would hand the client the whole library as if it were the result.

use serde_json::{json, Map, Value};

use crate::error::JellyfinError;
use crate::query::Query;

/// The most items one page holds, the official API's cap.
pub const MAX_PAGE: u32 = 500;

/// Our item kinds by Jellyfin's type names. Jellyfin has other types — `BoxSet`, `Folder`,
/// `Audio`… — of which this server holds none.
pub fn kind_of(jellyfin_type: &str) -> Option<&'static str> {
    match jellyfin_type.to_ascii_lowercase().as_str() {
        "movie" => Some("MOVIE"),
        "series" => Some("SHOW"),
        "season" => Some("SEASON"),
        "episode" => Some("EPISODE"),
        _ => None,
    }
}

/// Filters the reference honors that the official API cannot express.
const REFUSED: &[&str] = &[
    "person",
    "personTypes",
    "studioIds",
    "studios",
    "tags",
    "officialRatings",
    "hasSubtitles",
    "hasTrailer",
    "hasThemeSong",
    "hasThemeVideo",
    "hasSpecialFeature",
    "hasOverview",
    "hasImdbId",
    "hasTmdbId",
    "hasTvdbId",
    "is4K",
    "isHd",
    "is3D",
    "isLocked",
    "isMissing",
    "isUnaired",
    "isPlaceHolder",
    "minCommunityRating",
    "minCriticRating",
    "maxPremiereDate",
    "minDateLastSaved",
    "nameStartsWith",
    "nameStartsWithOrGreater",
    "nameLessThan",
    "excludeItemIds",
    "seriesStatus",
    "videoTypes",
    "minHeight",
    "maxHeight",
    "minWidth",
    "maxWidth",
    "genres",
    "airedDuringSeason",
    "adjacentTo",
    "parentIndexNumber",
    "indexNumber",
    "minIndexNumber",
    "albumIds",
    "artistIds",
];

/// Whether a request asks for favorites alone, by either of the two ways Jellyfin says it.
///
/// # Errors
///
/// [`JellyfinError::BadRequest`] for an `isFavorite` that is no boolean.
pub fn favorites_only(query: &Query) -> Result<bool, JellyfinError> {
    Ok(query
        .list("filters")
        .iter()
        .any(|name| name.eq_ignore_ascii_case("isfavorite"))
        || query.flag("isFavorite")? == Some(true))
}

/// A listing, in the official API's terms.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// `ItemFilterInput`, as JSON.
    pub filter: Map<String, Value>,
    pub order: Option<&'static str>,
    pub direction: Option<&'static str>,
    pub offset: u32,
    pub first: u32,
    /// The kinds the request leaves possible. Empty: it can match nothing held here.
    pub kinds: Vec<&'static str>,
}

impl Listing {
    /// # Errors
    ///
    /// [`JellyfinError::NotImplemented`] for a filter or an order the official API cannot
    /// express, [`JellyfinError::BadRequest`] for a malformed value.
    pub fn parse(query: &Query) -> Result<Self, JellyfinError> {
        if let Some(refused) = REFUSED.iter().find(|name| query.get(name).is_some()) {
            tracing::debug!(parameter = refused, "refused a Jellyfin listing filter");
            return Err(JellyfinError::NotImplemented("a listing filter"));
        }
        let mut filter = Map::new();
        let kinds = kinds(query);
        let recursive = query.flag("recursive")?.unwrap_or(false);
        // A kind is named only when the client includes types: the reference's default is
        // what a library holds at its top.
        if !query.list("includeItemTypes").is_empty() {
            match kinds.as_slice() {
                [kind] => {
                    filter.insert("kind".to_owned(), json!(kind));
                }
                [] => {}
                // The official API's search takes several kinds; a filter takes one.
                [_, _, ..]
                    if query.get("searchTerm").is_some() && query.get("parentId").is_none() => {}
                [_, _, ..] => {
                    return Err(JellyfinError::NotImplemented("several item types at once"));
                }
            }
        }
        if recursive && !filter.contains_key("kind") {
            filter.insert("depth".to_owned(), json!("ALL"));
        }
        let filters: Vec<String> = query
            .list("filters")
            .iter()
            .map(|name| name.to_ascii_lowercase())
            .collect();
        let played = query.flag("isPlayed")?;
        if filters.iter().any(|name| name == "isplayed") || played == Some(true) {
            filter.insert("watched".to_owned(), json!(true));
        }
        if filters.iter().any(|name| name == "isunplayed") || played == Some(false) {
            filter.insert("watched".to_owned(), json!(false));
        }
        if filters.iter().any(|name| name == "isresumable") {
            filter.insert("inProgress".to_owned(), json!(true));
        }
        if favorites_only(query)? {
            filter.insert("favorite".to_owned(), json!(true));
        }
        if filters
            .iter()
            .any(|name| matches!(name.as_str(), "likes" | "dislikes"))
        {
            return Err(JellyfinError::NotImplemented("likes"));
        }
        let years = query
            .list("years")
            .iter()
            .map(|year| year.parse::<i32>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| JellyfinError::BadRequest("a year is not a number"))?;
        if let (Some(&low), Some(&high)) = (years.iter().min(), years.iter().max()) {
            let span = usize::try_from(high - low).unwrap_or(usize::MAX);
            let mut distinct = years.clone();
            distinct.sort_unstable();
            distinct.dedup();
            if span + 1 != distinct.len() {
                return Err(JellyfinError::NotImplemented("years that are no range"));
            }
            filter.insert("yearFrom".to_owned(), json!(low));
            filter.insert("yearTo".to_owned(), json!(high));
        }
        let genres = query
            .list("genreIds")
            .iter()
            .map(|genre| {
                crate::id::global_of_type(genre, klapz_api::tag::TAG_TYPE)
                    .map_err(JellyfinError::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !genres.is_empty() {
            filter.insert(
                "tags".to_owned(),
                json!([{ "kind": "GENRE", "ids": genres }]),
            );
        }
        if let Some(date) = query.get("minPremiereDate") {
            filter.insert(
                "releasedFrom".to_owned(),
                json!(klapz_api::date_time::format(premiere_date(date)?)),
            );
        }
        if let Some(term) = query.get("searchTerm") {
            filter.insert("titleWordPrefix".to_owned(), json!(term));
        }
        let (order, direction) = order(query)?;
        Ok(Self {
            filter,
            order,
            direction,
            offset: query.count("startIndex")?.unwrap_or(0),
            first: query.count("limit")?.unwrap_or(MAX_PAGE).min(MAX_PAGE),
            kinds,
        })
    }

    /// The filter narrowed to one kind, for a listing that walks several libraries.
    pub fn filter_json(&self) -> Value {
        Value::Object(self.filter.clone())
    }
}

/// A date as ASP.NET binds one: RFC 3339, or without an offset, which the reference compares
/// with its UTC dates as if it were UTC, or a day alone, its midnight.
fn premiere_date(text: &str) -> Result<i64, JellyfinError> {
    let unzoned = || format!("{text}Z");
    let day = || format!("{text}T00:00:00Z");
    klapz_api::date_time::parse(text)
        .or_else(|| klapz_api::date_time::parse(&unzoned()))
        .or_else(|| klapz_api::date_time::parse(&day()))
        .ok_or(JellyfinError::BadRequest("a date is not a date"))
}

/// The kinds a request leaves possible: those it includes, or all, less those it excludes.
fn kinds(query: &Query) -> Vec<&'static str> {
    let included = query.list("includeItemTypes");
    let mut kinds: Vec<&'static str> = if included.is_empty() {
        vec!["MOVIE", "SHOW", "SEASON", "EPISODE"]
    } else {
        included.iter().filter_map(|name| kind_of(name)).collect()
    };
    let excluded: Vec<&'static str> = query
        .list("excludeItemTypes")
        .iter()
        .filter_map(|name| kind_of(name))
        .collect();
    kinds.retain(|kind| !excluded.contains(kind));
    kinds.dedup();
    kinds
}

/// The official API's order for the first of the client's sort keys. Later keys break ties
/// the reference breaks; ours breaks them by title, which is what clients send second.
fn order(query: &Query) -> Result<(Option<&'static str>, Option<&'static str>), JellyfinError> {
    let Some(key) = query
        .list("sortBy")
        .first()
        .map(|key| key.to_ascii_lowercase())
    else {
        return Ok((None, None));
    };
    let order = match key.as_str() {
        "sortname" | "name" | "seriessortname" => "TITLE",
        "productionyear" | "premieredate" => "RELEASED_ON",
        "datecreated" => "ADDED_AT",
        "communityrating" => "AUDIENCE_RATING",
        "criticrating" => "RATING",
        "runtime" => "DURATION",
        "dateplayed" => "LAST_VIEWED_AT",
        "random" => "RANDOM",
        _ => return Err(JellyfinError::NotImplemented("a sort order")),
    };
    let direction = match query
        .list("sortOrder")
        .first()
        .map(|direction| direction.to_ascii_lowercase())
        .as_deref()
    {
        None => None,
        Some("ascending") => Some("ASCENDING"),
        Some("descending") => Some("DESCENDING"),
        Some(_) => return Err(JellyfinError::BadRequest("a sort order is neither way")),
    };
    Ok((Some(order), direction))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn listing(raw: &str) -> Result<Listing, JellyfinError> {
        Listing::parse(&Query::parse(Some(raw)))
    }

    #[test]
    fn a_bare_listing_is_the_top_level_paged_by_the_cap() {
        let listing = listing("ParentId=x").unwrap();
        assert!(listing.filter.is_empty());
        assert_eq!((listing.offset, listing.first), (0, MAX_PAGE));
        assert_eq!(listing.order, None);
        assert_eq!(listing.kinds, ["MOVIE", "SHOW", "SEASON", "EPISODE"]);
    }

    #[test]
    fn one_included_type_is_the_kind_and_recursion_alone_widens_the_depth() {
        let films = listing("IncludeItemTypes=Movie&Recursive=true").unwrap();
        assert_eq!(films.filter_json(), json!({ "kind": "MOVIE" }));
        assert_eq!(films.kinds, ["MOVIE"]);
        let everything = listing("Recursive=true").unwrap();
        assert_eq!(everything.filter_json(), json!({ "depth": "ALL" }));
    }

    #[test]
    fn types_held_nowhere_here_leave_nothing_possible() {
        let boxsets = listing("IncludeItemTypes=BoxSet&Recursive=true").unwrap();
        assert!(boxsets.kinds.is_empty());
        let excluded = listing("ExcludeItemTypes=Movie,Episode").unwrap();
        assert_eq!(excluded.kinds, ["SHOW", "SEASON"]);
    }

    #[test]
    fn several_of_our_types_at_once_are_refused_but_by_a_search() {
        assert!(matches!(
            listing("IncludeItemTypes=Movie,Series"),
            Err(JellyfinError::NotImplemented(_))
        ));
        let search =
            listing("IncludeItemTypes=Movie,Series,Episode,Playlist&searchTerm=dune").unwrap();
        assert_eq!(search.kinds, ["MOVIE", "SHOW", "EPISODE"]);
        assert!(search.filter.get("kind").is_none());
        assert!(matches!(
            listing("IncludeItemTypes=Series,Episode&searchTerm=dune&parentId=0"),
            Err(JellyfinError::NotImplemented(_))
        ));
    }

    #[test]
    fn the_window_is_the_start_index_and_the_limit_under_the_cap() {
        let page = listing("StartIndex=100&Limit=50").unwrap();
        assert_eq!((page.offset, page.first), (100, 50));
        assert_eq!(listing("Limit=100000").unwrap().first, MAX_PAGE);
        assert!(matches!(
            listing("StartIndex=-3"),
            Err(JellyfinError::BadRequest(_))
        ));
    }

    #[test]
    fn sort_keys_map_onto_the_official_orders() {
        for (key, order) in [
            ("SortName", "TITLE"),
            ("SortName,ProductionYear", "TITLE"),
            ("ProductionYear,SortName", "RELEASED_ON"),
            ("PremiereDate", "RELEASED_ON"),
            ("DateCreated", "ADDED_AT"),
            ("CommunityRating", "AUDIENCE_RATING"),
            ("CriticRating", "RATING"),
            ("Runtime", "DURATION"),
            ("DatePlayed", "LAST_VIEWED_AT"),
            ("Random", "RANDOM"),
        ] {
            let parsed = listing(&format!("SortBy={key}&SortOrder=Descending")).unwrap();
            assert_eq!(parsed.order, Some(order), "{key}");
            assert_eq!(parsed.direction, Some("DESCENDING"), "{key}");
        }
        assert!(matches!(
            listing("SortBy=AlbumArtist"),
            Err(JellyfinError::NotImplemented(_))
        ));
        assert!(matches!(
            listing("SortBy=SortName&SortOrder=Sideways"),
            Err(JellyfinError::BadRequest(_))
        ));
    }

    #[test]
    fn watch_filters_read_from_filters_or_flags() {
        assert_eq!(
            listing("Filters=IsUnplayed").unwrap().filter_json(),
            json!({ "watched": false })
        );
        assert_eq!(
            listing("IsPlayed=true").unwrap().filter_json(),
            json!({ "watched": true })
        );
        assert_eq!(
            listing("Filters=IsResumable").unwrap().filter_json(),
            json!({ "inProgress": true })
        );
        for favorites in ["Filters=IsFavorite", "isFavorite=true"] {
            assert_eq!(
                listing(favorites).unwrap().filter_json(),
                json!({ "favorite": true }),
                "{favorites}"
            );
        }
    }

    #[test]
    fn a_minimum_premiere_date_is_a_release_lower_bound() {
        for written in [
            "2024-05-01T00:00:00.0000000Z",
            "2024-05-01T02:00:00%2B02:00",
            "2024-05-01T00:00:00",
            "2024-05-01",
        ] {
            let listing = listing(&format!("minPremiereDate={written}")).unwrap();
            assert_eq!(
                listing.filter_json(),
                json!({ "releasedFrom": "2024-05-01T00:00:00Z" }),
                "{written}"
            );
        }
        assert!(matches!(
            listing("minPremiereDate=yesterday"),
            Err(JellyfinError::BadRequest(_))
        ));
    }

    #[test]
    fn years_are_honored_as_a_range_only() {
        assert_eq!(
            listing("Years=2008").unwrap().filter_json(),
            json!({ "yearFrom": 2008, "yearTo": 2008 })
        );
        assert_eq!(
            listing("Years=2010,2008,2009").unwrap().filter_json(),
            json!({ "yearFrom": 2008, "yearTo": 2010 })
        );
        assert!(matches!(
            listing("Years=2001,2005"),
            Err(JellyfinError::NotImplemented(_))
        ));
        assert!(matches!(
            listing("Years=soon"),
            Err(JellyfinError::BadRequest(_))
        ));
    }

    #[test]
    fn a_genre_is_a_tag_filter_by_its_global_id() {
        let genre = crate::id::of("Tag", 12).unwrap();
        assert_eq!(
            listing(&format!("GenreIds={genre}")).unwrap().filter_json(),
            json!({ "tags": [{ "kind": "GENRE", "ids": [klapz_api::GlobalId::encode("Tag", 12)] }] })
        );
        assert!(matches!(
            listing("GenreIds=zzz"),
            Err(JellyfinError::BadRequest(_))
        ));
    }

    #[test]
    fn a_search_term_matches_title_words() {
        assert_eq!(
            listing("SearchTerm=dune").unwrap().filter_json(),
            json!({ "titleWordPrefix": "dune" })
        );
    }

    #[test]
    fn filters_the_official_api_cannot_express_are_refused() {
        for raw in [
            "StudioIds=x",
            "NameStartsWith=A",
            "Is4K=true",
            "MinCommunityRating=7",
            "Filters=Likes",
        ] {
            assert!(
                matches!(listing(raw), Err(JellyfinError::NotImplemented(_))),
                "{raw}"
            );
        }
    }

    #[test]
    fn presentation_parameters_are_tolerated() {
        assert!(listing(
            "Fields=Overview&ImageTypeLimit=1&EnableImageTypes=Primary&EnableUserData=true\
             &EnableTotalRecordCount=true&CollapseBoxSetItems=false&MediaTypes=Video"
        )
        .is_ok());
    }
}
