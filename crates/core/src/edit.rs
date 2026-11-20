//! Editing an item's metadata, and the fields an editor locks.
//!
//! A lock says a metadata refresh must leave a field alone: it is how a hand-made correction
//! survives the next match. Locking is separate from editing, as on the reference server: a
//! value written there stays unlocked unless the request also locks it, and Plex Web is the one
//! that sends both together when a field is typed into.
//!
//! An edit also rewrites what it derives, by the rules measured on the reference:
//!
//! - a new title gives a new sort title — the title less a leading article, accents folded —
//!   unless the sort title is locked or set by the same edit. An emptied title leaves it alone;
//! - a new release date gives its year, and clearing the date clears the year, unless the
//!   same edit sets the year. A locked year does not stop it: locks bind refreshes, not users.
//!
//! Locks change before values, so an edit that unlocks the sort title and retitles the item
//! derives the sort title afresh.

use std::collections::BTreeSet;

use crate::metadata::{ItemKind, MetadataItem};

/// A field of an item an editor can lock.
///
/// Declared in the order the reference numbers them, which is the order it lists them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemField {
    Title,
    TitleSort,
    OriginalTitle,
    Studio,
    /// The critics' rating.
    Rating,
    Tagline,
    Summary,
    ContentRating,
    Poster,
    Background,
    ReleasedOn,
    Year,
    Genres,
    Collections,
    Directors,
    Writers,
    Cast,
    Producers,
    Countries,
    AudienceRating,
}

impl ItemField {
    pub const ALL: [Self; 20] = [
        Self::Title,
        Self::TitleSort,
        Self::OriginalTitle,
        Self::Studio,
        Self::Rating,
        Self::Tagline,
        Self::Summary,
        Self::ContentRating,
        Self::Poster,
        Self::Background,
        Self::ReleasedOn,
        Self::Year,
        Self::Genres,
        Self::Collections,
        Self::Directors,
        Self::Writers,
        Self::Cast,
        Self::Producers,
        Self::Countries,
        Self::AudienceRating,
    ];

    /// Whether items of this kind hold the field.
    ///
    /// The fields each kind publishes. A series has no critics' rating, and an episode no
    /// studio, tagline or collection, and takes its genres and countries from its series; a
    /// season is little more than a number, a title and its artwork.
    pub const fn applies_to(self, kind: ItemKind) -> bool {
        match kind {
            ItemKind::Movie => true,
            ItemKind::Show => !matches!(self, Self::Rating),
            ItemKind::Season => matches!(
                self,
                Self::Title | Self::TitleSort | Self::Summary | Self::Poster | Self::Background
            ),
            ItemKind::Episode => !matches!(
                self,
                Self::Studio
                    | Self::Rating
                    | Self::Tagline
                    | Self::Collections
                    | Self::Genres
                    | Self::Countries
            ),
        }
    }
}

/// A day of the proleptic Gregorian calendar, in years 1 to 9999: what a release date is.
/// Stored as its midnight UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CalendarDate {
    year: i32,
    month: u8,
    day: u8,
}

const SECONDS_PER_DAY: i64 = 86_400;

/// Days from 0000-03-01 to the Unix epoch. Counting from March puts the leap day last.
const EPOCH_FROM_MARCH_0000: i64 = 719_468;

/// Days in a 400-year cycle of the Gregorian calendar.
const DAYS_PER_ERA: i64 = 146_097;

impl CalendarDate {
    /// The UTC day `seconds` since the Unix epoch falls on, if it is in years 1 to 9999.
    pub fn containing(seconds: i64) -> Option<Self> {
        // Howard Hinnant's `civil_from_days`.
        let days = seconds
            .div_euclid(SECONDS_PER_DAY)
            .checked_add(EPOCH_FROM_MARCH_0000)?;
        let era = days.div_euclid(DAYS_PER_ERA);
        let day_of_era = days.rem_euclid(DAYS_PER_ERA);
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_from_march = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
        let month = if month_from_march < 10 {
            month_from_march + 3
        } else {
            month_from_march - 9
        };
        let year = era * 400 + year_of_era + i64::from(month <= 2);
        (1..=9999).contains(&year).then_some(Self {
            year: i32::try_from(year).ok()?,
            month: u8::try_from(month).ok()?,
            day: u8::try_from(day).ok()?,
        })
    }

