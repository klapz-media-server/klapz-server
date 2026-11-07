//! Plex's listing-query grammar, translated into the official API's inputs.
//!
//! `/library/sections/{id}/all` takes its filters as query parameters whose *name* carries the
//! operator: `year=2022`, `year>=2020`, `year!=2022`. The field list is not guesswork — the
//! server publishes it per library under `Meta/Type/Filter` (via `includeMeta=1`), and
//! [`KNOWN_FILTERS`] is that list for a film library, captured from the reference.
//!
//! The important decision here is what to do with a filter we cannot honor. Plex ignores
//! parameters *it* does not recognize, and we do the same — a client appending
//! `includeCollections=1` or `X-Plex-Product` must not get an error. But silently ignoring a
//! filter Plex *does* support is a different thing: a client searching for `title=avatar` would
//! receive all 9,667 films and present them as the results. So a field on the published list
//! that we cannot translate is refused, loudly, rather than dropped.

use std::fmt::Write as _;

use async_graphql::{Name, Value};
use open_plex_api::tag::TAG_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::metadata::TagCondition;

/// Filters a film library advertises, captured from the reference server's own
/// `Meta/Type/Filter` declaration.
///
/// Membership in this list is what separates "a filter we have not implemented" from "a
/// parameter that is none of our business". Every entry we cannot translate is refused.
const KNOWN_FILTERS: &[&str] = &[
    "actor",
    "atmos",
    "audioCodec",
    "audioLanguage",
    "audioLayout",
    "collection",
    "contentRating",
    "country",
    "decade",
    "director",
    "dovi",
    "duplicate",
    "genre",
    "hdr",
    "hdr10plus",
    "inProgress",
    "label",
    "location",
    "producer",
    "resolution",
    "studio",
    "subtitleCodec",
    "subtitleLanguage",
    "title",
    "unmatched",
    "unwatched",
    "videoCodec",
    "writer",
    "year",
];

/// Parameters that steer the response without filtering it, and are handled elsewhere.
const CONTROL_PARAMS: &[&str] = &[
    "X-Plex-Container-Size",
    "X-Plex-Container-Start",
    "X-Plex-Token",
    "sort",
    "type",
];

#[derive(Debug, thiserror::Error)]
pub enum ListingError {
    /// A filter the server advertises that we cannot translate yet. Named so the operator's
    /// log says which one, and so the message is actionable rather than "not implemented".
    #[error("the `{0}` filter is not implemented")]
    UnsupportedFilter(String),
    #[error("the `{operator}` operator on `{field}` is not implemented")]
    UnsupportedOperator { field: String, operator: String },
    /// More alternatives in one tag filter than the server accepts. The reference has no
    /// such ceiling; we do, because every value becomes a bound parameter.
    #[error("the `{field}` filter lists {count} values, past the maximum of {max}", max = TagCondition::MAX_VALUES)]
    TooManyValues { field: String, count: usize },
}

/// The filters that select by tag, with the schema's name for each kind.
///
/// `actor` is the one whose name differs from its element: the reference emits `<Role>` and
/// filters with `actor=`.
const fn tag_type(field: &str) -> Option<&'static str> {
    Some(match field.as_bytes() {
        b"genre" => "GENRE",
        b"country" => "COUNTRY",
        b"director" => "DIRECTOR",
        b"writer" => "WRITER",
        b"actor" => "ACTOR",
        b"producer" => "PRODUCER",
        _ => return None,
    })
}

/// How a filter compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    Equals,
    AtLeast,
    AtMost,
}

