//! The `Meta` block of a listing, sent on `includeMeta=1`: what a client may filter and sort by.
//!
//! Plex Web builds its filter and sort menus from it. The reference lists 27 filters and 9
//! sorts for a film library; this lists the ones [`crate::listing`] actually honors, with the
//! reference's wording, order and keys. A menu entry that answered `501` when picked would be
//! worse than one that is not there. The advanced filters' `Field` and `FieldType` lists are
//! left out for the same reason: no listing understands those expressions yet.

use serde::Serialize;

use crate::language::Language;
use crate::listing::Sort;
use crate::translations::Catalog;

#[derive(Debug, Serialize)]
pub struct Meta {
    #[serde(rename = "Type")]
    types: Vec<MetaType>,
}

#[derive(Debug, Serialize)]
pub struct MetaType {
    key: String,
    #[serde(rename = "type")]
    kind: &'static str,
    title: &'static str,
    active: bool,
    #[serde(rename = "Filter", skip_serializing_if = "Vec::is_empty")]
    filters: Vec<MetaFilter>,
    #[serde(rename = "Sort", skip_serializing_if = "Vec::is_empty")]
    sorts: Vec<MetaSort>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaFilter {
    filter: &'static str,
    filter_type: &'static str,
    key: String,
    title: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaSort {
    #[serde(skip_serializing_if = "Option::is_none")]
    active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_direction: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<&'static str>,
    default_direction: &'static str,
    desc_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_character_key: Option<String>,
    key: &'static str,
    title: &'static str,
}

type Label = fn(&'static Catalog) -> &'static str;

/// `(filter, filterType, label)`, in the reference's order for each item type.
type FilterRow = (&'static str, &'static str, Label);

const MOVIE_FILTERS: &[FilterRow] = &[
    ("genre", "string", |c| c.genre),
    ("year", "integer", |c| c.year),
    ("decade", "integer", |c| c.decade),
    ("contentRating", "string", |c| c.content_rating),
    ("director", "string", |c| c.director),
    ("actor", "string", |c| c.actor),
    ("writer", "string", |c| c.writer),
    ("producer", "string", |c| c.producer),
    ("country", "string", |c| c.country),
    ("studio", "string", |c| c.studio),
    ("resolution", "string", |c| c.resolution),
    ("unwatched", "boolean", |c| c.unwatched),
    ("inProgress", "boolean", |c| c.in_progress),
];

const SHOW_FILTERS: &[FilterRow] = &[
    ("genre", "string", |c| c.genre),
    ("year", "integer", |c| c.year),
    ("contentRating", "string", |c| c.content_rating),
    ("studio", "string", |c| c.studio),
    ("country", "string", |c| c.country),
    ("director", "string", |c| c.director),
    ("actor", "string", |c| c.actor),
    ("writer", "string", |c| c.writer),
    ("producer", "string", |c| c.producer),
    ("unwatchedLeaves", "boolean", |c| c.unwatched),
];

const EPISODE_FILTERS: &[FilterRow] = &[
    ("year", "integer", |c| c.year),
    ("resolution", "string", |c| c.resolution),
    ("unwatched", "boolean", |c| c.unwatched),
    ("inProgress", "boolean", |c| c.in_progress),
];

/// `(key, defaultDirection, label)`, in the reference's order for each item type. The title
/// is not among them: it is written apart, with what only it carries.
type SortRow = (&'static str, &'static str, Label);

const RELEASE_DATE: SortRow = ("originallyAvailableAt", "desc", |c| c.release_date);
const CRITIC_RATING: SortRow = ("rating", "desc", |c| c.critic_rating);
const AUDIENCE_RATING: SortRow = ("audienceRating", "desc", |c| c.audience_rating);
const DATE_ADDED: SortRow = ("addedAt", "desc", |c| c.date_added);
const DATE_VIEWED: SortRow = ("lastViewedAt", "desc", |c| c.date_viewed);
const RANDOMLY: SortRow = ("random", "desc", |c| c.randomly);

const MOVIE_SORTS: &[SortRow] = &[
    RELEASE_DATE,
    CRITIC_RATING,
    AUDIENCE_RATING,
    ("duration", "desc", |c| c.duration),
    DATE_ADDED,
    DATE_VIEWED,
    ("mediaHeight", "asc", |c| c.resolution),
    RANDOMLY,
];

/// Without `unviewedLeafCount`, which the reference lists between audience rating and the
/// latest episode.
const SHOW_SORTS: &[SortRow] = &[
    RELEASE_DATE,
    CRITIC_RATING,
    AUDIENCE_RATING,
    ("episode.addedAt", "desc", |c| c.last_episode_date_added),
    DATE_VIEWED,
    RANDOMLY,
];

/// Seasons and episodes lead with an order by series, season and episode, which is not
/// offered: a listing cannot order by its items' parents yet.
const SEASON_SORTS: &[SortRow] = &[RANDOMLY];

const EPISODE_SORTS: &[SortRow] = &[
    RELEASE_DATE,
    CRITIC_RATING,
    AUDIENCE_RATING,
    DATE_ADDED,
    DATE_VIEWED,
    RANDOMLY,
];

/// One item type a library can list.
struct TypeRow {
    number: i32,
    kind: &'static str,
    title: Label,
    filters: &'static [FilterRow],
    by_title: bool,
    sorts: &'static [SortRow],
}

const MOVIE_TYPES: &[TypeRow] = &[TypeRow {
    number: 1,
    kind: "movie",
    title: |c| c.movies,
    filters: MOVIE_FILTERS,
    by_title: true,
    sorts: MOVIE_SORTS,
}];

const SHOW_TYPES: &[TypeRow] = &[
    TypeRow {
        number: 2,
        kind: "show",
        title: |c| c.tv_shows,
        filters: SHOW_FILTERS,
        by_title: true,
        sorts: SHOW_SORTS,
    },
    TypeRow {
        number: 3,
        kind: "season",
        title: |c| c.seasons,
        filters: &[],
        by_title: false,
        sorts: SEASON_SORTS,
    },
    TypeRow {
        number: 4,
        kind: "episode",
        title: |c| c.episodes,
        filters: EPISODE_FILTERS,
        by_title: false,
        sorts: EPISODE_SORTS,
    },
];

const TITLE_KEY: &str = "titleSort";

/// The sort in effect: the requested one, which the listing refused unless this type offers
/// it, else the title ascending.
fn active_sort(requested: Option<Sort>, row: &TypeRow) -> Option<(&'static str, &'static str)> {
    let direction = |sort: Sort| if sort.descending { "desc" } else { "asc" };
    match requested {
        Some(sort) if sort.key == TITLE_KEY && row.by_title => Some((TITLE_KEY, direction(sort))),
        Some(sort) if row.sorts.iter().any(|(key, _, _)| *key == sort.key) => {
            Some((sort.key, direction(sort)))
        }
        _ => row.by_title.then_some((TITLE_KEY, "asc")),
    }
}

fn sorts(
    section: i64,
    row: &TypeRow,
    catalog: &'static Catalog,
    current: Option<(&'static str, &'static str)>,
) -> Vec<MetaSort> {
    let marks = |key: &str| current.filter(|(active_key, _)| *active_key == key);

    let mut sorts = Vec::new();
    if row.by_title {
        let mark = marks(TITLE_KEY);
        sorts.push(MetaSort {
            active: mark.map(|_| true),
            active_direction: mark.map(|(_, direction)| direction),
            default: Some("asc"),
            default_direction: "asc",
            desc_key: format!("{TITLE_KEY}:desc"),
            first_character_key: Some(format!("/library/sections/{section}/firstCharacter")),
            key: TITLE_KEY,
            title: catalog.sort_title,
        });
    }
    for (key, default_direction, label) in row.sorts {
        let mark = marks(key);
        sorts.push(MetaSort {
            active: mark.map(|_| true),
            active_direction: mark.map(|(_, direction)| direction),
            default: None,
            default_direction,
            desc_key: format!("{key}:desc"),
            first_character_key: None,
            key,
            title: label(catalog),
        });
    }
    sorts
}

/// The key a filter's value list lives at. Only a series library's types qualify it, and
/// `country` not even there.
fn filter_key(section: i64, filter: &str, row: &TypeRow) -> String {
    if row.number == 1 || filter == "country" {
        format!("/library/sections/{section}/{filter}")
    } else {
        format!("/library/sections/{section}/{filter}?type={}", row.number)
    }
}

/// Builds the block for a library of `library_kind` (the official API's `MOVIE` or `SHOW`).
pub fn build(
    section: i64,
    library_kind: &str,
    requested_type: Option<i32>,
    sort: Option<Sort>,
    language: Language,
) -> Option<Meta> {
    let rows = match library_kind {
        "MOVIE" => MOVIE_TYPES,
        "SHOW" => SHOW_TYPES,
        _ => return None,
    };
    let catalog = language.catalog();
    // An unknown or absent `type` leaves the library's first type active.
    let active_number = requested_type
        .filter(|number| rows.iter().any(|row| row.number == *number))
        .unwrap_or(rows[0].number);

    let types = rows
        .iter()
        .map(|row| {
            let active = row.number == active_number;
            MetaType {
                key: format!("/library/sections/{section}/all?type={}", row.number),
                kind: row.kind,
                title: (row.title)(catalog),
                active,
                filters: row
                    .filters
                    .iter()
                    .map(|(filter, filter_type, label)| MetaFilter {
                        filter,
                        filter_type,
                        key: filter_key(section, filter, row),
                        title: label(catalog),
                        kind: "filter",
                    })
                    .collect(),
                sorts: sorts(
                    section,
                    row,
                    catalog,
                    active.then(|| active_sort(sort, row)).flatten(),
                ),
            }
        })
        .collect();
    Some(Meta { types })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::client::ClientIdentity;

    fn film_meta(requested_type: Option<i32>, sort: Option<&str>) -> serde_json::Value {
        let sort = sort.map(|sort| Sort::parse(sort).unwrap());
        let meta = build(4, "MOVIE", requested_type, sort, Language::default()).unwrap();
        serde_json::to_value(meta).unwrap()
    }

    fn active_sorts(meta: &serde_json::Value) -> Vec<(String, String)> {
        meta["Type"][0]["Sort"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|sort| sort.get("active").is_some())
            .map(|sort| {
                (
                    sort["key"].as_str().unwrap().to_owned(),
                    sort["activeDirection"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    fn sort_pair(key: &str, direction: &str) -> Vec<(String, String)> {
        vec![(key.to_owned(), direction.to_owned())]
    }

    #[test]
    fn the_title_is_active_by_default() {
        assert_eq!(
            active_sorts(&film_meta(None, None)),
            sort_pair("titleSort", "asc")
        );
    }

    #[test]
    fn marks_the_requested_sort_and_its_direction() {
        assert_eq!(
            active_sorts(&film_meta(None, Some("addedAt:desc"))),
            sort_pair("addedAt", "desc")
        );
        assert_eq!(
            active_sorts(&film_meta(None, Some("addedAt"))),
            sort_pair("addedAt", "asc")
        );
        assert_eq!(
            active_sorts(&film_meta(None, Some("titleSort:desc"))),
            sort_pair("titleSort", "desc")
        );
    }

    #[test]
    fn a_sort_another_type_offers_leaves_the_title_marked() {
        // `episode.addedAt` is a series' sort; a film listing marks what it falls back to.
        assert_eq!(
            active_sorts(&film_meta(None, Some("episode.addedAt:desc"))),
            sort_pair("titleSort", "asc")
        );
        assert_eq!(
            active_sorts(&film_meta(None, Some("rating:desc"))),
            sort_pair("rating", "desc")
        );
    }

    fn sort_keys(meta: &serde_json::Value, row: usize) -> Vec<String> {
        meta["Type"][row]["Sort"]
            .as_array()
            .unwrap()
            .iter()
            .map(|sort| sort["key"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn offers_the_reference_sorts_in_its_order() {
        // Captured from the reference: a film library offers all nine; a series library
        // lacks `unviewedLeafCount`, and its episodes their order by series.
        assert_eq!(
            sort_keys(&film_meta(None, None), 0),
            [
                "titleSort",
                "originallyAvailableAt",
                "rating",
                "audienceRating",
                "duration",
                "addedAt",
                "lastViewedAt",
                "mediaHeight",
                "random"
            ]
        );
        let shows =
            serde_json::to_value(build(5, "SHOW", None, None, Language::default())).unwrap();
        assert_eq!(
            sort_keys(&shows, 0),
            [
                "titleSort",
                "originallyAvailableAt",
                "rating",
                "audienceRating",
                "episode.addedAt",
                "lastViewedAt",
                "random"
            ]
        );
        assert_eq!(sort_keys(&shows, 1), ["random"]);
        assert_eq!(
            sort_keys(&shows, 2),
            [
                "originallyAvailableAt",
                "rating",
                "audienceRating",
                "addedAt",
                "lastViewedAt",
                "random"
            ]
        );
    }

    /// Like the filters: a sort offered must be one a listing accepts.
    #[test]
    fn every_sort_offered_is_one_a_listing_honors() {
        for rows in [MOVIE_TYPES, SHOW_TYPES] {
            for row in rows {
                for (key, _, _) in row.sorts {
                    assert!(Sort::parse(key).is_ok(), "{key}");
                }
            }
        }
    }

    #[test]
    fn writes_a_film_sort_as_the_reference_does() {
        let meta = film_meta(None, None);
        assert_eq!(
            meta["Type"][0]["Sort"][0],
            serde_json::json!({
                "active": true,
                "activeDirection": "asc",
                "default": "asc",
                "defaultDirection": "asc",
                "descKey": "titleSort:desc",
                "firstCharacterKey": "/library/sections/4/firstCharacter",
                "key": "titleSort",
                "title": "Title",
            })
        );
        assert_eq!(
            meta["Type"][0]["Sort"][5],
            serde_json::json!({
                "defaultDirection": "desc",
                "descKey": "addedAt:desc",
                "key": "addedAt",
                "title": "Date Added",
            })
        );
    }

    #[test]
    fn a_series_library_qualifies_its_filter_keys_except_country() {
        let meta = build(5, "SHOW", None, None, Language::default()).unwrap();
        let meta = serde_json::to_value(meta).unwrap();
        let filters = meta["Type"][0]["Filter"].as_array().unwrap();
        let key_of = |name: &str| {
            filters
                .iter()
                .find(|filter| filter["filter"] == name)
                .map(|filter| filter["key"].as_str().unwrap().to_owned())
        };
        assert_eq!(key_of("genre").unwrap(), "/library/sections/5/genre?type=2");
        assert_eq!(key_of("country").unwrap(), "/library/sections/5/country");
    }

    #[test]
    fn the_requested_type_is_the_active_one() {
        let meta = build(5, "SHOW", Some(4), None, Language::default()).unwrap();
        let meta = serde_json::to_value(meta).unwrap();
        let active: Vec<_> = meta["Type"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["type"].as_str().unwrap(),
                    row["active"].as_bool().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            active,
            [("show", false), ("season", false), ("episode", true)]
        );
    }

    #[test]
    fn labels_follow_the_clients_language() {
        let client = ClientIdentity {
            language: Some("fr".to_owned()),
            ..ClientIdentity::default()
        };
        let meta = build(4, "MOVIE", None, None, Language::of(&client)).unwrap();
        let meta = serde_json::to_value(meta).unwrap();
        assert_eq!(meta["Type"][0]["title"], "Films");
        assert_eq!(meta["Type"][0]["Filter"][1]["title"], "Année");
    }

    /// The reason this list is shorter than the reference's: every filter offered must be
    /// one a listing accepts, or the menu would lead to a `501`.
    #[test]
    fn every_filter_offered_is_one_a_listing_honors() {
        for rows in [MOVIE_TYPES, SHOW_TYPES] {
            for row in rows {
                for (filter, _, _) in row.filters {
                    let value = if *filter == "resolution" {
                        "1080"
                    } else {
                        "2020"
                    };
                    let pairs = [((*filter).to_owned(), value.to_owned())];
                    assert!(crate::listing::parse(&pairs).is_ok(), "{filter}");
                }
            }
        }
    }
}