    /// The day's midnight UTC, in seconds since the Unix epoch.
    pub fn midnight(self) -> i64 {
        // Howard Hinnant's `days_from_civil`.
        let (month, day) = (i64::from(self.month), i64::from(self.day));
        let year = i64::from(self.year) - i64::from(month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year.rem_euclid(400);
        let month_from_march = if month > 2 { month - 3 } else { month + 9 };
        let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        (era * DAYS_PER_ERA + day_of_era - EPOCH_FROM_MARCH_0000) * SECONDS_PER_DAY
    }

    /// Reads `YYYY-MM-DD`, refusing a day the month does not have.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('-');
        let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some()
            || year.len() != 4
            || month.len() != 2
            || day.len() != 2
            || ![year, month, day]
                .iter()
                .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return None;
        }
        let (year, month, day): (i32, u8, u8) =
            (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days_in_month = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return None,
        };
        (year >= 1 && (1..=days_in_month).contains(&day)).then_some(Self { year, month, day })
    }

    pub const fn year(self) -> i32 {
        self.year
    }
}

impl std::fmt::Display for CalendarDate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}",
            self.year, self.month, self.day
        )
    }
}

/// A new value for one field. `None` clears it.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldEdit {
    Title(Option<String>),
    TitleSort(Option<String>),
    OriginalTitle(Option<String>),
    Summary(Option<String>),
    Tagline(Option<String>),
    Studio(Option<String>),
    ContentRating(Option<String>),
    Year(Option<i32>),
    ReleasedOn(Option<CalendarDate>),
    /// Out of 10.
    Rating(Option<f64>),
    /// Out of 10.
    AudienceRating(Option<f64>),
}

impl FieldEdit {
    pub const fn field(&self) -> ItemField {
        match self {
            Self::Title(_) => ItemField::Title,
            Self::TitleSort(_) => ItemField::TitleSort,
            Self::OriginalTitle(_) => ItemField::OriginalTitle,
            Self::Summary(_) => ItemField::Summary,
            Self::Tagline(_) => ItemField::Tagline,
            Self::Studio(_) => ItemField::Studio,
            Self::ContentRating(_) => ItemField::ContentRating,
            Self::Year(_) => ItemField::Year,
            Self::ReleasedOn(_) => ItemField::ReleasedOn,
            Self::Rating(_) => ItemField::Rating,
            Self::AudienceRating(_) => ItemField::AudienceRating,
        }
    }

    /// Whether the value is one the field can hold. Text always is.
    fn in_range(&self) -> bool {
        match self {
            Self::Year(Some(year)) => (1..=9999).contains(year),
            Self::Rating(Some(score)) | Self::AudienceRating(Some(score)) => {
                (0.0..=10.0).contains(score)
            }
            _ => true,
        }
    }

    /// An empty text is no text: storage keeps absence, never an empty string.
    fn normalized(self) -> Self {
        let text = |value: Option<String>| value.filter(|text| !text.is_empty());
        match self {
            Self::Title(value) => Self::Title(text(value)),
            Self::TitleSort(value) => Self::TitleSort(text(value)),
            Self::OriginalTitle(value) => Self::OriginalTitle(text(value)),
            Self::Summary(value) => Self::Summary(text(value)),
            Self::Tagline(value) => Self::Tagline(text(value)),
            Self::Studio(value) => Self::Studio(text(value)),
            Self::ContentRating(value) => Self::ContentRating(text(value)),
            other => other,
        }
    }
}

/// Why an edit cannot be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("{0:?} is not a field of this kind of item")]
    NotApplicable(ItemField),
    #[error("the value given for {0:?} is out of its range")]
    OutOfRange(ItemField),
    #[error("{0:?} is set twice")]
    SetTwice(ItemField),
    #[error("{0:?} is both locked and unlocked")]
    LockedAndUnlocked(ItemField),
}