/// Splits `year>=` into the field and its operator.
///
/// The operator rides on the parameter *name*, so the name is what has to be taken apart. A
/// suffix we do not know is reported rather than treated as part of the field, which would
/// turn an unsupported comparison into an unknown field and lose the distinction.
fn split_operator(key: &str) -> (&str, Option<Operator>) {
    for (suffix, operator) in [
        (">=", Operator::AtLeast),
        ("<=", Operator::AtMost),
        (">", Operator::AtLeast),
        ("<", Operator::AtMost),
    ] {
        if let Some(field) = key.strip_suffix(suffix) {
            return (field, Some(operator));
        }
    }
    // Anything still ending in punctuation is an operator we do not implement — `!=` for
    // negation is the common one.
    if key.ends_with(|last: char| !last.is_alphanumeric()) {
        return (
            key.trim_end_matches(|last: char| !last.is_alphanumeric()),
            None,
        );
    }
    (key, Some(Operator::Equals))
}

/// The known filter a malformed key opens with, if any.
///
/// A key is malformed when it begins with a filter field followed by punctuation but did not
/// split into a field and an operator — the shape `year>=2020` takes when a client encodes
/// the `=` and the whole expression arrives as one parameter name.
fn malformed_filter(key: &str) -> Option<&'static str> {
    KNOWN_FILTERS.iter().copied().find(|field| {
        key.strip_prefix(field)
            .is_some_and(|rest| rest.starts_with(|next: char| !next.is_alphanumeric()))
    })
}

/// What the adapter reads off a listing request.
#[derive(Debug, Default)]
pub struct ListingRequest {
    pub start: u64,
    pub size: Option<u32>,
    /// Numeric item type: 1 movie, 2 show, 3 season, 4 episode.
    pub kind: Option<i32>,
    /// Sort field, optionally suffixed `:asc` or `:desc`.
    pub sort: Option<String>,
    pub filters: Filters,
}

/// The subset of Plex's filters that maps onto `ItemFilterInput`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Filters {
    pub title_word_prefix: Option<String>,
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
    pub content_rating: Option<String>,
    pub studio_contains: Option<String>,
    pub min_width: Option<i32>,
    /// One entry per tag parameter; several combine with `AND`.
    pub tags: Vec<TagFilter>,
}

/// One tag parameter: `genre=38,Crime` is one filter with an id and a name.
#[derive(Debug, PartialEq, Eq)]
pub struct TagFilter {
    /// The schema's enum value for the kind.
    pub kind: &'static str,
    pub ids: Vec<i64>,
    pub names: Vec<String>,
}

impl TagFilter {
    /// Reads a comma list, where an integer is an id and anything else a name.
    ///
    /// The rule the reference follows, measured: `genre=038` is genre 38, while `genre=38.0`,
    /// `genre=%2038` and `genre=Act` match nothing, being names no genre has. A name is
    /// passed through untouched; the matching rule lives in storage.
    fn parse(kind: &'static str, value: &str) -> Self {
        let mut filter = Self {
            kind,
            ids: Vec::new(),
            names: Vec::new(),
        };
        for part in value.split(',') {
            match part.parse::<i64>() {
                Ok(id) => filter.ids.push(id),
                Err(_) => filter.names.push(part.to_owned()),
            }
        }
        filter
    }

    fn len(&self) -> usize {
        self.ids.len() + self.names.len()
    }

    fn to_graphql(&self) -> Value {
        let mut fields = async_graphql::indexmap::IndexMap::new();
        fields.insert(Name::new("kind"), Value::Enum(Name::new(self.kind)));
        if !self.ids.is_empty() {
            let ids = self
                .ids
                .iter()
                .map(|id| Value::String(GlobalId::encode(TAG_TYPE, *id)))
                .collect();
            fields.insert(Name::new("ids"), Value::List(ids));
        }
        if !self.names.is_empty() {
            let names = self.names.iter().cloned().map(Value::String).collect();
            fields.insert(Name::new("names"), Value::List(names));
        }
        Value::Object(fields)
    }
}

