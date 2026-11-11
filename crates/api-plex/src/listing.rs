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
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::tag::TAG_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::metadata::TagCondition;

/// Filters the reference advertises, captured from its `Meta/Type/Filter` declarations for
/// films, series and episodes, and the fields its shelves filter on without advertising them
/// (`audienceRating`, `lastViewedAt`, `originallyAvailableAt`, `viewCount`, `viewOffset`,
/// `viewedOrInProgressLeafCount`).
///
/// Membership in this list is what separates "a filter we have not implemented" from "a
/// parameter that is none of our business". Every entry we cannot translate is refused.
const KNOWN_FILTERS: &[&str] = &[
    "actor",
    "atmos",
    "audioCodec",
    "audioLanguage",
    "audioLayout",
    "audienceRating",
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
    "lastViewedAt",
    "location",
    "network",
    "originallyAvailableAt",
    "producer",
    "resolution",
    "studio",
    "subtitleCodec",
    "subtitleLanguage",
    "title",
    "unmatched",
    "unwatched",
    "unwatchedLeaves",
    "videoCodec",
    "viewCount",
    "viewOffset",
    "viewedOrInProgressLeafCount",
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
    /// A sort we cannot honor. Refused like a filter, rather than answered in title order
    /// a client would present as the order it asked for.
    #[error("sorting by `{0}` is not implemented")]
    UnsupportedSort(String),
}

/// Plex's sort keys a listing honors, with the official API's order for each.
pub const SORTS: &[(&str, &str)] = &[
    ("titleSort", "TITLE"),
    ("addedAt", "ADDED_AT"),
    ("originallyAvailableAt", "RELEASED_ON"),
    ("rating", "RATING"),
    ("audienceRating", "AUDIENCE_RATING"),
    ("duration", "DURATION"),
    ("mediaHeight", "HEIGHT"),
    ("lastViewedAt", "LAST_VIEWED_AT"),
    ("episode.addedAt", "EPISODE_ADDED_AT"),
    ("random", "RANDOM"),
];

/// A `sort=` a listing honors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    /// Plex's key, as the client sent it.
    pub key: &'static str,
    /// The official API's `ItemOrder`.
    pub order: &'static str,
    pub descending: bool,
}

impl Sort {
    /// Reads `field` or `field:direction`.
    ///
    /// The reference descends on `desc` exactly and ascends on anything else, `DESC`
    /// included; it matches the field case-sensitively, and answers `500` for one it does not
    /// know. A list of fields is how it states tiebreakers, which we cannot choose.
    pub(crate) fn parse(value: &str) -> Result<Self, ListingError> {
        let unsupported = || ListingError::UnsupportedSort(value.to_owned());
        if value.contains(',') {
            return Err(unsupported());
        }
        let (field, suffix) = value.split_once(':').unwrap_or((value, "asc"));
        let (key, order) = SORTS
            .iter()
            .find(|(key, _)| *key == field)
            .ok_or_else(unsupported)?;
        Ok(Self {
            key,
            order,
            descending: suffix == "desc",
        })
    }

    pub const fn direction(self) -> &'static str {
        if self.descending {
            "DESCENDING"
        } else {
            "ASCENDING"
        }
    }
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
    /// `!=`, which splits at its `=` and leaves the `!` on the name.
    Differs,
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
        ("!", Operator::Differs),
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
    pub sort: Option<Sort>,
    pub filters: Filters,
}

impl ListingRequest {
    /// The official API's order and direction: the title ascending when no sort was asked.
    pub fn ordering(&self) -> (&'static str, &'static str) {
        self.sort.map_or(("TITLE", "ASCENDING"), |sort| {
            (sort.order, sort.direction())
        })
    }
}