/// Changes to one item: new values, and fields locked or unlocked.
///
/// Built through [`ItemEdit::new`], which checks it against the kind of item it is for, so an
/// edit that exists is one that can be applied.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemEdit {
    kind: ItemKind,
    values: Vec<FieldEdit>,
    lock: BTreeSet<ItemField>,
    unlock: BTreeSet<ItemField>,
}

impl ItemEdit {
    /// # Errors
    ///
    /// Refuses a field the kind does not hold, a value out of its field's range, a field given
    /// two values, and a field both locked and unlocked.
    pub fn new(
        kind: ItemKind,
        values: Vec<FieldEdit>,
        lock: impl IntoIterator<Item = ItemField>,
        unlock: impl IntoIterator<Item = ItemField>,
    ) -> Result<Self, EditError> {
        let lock: BTreeSet<ItemField> = lock.into_iter().collect();
        let unlock: BTreeSet<ItemField> = unlock.into_iter().collect();
        let mut seen = BTreeSet::new();
        for value in &values {
            let field = value.field();
            if !field.applies_to(kind) {
                return Err(EditError::NotApplicable(field));
            }
            if !value.in_range() {
                return Err(EditError::OutOfRange(field));
            }
            if !seen.insert(field) {
                return Err(EditError::SetTwice(field));
            }
        }
        if let Some(field) = lock.iter().chain(&unlock).find(|f| !f.applies_to(kind)) {
            return Err(EditError::NotApplicable(*field));
        }
        if let Some(field) = lock.intersection(&unlock).next() {
            return Err(EditError::LockedAndUnlocked(*field));
        }
        Ok(Self {
            kind,
            values: values.into_iter().map(FieldEdit::normalized).collect(),
            lock,
            unlock,
        })
    }

    /// The kind of item the edit was checked against.
    pub const fn kind(&self) -> ItemKind {
        self.kind
    }

    fn sets(&self, field: ItemField) -> bool {
        self.values.iter().any(|value| value.field() == field)
    }

    /// Applies the edit and what it derives to an item and its locks.
    ///
    /// The caller checks the item is of the edit's kind.
    pub fn apply(&self, item: &mut MetadataItem, locked: &mut BTreeSet<ItemField>) {
        locked.extend(&self.lock);
        locked.retain(|field| !self.unlock.contains(field));

        for value in &self.values {
            match value.clone() {
                FieldEdit::Title(title) => {
                    let derives =
                        !locked.contains(&ItemField::TitleSort) && !self.sets(ItemField::TitleSort);
                    if let (Some(title), true) = (&title, derives) {
                        item.title_sort = derived_title_sort(title);
                    }
                    item.title = title;
                }
                FieldEdit::TitleSort(sort) => item.title_sort = sort,
                FieldEdit::OriginalTitle(text) => item.original_title = text,
                FieldEdit::Summary(text) => item.summary = text,
                FieldEdit::Tagline(text) => item.tagline = text,
                FieldEdit::Studio(text) => item.studio = text,
                FieldEdit::ContentRating(text) => item.content_rating = text,
                FieldEdit::Year(year) => item.year = year,
                FieldEdit::ReleasedOn(date) => {
                    if !self.sets(ItemField::Year) {
                        item.year = date.map(CalendarDate::year);
                    }
                    item.released_on = date.map(CalendarDate::midnight);
                }
                FieldEdit::Rating(score) => item.rating = score,
                FieldEdit::AudienceRating(score) => item.audience_rating = score,
            }
        }
    }
}

/// The words a title may open with and still sort by the next one: the reference's default
/// `ArticleStrings` preference, for every library whatever its language.
const ARTICLES: [&str; 7] = ["the", "das", "der", "a", "an", "el", "la"];