impl Filters {
    /// Renders as the `ItemFilterInput` the schema takes, or null when nothing is set.
    ///
    /// `kind` is added by the caller: it comes from `type=`, which is a control parameter
    /// rather than a filter on Plex's own list.
    pub fn to_graphql(&self, kind: Option<&str>) -> Value {
        let mut fields = async_graphql::indexmap::IndexMap::new();
        let mut set = |name: &str, value: Value| {
            fields.insert(Name::new(name), value);
        };

        if let Some(kind) = kind {
            set("kind", Value::Enum(Name::new(kind)));
        }
        if let Some(prefix) = &self.title_word_prefix {
            set("titleWordPrefix", Value::String(prefix.clone()));
        }
        if let Some(year) = self.year_from {
            set("yearFrom", Value::Number(year.into()));
        }
        if let Some(year) = self.year_to {
            set("yearTo", Value::Number(year.into()));
        }
        if let Some(rating) = &self.content_rating {
            set("contentRating", Value::String(rating.clone()));
        }
        if let Some(studio) = &self.studio_contains {
            set("studioContains", Value::String(studio.clone()));
        }
        if let Some(width) = self.min_width {
            set("minWidth", Value::Number(width.into()));
        }
        if !self.tags.is_empty() {
            set(
                "tags",
                Value::List(self.tags.iter().map(TagFilter::to_graphql).collect()),
            );
        }

        if fields.is_empty() {
            Value::Null
        } else {
            Value::Object(fields)
        }
    }
}

/// Minimum frame width for each resolution label Plex filters by.
///
/// The inverse of the label the listing emits, with the same thresholds, so a client that
/// reads `videoResolution: "4k"` off an item and filters by it finds that item again.
fn resolution_width(label: &str) -> Option<i32> {
    match label.to_ascii_lowercase().as_str() {
        "4k" => Some(3000),
        "1080" => Some(1700),
        "720" => Some(1200),
        "576" | "480" | "sd" => Some(0),
        _ => None,
    }
}

/// Reads a listing request out of the raw query pairs.
///
/// Pairs rather than a map: Plex allows the same field twice and combines the two with `AND`,
/// so collapsing duplicates would silently widen the result.
///
/// # Errors
///
/// Fails when a parameter names a filter the server advertises and we cannot translate.
pub fn parse(pairs: &[(String, String)]) -> Result<ListingRequest, ListingError> {
    let mut request = ListingRequest::default();
    let mut seen_filters = Vec::new();

    for (key, value) in pairs {
        match key.as_str() {
            "X-Plex-Container-Start" => request.start = value.parse().unwrap_or(0),
            "X-Plex-Container-Size" => request.size = value.parse().ok(),
            "type" => request.kind = value.parse().ok(),
            "sort" => request.sort = Some(value.clone()),
            _ => {}
        }
        if CONTROL_PARAMS.contains(&key.as_str()) {
            continue;
        }

        let (field, operator) = split_operator(key);
        if !KNOWN_FILTERS.contains(&field) {
            // A key that *opens* with a known field but is not one — `year>=2020` arriving
            // whole because the client percent-encoded the `=` — must not fall through to
            // "none of our business". That is precisely the silent widening this module
            // exists to prevent.
            if let Some(known) = malformed_filter(key) {
                return Err(ListingError::UnsupportedOperator {
                    field: known.to_owned(),
                    operator: key[known.len()..].to_owned(),
                });
            }
            // Otherwise none of our business, exactly as a real server treats it.
            continue;
        }

        let Some(operator) = operator else {
            return Err(ListingError::UnsupportedOperator {
                field: field.to_owned(),
                operator: key[field.len()..].to_owned(),
            });
        };

        if let (Some(kind), Operator::Equals) = (tag_type(field), operator) {
            // The one family where both combinations are expressible: a comma is an `OR`
            // inside one tag condition, a repeated parameter another condition, `AND`ed.
            let filter = TagFilter::parse(kind, value);
            if filter.len() > TagCondition::MAX_VALUES {
                return Err(ListingError::TooManyValues {
                    field: field.to_owned(),
                    count: filter.len(),
                });
            }
            request.filters.tags.push(filter);
            continue;
        }

        // A comma means OR, which `ItemFilterInput` cannot express: each of its fields is one
        // value combined with AND.
        if value.contains(',') {
            return Err(ListingError::UnsupportedFilter(field.to_owned()));
        }
        // The same field twice is an AND of two constraints. Only a range survives that, and
        // only for the two fields that have one.
        if seen_filters.contains(&field) && !matches!(field, "year" | "decade") {
            return Err(ListingError::UnsupportedFilter(field.to_owned()));
        }
        seen_filters.push(field);

        apply(&mut request.filters, field, operator, value)?;
    }

    Ok(request)
}