/// The subset of Plex's filters that maps onto `ItemFilterInput`.
#[derive(Debug, Default, PartialEq)]
pub struct Filters {
    pub title_word_prefix: Option<String>,
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
    pub content_rating: Option<String>,
    pub studio_contains: Option<String>,
    /// A resolution class: `4k`, `1080`, `sd`.
    pub resolution: Option<&'static str>,
    /// One entry per tag parameter; several combine with `AND`.
    pub tags: Vec<TagFilter>,
    /// One entry per `collection` parameter, combined the same way.
    pub collections: Vec<CollectionFilter>,
    /// `id!=` — the shelf "more by this director", without the film it was opened from.
    pub excluded_id: Option<i64>,
    pub watched: Option<bool>,
    pub in_progress: Option<bool>,
    pub all_episodes_watched: Option<bool>,
    pub any_episode_started: Option<bool>,
    /// A calendar date, `2025-11-10`.
    pub released_from: Option<String>,
    /// Seconds since the Unix epoch.
    pub last_viewed_from: Option<i64>,
    pub last_viewed_to: Option<i64>,
    pub audience_rating_from: Option<f64>,
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

/// One `collection` parameter: the numbers items carry collections under, or titles.
///
/// Read as a tag is, a number being the collection's `index` — what an item's `Collection`
/// element publishes as its `id` and its `filter`.
#[derive(Debug, PartialEq, Eq)]
pub struct CollectionFilter {
    pub numbers: Vec<i64>,
    pub titles: Vec<String>,
}

impl CollectionFilter {
    fn parse(value: &str) -> Self {
        let tag = TagFilter::parse("COLLECTION", value);
        Self {
            numbers: tag.ids,
            titles: tag.names,
        }
    }

    fn len(&self) -> usize {
        self.numbers.len() + self.titles.len()
    }