/// The sort title the reference derives from a title it is given.
///
/// Measured on twenty titles: spaces trimmed, one leading article dropped when a word follows
/// it after a space (`The The Band` sorts as `The Band`, `The` as itself, `the\tTab` as
/// itself), and accents folded with case kept (`Les Misérables` sorts as `Les Miserables`,
/// since `les` is not on the list). `None` for a title of spaces alone.
pub fn derived_title_sort(title: &str) -> Option<String> {
    let trimmed = title.trim_matches(' ');
    let without_article = trimmed
        .split_once(' ')
        .filter(|(first, _)| ARTICLES.iter().any(|a| first.eq_ignore_ascii_case(a)))
        .map_or(trimmed, |(_, rest)| rest.trim_start_matches(' '));
    let folded = crate::search::without_accents(without_article);
    (!folded.is_empty()).then_some(folded)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::{RatingKey, SectionId};

    fn film() -> MetadataItem {
        MetadataItem {
            id: RatingKey::new(577_986),
            section_id: SectionId::new(27),
            parent_id: None,
            kind: ItemKind::Movie,
            guid: "tv.plex.agents.none://577986".to_owned(),
            title: Some("Soul".to_owned()),
            title_sort: Some("Soul".to_owned()),
            original_title: None,
            summary: None,
            tagline: None,
            studio: None,
            content_rating: None,
            content_rating_age: None,
            position: Some(1),
            year: Some(2021),
            released_on: None,
            duration_ms: None,
            rating: None,
            audience_rating: None,
            added_at: 0,
            updated_at: 0,
            refreshed_at: None,
            chapter_source: None,
        }
    }

    fn edited(values: Vec<FieldEdit>, lock: &[ItemField], unlock: &[ItemField]) -> MetadataItem {
        let mut item = film();
        let mut locked = BTreeSet::from([ItemField::Studio]);
        ItemEdit::new(
            ItemKind::Movie,
            values,
            lock.iter().copied(),
            unlock.iter().copied(),
        )
        .unwrap()
        .apply(&mut item, &mut locked);
        item
    }

    #[test]
    fn derives_sort_titles_as_the_reference_does() {
        let cases = [
            ("La Haine", Some("Haine")),
            ("Le Mans", Some("Le Mans")),
            ("THE END", Some("END")),
            ("The", Some("The")),
            ("The ", Some("The")),
            ("the   Three", Some("Three")),
            ("An American", Some("American")),
            ("Das Boot", Some("Boot")),
            ("A.I.", Some("A.I.")),
            ("The-Thing", Some("The-Thing")),
            ("L'Amour", Some("L'Amour")),
            ("Les Misérables", Some("Les Miserables")),
            ("  The Lead", Some("Lead")),
            ("The The Band", Some("The Band")),
            ("Ánimo", Some("Animo")),
            ("the\tTab", Some("the\tTab")),
            ("   ", None),
        ];
        for (title, expected) in cases {
            assert_eq!(derived_title_sort(title).as_deref(), expected, "{title:?}");
        }
    }

    #[test]
    fn a_new_title_gives_a_new_sort_title_unless_it_is_locked_or_given() {
        let retitled = edited(vec![FieldEdit::Title(Some("The Soul".into()))], &[], &[]);
        assert_eq!(retitled.title_sort.as_deref(), Some("Soul"));

        let locked = edited(
            vec![FieldEdit::Title(Some("The Probe".into()))],
            &[ItemField::TitleSort],
            &[],
        );
        assert_eq!(locked.title_sort.as_deref(), Some("Soul"));

        let given = edited(
            vec![
                FieldEdit::Title(Some("The Probe".into())),
                FieldEdit::TitleSort(Some("Custom".into())),
            ],
            &[],
            &[],
        );
        assert_eq!(given.title_sort.as_deref(), Some("Custom"));
    }

    #[test]
    fn an_emptied_title_keeps_its_sort_title() {
        let emptied = edited(vec![FieldEdit::Title(Some(String::new()))], &[], &[]);
        assert_eq!(emptied.title, None);
        assert_eq!(emptied.title_sort.as_deref(), Some("Soul"));
    }

    #[test]
    fn a_date_is_its_midnight_utc_and_back() {
        for (text, midnight) in [
            ("1970-01-01", 0),
            ("1969-12-31", -86_400),
            ("2000-02-29", 951_782_400),
            ("2009-12-16", 1_260_921_600),
            ("0001-01-01", -62_135_596_800),
            ("9999-12-31", 253_402_214_400),
        ] {
            let date = CalendarDate::parse(text).unwrap();
            assert_eq!(date.midnight(), midnight, "{text}");
            assert_eq!(CalendarDate::containing(midnight), Some(date), "{text}");
            assert_eq!(
                CalendarDate::containing(midnight + 86_399),
                Some(date),
                "{text}"
            );
            assert_eq!(date.to_string(), text);
        }
        assert_eq!(CalendarDate::containing(-62_135_596_801), None);
        assert_eq!(CalendarDate::containing(253_402_300_800), None);
        assert_eq!(CalendarDate::containing(i64::MIN), None);
        assert_eq!(CalendarDate::containing(i64::MAX), None);
    }

    #[test]
    fn a_release_date_gives_its_year_unless_the_year_is_given() {
        let date = CalendarDate::parse("2019-05-01");
        let dated = edited(vec![FieldEdit::ReleasedOn(date)], &[], &[]);
        assert_eq!(
            (dated.year, dated.released_on),
            (Some(2019), Some(1_556_668_800))
        );

        let cleared = edited(vec![FieldEdit::ReleasedOn(None)], &[ItemField::Year], &[]);
        assert_eq!((cleared.year, cleared.released_on), (None, None));

        let both = edited(
            vec![FieldEdit::ReleasedOn(date), FieldEdit::Year(Some(2001))],
            &[],
            &[],
        );
        assert_eq!(both.year, Some(2001));
    }

    #[test]
    fn locks_change_before_values_and_editing_locks_nothing() {
        let mut item = film();
        let mut locked = BTreeSet::from([ItemField::TitleSort, ItemField::Studio]);
        ItemEdit::new(
            ItemKind::Movie,
            vec![FieldEdit::Title(Some("A Soul Again".into()))],
            [ItemField::Summary],
            [ItemField::TitleSort],
        )
        .unwrap()
        .apply(&mut item, &mut locked);
        assert_eq!(item.title_sort.as_deref(), Some("Soul Again"));
        assert_eq!(
            locked,
            BTreeSet::from([ItemField::Studio, ItemField::Summary])
        );
    }

    #[test]
    fn refuses_what_the_kind_does_not_hold_or_cannot_hold() {
        let episode = |values: Vec<FieldEdit>, lock: Vec<ItemField>| {
            ItemEdit::new(ItemKind::Episode, values, lock, [])
        };
        assert_eq!(
            episode(vec![FieldEdit::Studio(Some("HBO".into()))], vec![]),
            Err(EditError::NotApplicable(ItemField::Studio))
        );
        assert_eq!(
            episode(vec![], vec![ItemField::Genres]),
            Err(EditError::NotApplicable(ItemField::Genres))
        );
        assert_eq!(
            episode(vec![FieldEdit::AudienceRating(Some(10.5))], vec![]),
            Err(EditError::OutOfRange(ItemField::AudienceRating))
        );
        assert_eq!(
            episode(vec![FieldEdit::Year(Some(0))], vec![]),
            Err(EditError::OutOfRange(ItemField::Year))
        );
        assert_eq!(
            episode(
                vec![FieldEdit::Summary(None), FieldEdit::Summary(None)],
                vec![]
            ),
            Err(EditError::SetTwice(ItemField::Summary))
        );
        assert_eq!(
            ItemEdit::new(ItemKind::Show, vec![], [ItemField::Year], [ItemField::Year]),
            Err(EditError::LockedAndUnlocked(ItemField::Year))
        );
    }

    #[test]
    fn reads_calendar_dates_and_refuses_impossible_ones() {
        assert_eq!(
            CalendarDate::parse("2024-02-29").map(|date| date.to_string()),
            Some("2024-02-29".to_owned())
        );
        for refused in [
            "2023-02-29",
            "2020-13-01",
            "2020-1-01",
            "garbage",
            "0000-01-01",
            "2020-01-01-01",
            "+020-01-01",
        ] {
            assert_eq!(CalendarDate::parse(refused), None, "{refused}");
        }
    }
}