/// Translates one filter, or refuses it.
fn apply(
    filters: &mut Filters,
    field: &str,
    operator: Operator,
    value: &str,
) -> Result<(), ListingError> {
    let unsupported = || ListingError::UnsupportedFilter(field.to_owned());

    match (field, operator) {
        // A prefix match, which is what the reference does: `title=atar` finds nothing while
        // `title=avatar` finds the four Avatar films.
        ("title", Operator::Equals) => filters.title_word_prefix = Some(value.to_owned()),
        ("year", Operator::Equals) => {
            let year = value.parse().map_err(|_| unsupported())?;
            filters.year_from = Some(year);
            filters.year_to = Some(year);
        }
        ("year", Operator::AtLeast) => {
            filters.year_from = Some(value.parse().map_err(|_| unsupported())?);
        }
        ("year", Operator::AtMost) => {
            filters.year_to = Some(value.parse().map_err(|_| unsupported())?);
        }
        // A decade is the ten years it opens: `decade=2020` is 2020 through 2029.
        ("decade", Operator::Equals) => {
            let start: i32 = value.parse().map_err(|_| unsupported())?;
            filters.year_from = Some(start);
            filters.year_to = Some(start + 9);
        }
        ("contentRating", Operator::Equals) => filters.content_rating = Some(value.to_owned()),
        // A substring, not a name: `studio=Films` matches 992 of 9,667 films on the
        // reference, which is every studio containing the word.
        ("studio", Operator::Equals) => filters.studio_contains = Some(value.to_owned()),
        ("resolution", Operator::Equals) => {
            filters.min_width = Some(resolution_width(value).ok_or_else(unsupported)?);
        }
        _ => return Err(unsupported()),
    }
    Ok(())
}