    fn to_graphql(&self) -> Value {
        let mut fields = async_graphql::indexmap::IndexMap::new();
        if !self.numbers.is_empty() {
            let numbers = self
                .numbers
                .iter()
                .map(|number| Value::String(number.to_string()))
                .collect();
            fields.insert(Name::new("tagNumbers"), Value::List(numbers));
        }
        if !self.titles.is_empty() {
            let titles = self.titles.iter().cloned().map(Value::String).collect();
            fields.insert(Name::new("titles"), Value::List(titles));
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
        if let Some(class) = self.resolution {
            set("frame", crate::resolution::filter_range(class));
        }
        if !self.tags.is_empty() {
            set(
                "tags",
                Value::List(self.tags.iter().map(TagFilter::to_graphql).collect()),
            );
        }
        if !self.collections.is_empty() {
            set(
                "collections",
                Value::List(
                    self.collections
                        .iter()
                        .map(CollectionFilter::to_graphql)
                        .collect(),
                ),
            );
        }
        if let Some(id) = self.excluded_id {
            set("excludedId", Value::String(GlobalId::encode(ITEM_TYPE, id)));
        }
        if let Some(date) = &self.released_from {
            set("releasedFrom", Value::String(date.clone()));
        }
        if let Some(time) = self.last_viewed_from {
            set("lastViewedFrom", Value::String(time.to_string()));
        }
        if let Some(time) = self.last_viewed_to {
            set("lastViewedTo", Value::String(time.to_string()));
        }
        if let Some(rating) = self
            .audience_rating_from
            .and_then(async_graphql::Number::from_f64)
        {
            set("audienceRatingFrom", Value::Number(rating));
        }
        for (name, value) in [
            ("watched", self.watched),
            ("inProgress", self.in_progress),
            ("allEpisodesWatched", self.all_episodes_watched),
            ("anyEpisodeStarted", self.any_episode_started),
        ] {
            if let Some(value) = value {
                set(name, Value::Boolean(value));
            }
        }

        if fields.is_empty() {
            Value::Null
        } else {
            Value::Object(fields)
        }
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
            // An empty `sort=` is no sort.
            "sort" if value.is_empty() => request.sort = None,
            "sort" => request.sort = Some(Sort::parse(value)?),
            _ => {}
        }
        if CONTROL_PARAMS.contains(&key.as_str()) {
            continue;
        }

        // The query string `id!=7` splits at its `=`, leaving the `!` on the name.
        if key == "id!" {
            request.filters.excluded_id = value.parse().ok();
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
        if (field, operator) == ("collection", Operator::Equals) {
            let filter = CollectionFilter::parse(value);
            if filter.len() > TagCondition::MAX_VALUES {
                return Err(ListingError::TooManyValues {
                    field: field.to_owned(),
                    count: filter.len(),
                });
            }
            request.filters.collections.push(filter);
            continue;
        }

        // A comma means OR, which `ItemFilterInput` cannot express: each of its fields is one
        // value combined with AND.
        if value.contains(',') {
            return Err(ListingError::UnsupportedFilter(field.to_owned()));
        }
        // The same field twice is an AND of two constraints. Only a range survives that, and
        // only for the fields that have one.
        if seen_filters.contains(&field) && !matches!(field, "year" | "decade" | "lastViewedAt") {
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
            filters.resolution = Some(crate::resolution::known(value).ok_or_else(unsupported)?);
        }
        // `1` asks for the flag and anything else, `true` and the empty string included, for
        // its opposite: measured on the reference, `unwatched=true` lists the watched films.
        // `!=` turns it over, which a flag can express where a year cannot.
        ("unwatched", operator @ (Operator::Equals | Operator::Differs)) => {
            filters.watched = Some(!flag(value, operator));
        }
        ("inProgress", operator @ (Operator::Equals | Operator::Differs)) => {
            filters.in_progress = Some(flag(value, operator));
        }
        ("unwatchedLeaves", operator @ (Operator::Equals | Operator::Differs)) => {
            filters.all_episodes_watched = Some(!flag(value, operator));
        }
        // What the shelves link to: out within a year, rated 7 or more, watched and not being
        // rewatched, not begun. Other values of these answer `501`.
        ("originallyAvailableAt", Operator::AtLeast) => {
            let today = open_plex_core::now_epoch().div_euclid(SECONDS_PER_DAY);
            filters.released_from = Some(calendar_date(value, today).ok_or_else(unsupported)?);
        }
        ("audienceRating", Operator::AtLeast) => {
            let rating: f64 = value.parse().map_err(|_| unsupported())?;
            if !rating.is_finite() {
                return Err(unsupported());
            }
            filters.audience_rating_from = Some(rating);
        }
        ("viewOffset", Operator::Equals) if value == "0" => filters.in_progress = Some(false),
        // "Rediscover": what was watched, last played between five years and six weeks ago.
        ("viewCount", Operator::AtLeast) if value == "1" => filters.watched = Some(true),
        ("viewCount", Operator::Equals) if value == "0" => filters.watched = Some(false),
        ("lastViewedAt", Operator::AtLeast) => {
            filters.last_viewed_from =
                Some(instant(value, open_plex_core::now_epoch()).ok_or_else(unsupported)?);
        }
        ("lastViewedAt", Operator::AtMost) => {
            filters.last_viewed_to =
                Some(instant(value, open_plex_core::now_epoch()).ok_or_else(unsupported)?);
        }
        ("viewedOrInProgressLeafCount", Operator::Equals) if value == "0" => {
            filters.any_episode_started = Some(false);
        }
        (_, Operator::Differs) => {
            return Err(ListingError::UnsupportedOperator {
                field: field.to_owned(),
                operator: "!=".to_owned(),
            });
        }
        _ => return Err(unsupported()),
    }
    Ok(())
}

const SECONDS_PER_DAY: i64 = 86_400;

/// The date a filter value names: a calendar date as given, or one counted back from
/// `today`, in days since the Unix epoch — `-1y`, `-2m`, `-6w`, `-30d`.
///
/// Months and years move the calendar and clamp to the month's last day: a year before
/// 29 February is 28 February.
fn calendar_date(value: &str, today: i64) -> Option<String> {
    let (year, month, day) = if let Some(relative) = value.strip_prefix('-') {
        counted_back(relative, today)?
    } else {
        written(value)?
    };
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// `1y`, `2m`, `6w` or `30d` before `today`.
fn counted_back(relative: &str, today: i64) -> Option<(i64, i64, i64)> {
    let split = relative.len().checked_sub(1)?;
    let (count, unit) = (relative.get(..split)?, relative.get(split..)?);
    let count: i64 = count.parse().ok()?;
    let months = match unit {
        "d" => return Some(civil_from_days(today.checked_sub(count)?)),
        "w" => return Some(civil_from_days(today.checked_sub(count.checked_mul(7)?)?)),
        "m" => count,
        "y" => count.checked_mul(12)?,
        _ => return None,
    };
    let (year, month, day) = civil_from_days(today);
    let total = (year * 12 + month - 1).checked_sub(months)?;
    let (year, month) = (total.div_euclid(12), total.rem_euclid(12) + 1);
    Some((year, month, day.min(days_in_month(year, month))))
}

/// `2024-02-29`, when it is a date the calendar has.
fn written(value: &str) -> Option<(i64, i64, i64)> {
    let mut parts = value.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let (year, month, day): (i64, i64, i64) =
        (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    ((1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day))
        .then_some((year, month, day))
}

/// The time a filter value names, in seconds since the Unix epoch: one given as such, or one
/// counted back from `now` as [`calendar_date`] counts, at the same time of day.
fn instant(value: &str, now: i64) -> Option<i64> {
    if !value.starts_with('-') {
        return value.parse().ok();
    }
    let today = now.div_euclid(SECONDS_PER_DAY);
    let (year, month, day) = counted_back(value.strip_prefix('-')?, today)?;
    let back = today.checked_sub(days_from_civil(year, month, day))?;
    now.checked_sub(back.checked_mul(SECONDS_PER_DAY)?)
}

/// Howard Hinnant's `days_from_civil`, the inverse of [`civil_from_days`].
const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let march_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * march_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

const fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The proleptic Gregorian date of a day count, by Howard Hinnant's `civil_from_days`.
const fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    };
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

/// Whether a flag filter asks for the flag: `1` does, anything else asks for its opposite, and
/// `!=` turns the answer over.
fn flag(value: &str, operator: Operator) -> bool {
    (value == "1") != matches!(operator, Operator::Differs)
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
        "unwatched",
        "inProgress",
        "unwatchedLeaves",
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
    fn reads_the_watch_flags_as_the_reference_does() {
        let filters = |key: &str, value: &str| parse(&pairs(&[(key, value)])).unwrap().filters;
        assert_eq!(filters("unwatched", "1").watched, Some(false));
        // Anything but `1` is the opposite flag.
        for value in ["0", "true", "2", ""] {
            assert_eq!(filters("unwatched", value).watched, Some(true), "{value}");
            assert_eq!(
                filters("inProgress", value).in_progress,
                Some(false),
                "{value}"
            );
        }
        assert_eq!(filters("inProgress", "1").in_progress, Some(true));
        assert_eq!(
            filters("unwatchedLeaves", "1").all_episodes_watched,
            Some(false)
        );
        assert_eq!(
            filters("unwatchedLeaves", "0").all_episodes_watched,
            Some(true)
        );
        // `!=` turns a flag over: `unwatched!=1` lists the watched ones.
        assert_eq!(filters("unwatched!", "1").watched, Some(true));
        assert_eq!(filters("inProgress!", "1").in_progress, Some(false));
        // A repeated flag stays refused: `unwatched=1&unwatched=0` is empty on the reference,
        // which one flag cannot express.
        assert!(parse(&pairs(&[("unwatched", "1"), ("unwatched", "0")])).is_err());
        // And negation stays refused where it is not a flag.
        assert!(matches!(
            parse(&pairs(&[("year!", "2022")])),
            Err(ListingError::UnsupportedOperator { .. })
        ));
    }

    #[test]
    fn a_release_date_is_absolute_or_counted_back_from_today() {
        // 2026-11-10 is day 20,767 of the epoch.
        let today = 20_767;
        assert_eq!(civil_from_days(today), (2026, 11, 10));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(calendar_date("-1y", today).as_deref(), Some("2025-11-10"));
        assert_eq!(calendar_date("-6w", today).as_deref(), Some("2026-09-29"));
        assert_eq!(calendar_date("-30d", today).as_deref(), Some("2026-10-11"));
        assert_eq!(calendar_date("-11m", today).as_deref(), Some("2025-12-10"));
        assert_eq!(
            calendar_date("2024-02-29", today).as_deref(),
            Some("2024-02-29")
        );
        // A year before a 29 February is the 28th.
        let leap_day = 19_782;
        assert_eq!(civil_from_days(leap_day), (2024, 2, 29));
        assert_eq!(
            calendar_date("-1y", leap_day).as_deref(),
            Some("2023-02-28")
        );
        for value in [
            "-1x",
            "-y",
            "-",
            "yesterday",
            "2023-02-29",
            "2024-13-01",
            "1700000000",
        ] {
            assert_eq!(calendar_date(value, today), None, "{value}");
        }
    }

    #[test]
    fn a_time_is_given_or_counted_back_at_the_same_hour() {
        // 2026-11-10 13:00 UTC.
        let now = 20_767 * 86_400 + 13 * 3_600;
        assert_eq!(days_from_civil(2026, 11, 10), 20_767);
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(instant("-6w", now), Some(now - 42 * 86_400));
        // Five calendar years, one of them leap: 1,826 days.
        assert_eq!(instant("-5y", now), Some(now - 1_826 * 86_400));
        assert_eq!(instant("1784650830", now), Some(1_784_650_830));
        assert_eq!(instant("-6x", now), None);
        assert_eq!(instant("soon", now), None);
    }

    #[test]
    fn reads_what_rediscover_links_to() {
        let request = parse(&pairs(&[
            ("type", "2"),
            ("lastViewedAt<", "-6w"),
            ("lastViewedAt>", "-5y"),
            ("unwatchedLeaves", "1"),
            ("viewCount>", "1"),
        ]))
        .unwrap();
        let filters = request.filters;
        assert!(filters.last_viewed_from < filters.last_viewed_to);
        assert_eq!(filters.watched, Some(true));
        assert_eq!(filters.all_episodes_watched, Some(false));
        assert!(parse(&pairs(&[("viewCount>", "3")])).is_err());
    }

    #[test]
    fn reads_what_the_shelves_link_to() {
        let request = parse(&pairs(&[
            ("unwatched", "0"),
            ("viewOffset", "0"),
            ("audienceRating>", "7.0"),
            ("viewedOrInProgressLeafCount", "0"),
        ]))
        .unwrap();
        assert_eq!(request.filters.in_progress, Some(false));
        assert_eq!(request.filters.audience_rating_from, Some(7.0));
        assert_eq!(request.filters.any_episode_started, Some(false));
        let recent = parse(&pairs(&[("originallyAvailableAt>", "-1y")])).unwrap();
        assert!(recent.filters.released_from.is_some());
        // Values beyond those are refused, not dropped.
        for (key, value) in [
            ("viewOffset>", "300"),
            ("viewOffset", "5"),
            ("viewedOrInProgressLeafCount", "2"),
            ("audienceRating>", "NaN"),
            ("originallyAvailableAt", "2020-01-01"),
        ] {
            assert!(parse(&pairs(&[(key, value)])).is_err(), "{key}{value}");
        }
    }

    #[test]
    fn reads_the_sort_as_the_reference_does() {
        let sort = |value: &str| Sort::parse(value).map(|sort| (sort.order, sort.descending));
        assert_eq!(sort("rating:desc").unwrap(), ("RATING", true));
        assert_eq!(sort("rating").unwrap(), ("RATING", false));
        // Only `desc` descends: `DESC` and anything else ascend, as measured.
        assert_eq!(sort("rating:DESC").unwrap(), ("RATING", false));
        assert_eq!(sort("rating:nullsLast").unwrap(), ("RATING", false));
        assert_eq!(
            sort("episode.addedAt:desc").unwrap(),
            ("EPISODE_ADDED_AT", true)
        );
        // Refused rather than answered in title order: a field we do not order by, one
        // spelt in another case, and a list of tiebreakers.
        for value in [
            "year:desc",
            "Rating:desc",
            "unviewedLeafCount",
            "lastViewedAt:desc,titleSort",
        ] {
            assert!(
                matches!(sort(value), Err(ListingError::UnsupportedSort(_))),
                "{value}"
            );
        }
    }

    #[test]
    fn an_empty_sort_is_none() {
        let request = parse(&pairs(&[("sort", "")])).expect("parses");
        assert_eq!(request.sort, None);
        assert_eq!(request.ordering(), ("TITLE", "ASCENDING"));
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
        assert_eq!(
            request.sort,
            Some(Sort {
                key: "addedAt",
                order: "ADDED_AT",
                descending: true
            })
        );
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
    fn resolution_takes_the_classes_the_reference_lists() {
        let filters = parse(&pairs(&[("resolution", "2.7k")]))
            .expect("parses")
            .filters;
        assert_eq!(filters.resolution, Some("2.7k"));
        // Not a class of the reference's: refused rather than ignored.
        assert!(parse(&pairs(&[("resolution", "6k")])).is_err());
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
        for field in ["label", "network", "hdr"] {
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
    fn a_collection_is_named_by_number_or_title() {
        // Measured: `collection=187346` and `collection=La 7ème compagnie - Saga` list the
        // same three films on the reference.
        let filters = parse(&pairs(&[("collection", "187346,La 7ème compagnie - Saga")]))
            .expect("parses")
            .filters;
        assert_eq!(
            filters.collections,
            [CollectionFilter {
                numbers: vec![187_346],
                titles: vec!["La 7ème compagnie - Saga".to_owned()],
            }]
        );
        let rendered = filters.to_graphql(None).to_string();
        assert!(rendered.contains("tagNumbers"), "{rendered}");
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
