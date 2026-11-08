//! The `Meta` block of a listing, sent on `includeMeta=1`: what a client may filter and sort by.
//!
//! Plex Web builds its filter and sort menus from it. The reference lists 27 filters and 9
//! sorts for a film library; this lists the ones [`crate::listing`] actually honors, with the
//! reference's wording, order and keys. A menu entry that answered `501` when picked would be
//! worse than one that is not there. The advanced filters' `Field` and `FieldType` lists are
//! left out for the same reason: no listing understands those expressions yet.

use serde::Serialize;

use crate::language::Language;
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
];

const EPISODE_FILTERS: &[FilterRow] = &[
    ("year", "integer", |c| c.year),
    ("resolution", "string", |c| c.resolution),
];

/// One item type a library can list.
struct TypeRow {
    number: i32,
    kind: &'static str,
    title: Label,
    filters: &'static [FilterRow],
    by_title: bool,
    by_date_added: bool,
}

const MOVIE_TYPES: &[TypeRow] = &[TypeRow {
    number: 1,
    kind: "movie",
    title: |c| c.movies,
    filters: MOVIE_FILTERS,
    by_title: true,
    by_date_added: true,
}];

const SHOW_TYPES: &[TypeRow] = &[
    TypeRow {
        number: 2,
        kind: "show",
        title: |c| c.tv_shows,
        filters: SHOW_FILTERS,
        by_title: true,
        by_date_added: false,
    },
    TypeRow {
        number: 3,
        kind: "season",
        title: |c| c.seasons,
        filters: &[],
        by_title: false,
        by_date_added: false,
    },
    TypeRow {
        number: 4,
        kind: "episode",
        title: |c| c.episodes,
        filters: EPISODE_FILTERS,
        by_title: false,
        by_date_added: true,
    },
];

const TITLE_KEY: &str = "titleSort";
const DATE_ADDED_KEY: &str = "addedAt";

/// The sort in effect: the requested one when this type offers it, else what a listing falls
/// back to, the title.
fn active_sort(requested: Option<&str>, row: &TypeRow) -> (&'static str, &'static str) {
    let (key, suffix) = requested.map_or((TITLE_KEY, "asc"), |sort| {
        sort.split_once(':').unwrap_or((sort, "asc"))
    });
    let direction = if suffix.eq_ignore_ascii_case("desc") {
        "desc"
    } else {
        "asc"
    };
    match key {
        DATE_ADDED_KEY if row.by_date_added => (DATE_ADDED_KEY, direction),
        TITLE_KEY if row.by_title => (TITLE_KEY, direction),
        _ => (TITLE_KEY, "asc"),
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
    if row.by_date_added {
        let mark = marks(DATE_ADDED_KEY);
        sorts.push(MetaSort {
            active: mark.map(|_| true),
            active_direction: mark.map(|(_, direction)| direction),
            default: None,
            default_direction: "desc",
            desc_key: format!("{DATE_ADDED_KEY}:desc"),
            first_character_key: None,
            key: DATE_ADDED_KEY,
            title: catalog.date_added,
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
    sort: Option<&str>,
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
                    active.then(|| active_sort(sort, row)),
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
    fn a_sort_we_do_not_honor_shows_the_title_the_listing_falls_back_to() {
        assert_eq!(
            active_sorts(&film_meta(None, Some("rating:desc"))),
            sort_pair("titleSort", "asc")
        );
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
            meta["Type"][0]["Sort"][1],
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