/// Lists the filters this server can honor, for an operator reading a refusal in the log.
pub fn supported_filters() -> String {
    let mut out = String::new();
    for field in [
        "title",
        "year",
        "decade",
        "contentRating",
        "studio",
        "resolution",
        "genre",
        "country",
        "director",
        "writer",
        "actor",
        "producer",
    ] {
        if !out.is_empty() {
            out.push_str(", ");
        }
        let _ = write!(out, "{field}");
    }
    out
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn pairs(query: &[(&str, &str)]) -> Vec<(String, String)> {
        query
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn reads_the_container_window_and_the_view_controls() {
        let request = parse(&pairs(&[
            ("X-Plex-Container-Start", "50"),
            ("X-Plex-Container-Size", "25"),
            ("type", "2"),
            ("sort", "addedAt:desc"),
        ]))
        .expect("parses");

        assert_eq!(request.start, 50);
        assert_eq!(request.size, Some(25));
        assert_eq!(request.kind, Some(2));
        assert_eq!(request.sort.as_deref(), Some("addedAt:desc"));
        assert_eq!(request.filters, Filters::default());
    }

    #[test]
    fn a_title_filter_is_a_prefix() {
        // Measured: `title=avatar` returns 4 of 9,667 films, `title=atar` returns none.
        let request = parse(&pairs(&[("title", "avatar")])).expect("parses");
        assert_eq!(request.filters.title_word_prefix.as_deref(), Some("avatar"));
    }

    #[test]
    fn year_operators_become_a_range() {
        let exact = parse(&pairs(&[("year", "2022")])).expect("parses").filters;
        assert_eq!((exact.year_from, exact.year_to), (Some(2022), Some(2022)));

        let from = parse(&pairs(&[("year>=", "2020")]))
            .expect("parses")
            .filters;
        assert_eq!((from.year_from, from.year_to), (Some(2020), None));

        let to = parse(&pairs(&[("year<=", "1980")]))
            .expect("parses")
            .filters;
        assert_eq!((to.year_from, to.year_to), (None, Some(1980)));

        // Both bounds at once: the one case where repeating a field is expressible.
        let between = parse(&pairs(&[("year>=", "2000"), ("year<=", "2010")]))
            .expect("parses")
            .filters;
        assert_eq!(
            (between.year_from, between.year_to),
            (Some(2000), Some(2010))
        );
    }

    #[test]
    fn a_decade_is_the_ten_years_it_opens() {
        let filters = parse(&pairs(&[("decade", "2020")]))
            .expect("parses")
            .filters;
        assert_eq!(
            (filters.year_from, filters.year_to),
            (Some(2020), Some(2029))
        );
    }

    #[test]
    fn resolution_maps_to_the_width_the_listing_reports() {
        // A client reading `videoResolution: "4k"` off an item must find it again by
        // filtering on that value, so the thresholds have to be the same ones.
        let filters = parse(&pairs(&[("resolution", "4k")]))
            .expect("parses")
            .filters;
        assert_eq!(filters.min_width, Some(3000));
        assert!(parse(&pairs(&[("resolution", "8k")])).is_err());
    }

    #[test]
    fn parameters_that_are_not_filters_are_ignored() {
        // A real server tolerates these, including nonsense. Refusing them would break
        // ordinary browsing.
        let request = parse(&pairs(&[
            ("includeMeta", "1"),
            ("includeCollections", "1"),
            ("excludeAllLeaves", "1"),
            ("X-Plex-Product", "Plex Web"),
            ("nonsenseParam", "42"),
        ]))
        .expect("parses");
        assert_eq!(request.filters, Filters::default());
    }

    #[test]
    fn an_advertised_filter_we_cannot_honor_is_refused_not_dropped() {
        // The whole point: dropping `genre` would answer with the entire library and let the
        // client present it as the filtered result.
        for field in ["collection", "label", "unwatched", "hdr"] {
            let error = parse(&pairs(&[(field, "1")])).expect_err("refused");
            assert!(
                matches!(&error, ListingError::UnsupportedFilter(name) if name == field),
                "{field}: {error}"
            );
        }
    }

    #[test]
    fn an_operator_we_cannot_honor_is_refused_too() {
        // `year!=2022` is a negation the filter input cannot express.
        let error = parse(&pairs(&[("year!=", "2022")])).expect_err("refused");
        assert!(
            matches!(
                &error,
                ListingError::UnsupportedOperator { field, operator } if field == "year" && operator == "!="
            ),
            "{error}"
        );
    }

    #[test]
    fn a_comma_list_is_refused_because_it_means_or() {
        // Measured: `year=2020,2021` returns 1,225 on the reference, exactly 632 + 593.
        let error = parse(&pairs(&[("year", "2020,2021")])).expect_err("refused");
        assert!(
            matches!(&error, ListingError::UnsupportedFilter(name) if name == "year"),
            "{error}"
        );
    }

    #[test]
    fn repeating_a_non_range_field_is_refused() {
        // Two studios combine with AND on the reference, which one field cannot express.
        assert!(parse(&pairs(&[("studio", "HBO"), ("studio", "Netflix")])).is_err());
    }

    #[test]
    fn an_encoded_operator_is_refused_rather_than_ignored() {
        // `year%3E%3D2020` arrives as the single key `year>=2020` with an empty value. The
        // tempting reading is "not a known field, ignore it" — which answers with the whole
        // library. Measured on the reference, the encoded form is rejected there too.
        let error = parse(&pairs(&[("year>=2020", "")])).expect_err("refused");
        assert!(
            matches!(
                &error,
                ListingError::UnsupportedOperator { field, operator }
                    if field == "year" && operator == ">=2020"
            ),
            "{error}"
        );

        // A parameter that merely starts with the same letters is still none of our business.
        assert!(parse(&pairs(&[("yearbook", "1")])).is_ok());
        assert!(parse(&pairs(&[("titleish", "x")])).is_ok());
    }

    #[test]
    fn a_tag_value_is_an_id_when_it_is_an_integer_and_a_name_otherwise() {
        // Measured: `genre=038` is genre 38; `genre=38.0` and `genre=%2038` match nothing.
        let filters = parse(&pairs(&[("genre", "038,Crime,38.0, 38")]))
            .expect("parses")
            .filters;
        assert_eq!(
            filters.tags,
            vec![TagFilter {
                kind: "GENRE",
                ids: vec![38],
                names: vec!["Crime".to_owned(), "38.0".to_owned(), " 38".to_owned()],
            }]
        );
    }

    #[test]
    fn a_comma_is_or_and_a_repeat_is_and_for_tags() {
        // Measured: `genre=38,198` returns 1,629 films on the reference and
        // `genre=38&genre=198` 404.
        let filters = parse(&pairs(&[("genre", "38,198"), ("genre", "74")]))
            .expect("parses")
            .filters;
        assert_eq!(filters.tags.len(), 2);
        assert_eq!(filters.tags[0].ids, vec![38, 198]);
        assert_eq!(filters.tags[1].ids, vec![74]);
    }

    #[test]
    fn each_tag_filter_names_its_kind() {
        for (field, kind) in [
            ("genre", "GENRE"),
            ("country", "COUNTRY"),
            ("director", "DIRECTOR"),
            ("writer", "WRITER"),
            ("actor", "ACTOR"),
            ("producer", "PRODUCER"),
        ] {
            let filters = parse(&pairs(&[(field, "1")])).expect("parses").filters;
            assert_eq!(filters.tags[0].kind, kind, "{field}");
        }
    }

    #[test]
    fn a_tag_operator_we_cannot_honor_is_refused() {
        // `genre!=38` is a negation the filter input cannot express.
        assert!(parse(&pairs(&[("genre!=", "38")])).is_err());
    }

    #[test]
    fn too_many_tag_values_are_refused() {
        let value = vec!["1"; TagCondition::MAX_VALUES + 1].join(",");
        let error = parse(&pairs(&[("actor", &value)])).expect_err("refused");
        assert!(
            matches!(error, ListingError::TooManyValues { .. }),
            "{error}"
        );
    }

    #[test]
    fn a_tag_filter_renders_ids_as_global_ids() {
        let filters = parse(&pairs(&[("genre", "38,Crime")]))
            .expect("parses")
            .filters;
        let Value::Object(fields) = filters.to_graphql(None) else {
            panic!("an object");
        };
        let Some(Value::List(tags)) = fields.get("tags") else {
            panic!("a tag list");
        };
        let Value::Object(tag) = &tags[0] else {
            panic!("a tag condition");
        };
        assert_eq!(tag.get("kind"), Some(&Value::Enum(Name::new("GENRE"))));
        assert_eq!(
            tag.get("ids"),
            Some(&Value::List(vec![Value::String(GlobalId::encode(
                TAG_TYPE, 38
            ))]))
        );
        assert_eq!(
            tag.get("names"),
            Some(&Value::List(vec![Value::String("Crime".to_owned())]))
        );
    }

    #[test]
    fn filters_render_as_the_schema_input() {
        let filters = parse(&pairs(&[("title", "av"), ("year>=", "2000")]))
            .expect("parses")
            .filters;
        let Value::Object(fields) = filters.to_graphql(Some("MOVIE")) else {
            panic!("an object");
        };
        assert_eq!(fields.get("kind"), Some(&Value::Enum(Name::new("MOVIE"))));
        assert_eq!(
            fields.get("titleWordPrefix"),
            Some(&Value::String("av".to_owned()))
        );
        assert_eq!(fields.get("yearFrom"), Some(&Value::Number(2000.into())));
        assert!(fields.get("yearTo").is_none());

        // Nothing set and no kind: null rather than an empty object, so the schema applies
        // its own default.
        assert_eq!(Filters::default().to_graphql(None), Value::Null);
    }
}
