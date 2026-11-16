//! The order of a listing, as SQL: `ORDER BY`, the keyset condition a cursor resumes from,
//! and the cursor read back from each row.
//!
//! An ordering is a list of terms compared in turn. A listing by title is the sort key then
//! the id, both in the listing's direction; any other is its own value, then the sort key and
//! the id ascending — ties in title order whichever way the listing runs, as on the
//! reference.
//!
//! Absence sorts first. `NULL` is the smallest value on `SQLite`, but `PostgreSQL` makes it
//! the largest, so the position is stated in every `ORDER BY` and spelt out in every keyset
//! comparison rather than left to the engine.

use open_plex_core::metadata::{ItemKind, ItemQuery, ItemSort, MetadataError, SearchOrder};
use open_plex_core::page::{Cursor, CursorKey, Direction};
use sea_orm::sea_query::{
    Alias, CaseStatement, Expr, Func, JoinType, NullOrdering, Query, SimpleExpr,
};
use sea_orm::{Condition, Order, QueryFilter, QueryOrder, QueryResult, QuerySelect, Select};

use crate::entity::{media, metadata_item, watch_state};
use crate::metadata_repository::kind_name;

/// The columns the leading values are read back from, one per value.
const KEY_COLUMNS: [&str; 4] = ["order_key", "order_key_1", "order_key_2", "order_key_3"];

/// What an absent season or episode number, or release date, is read as in
/// [`ItemSort::Episodes`]: past every real one, so that absence sorts last there.
const ABSENT_LAST: i64 = i64::MAX;

/// A Mersenne prime above every id, the modulus of the shuffle.
const SHUFFLE_MODULUS: i64 = 2_147_483_647;

/// The shape of the leading value, to read it back for the cursor.
#[derive(Debug, Clone, Copy)]
enum KeyType {
    Int,
    Real,
    Text,
}

#[derive(Debug, Clone)]
struct Leading {
    expr: SimpleExpr,
    key_type: KeyType,
    nullable: bool,
}

#[derive(Debug, Clone)]
struct Term {
    expr: SimpleExpr,
    descending: bool,
    nullable: bool,
}

/// What ties on the leading value follow before the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ties {
    Title,
    /// The id alone: the order items arrived in.
    Arrival,
    /// The title, except for series, which follow their ids.
    TitleExceptSeries,
}

/// How a listing is ordered.
#[derive(Debug, Clone)]
pub(crate) struct ItemOrder {
    /// The ordering's own values, compared in turn; none orders by title.
    leading: Vec<Leading>,
    ties: Ties,
    direction: Direction,
}

impl ItemOrder {
    pub(crate) fn new(query: &ItemQuery) -> Self {
        let (sort, direction) = (query.sort, query.direction);
        let column = |column: metadata_item::Column, key_type, nullable| Leading {
            expr: Expr::col((metadata_item::Entity, column)).into(),
            key_type,
            nullable,
        };
        let leading = match sort {
            ItemSort::Title => Vec::new(),
            ItemSort::AddedAt | ItemSort::Arrival => {
                vec![column(metadata_item::Column::AddedAt, KeyType::Int, false)]
            }
            ItemSort::ReleasedOn => vec![column(
                metadata_item::Column::ReleasedOn,
                KeyType::Int,
                true,
            )],
            ItemSort::Rating => vec![column(metadata_item::Column::Rating, KeyType::Real, true)],
            ItemSort::AudienceRating => vec![column(
                metadata_item::Column::AudienceRating,
                KeyType::Real,
                true,
            )],
            ItemSort::Duration => vec![version_extreme(media::Column::DurationMs, direction)],
            ItemSort::Height => vec![version_extreme(media::Column::Height, direction)],
            ItemSort::LastViewedAt(viewer) => vec![Leading {
                expr: viewer.map_or_else(
                    || Expr::value(Option::<i64>::None),
                    |viewer| {
                        scalar(
                            Query::select()
                                .column(watch_state::Column::LastViewedAt)
                                .from(watch_state::Entity)
                                .and_where(
                                    Expr::col((watch_state::Entity, watch_state::Column::Viewer))
                                        .eq(viewer.get()),
                                )
                                .and_where(
                                    Expr::col((watch_state::Entity, watch_state::Column::Guid))
                                        .equals((
                                            metadata_item::Entity,
                                            metadata_item::Column::Guid,
                                        )),
                                )
                                .to_owned(),
                        )
                    },
                ),
                key_type: KeyType::Int,
                nullable: true,
            }],
            ItemSort::UserRating(viewer) => vec![Leading {
                expr: crate::metadata_repository::viewer_rating(viewer),
                key_type: KeyType::Real,
                nullable: true,
            }],
            ItemSort::EpisodeAddedAt => vec![Leading {
                expr: episode_added_at(direction),
                key_type: KeyType::Int,
                nullable: true,
            }],
            ItemSort::Episodes => episode_keys(),
            ItemSort::Shuffled(seed) => vec![Leading {
                expr: shuffled(seed),
                key_type: KeyType::Int,
                nullable: false,
            }],
            ItemSort::Filmography => vec![Leading {
                expr: library_rank(&query.sections),
                key_type: KeyType::Int,
                nullable: false,
            }],
        };
        let ties = match sort {
            ItemSort::Arrival => Ties::Arrival,
            ItemSort::Filmography => Ties::TitleExceptSeries,
            _ => Ties::Title,
        };
        Self {
            leading,
            ties,
            direction,
        }
    }

    fn terms(&self) -> Vec<Term> {
        let descending = self.direction.is_descending();
        let column = |column: metadata_item::Column, descending| Term {
            expr: Expr::col((metadata_item::Entity, column)).into(),
            descending,
            nullable: false,
        };
        if self.leading.is_empty() {
            return vec![
                column(metadata_item::Column::SortKey, descending),
                column(metadata_item::Column::Id, descending),
            ];
        }
        let mut terms: Vec<Term> = self
            .leading
            .iter()
            .map(|leading| Term {
                expr: leading.expr.clone(),
                descending,
                nullable: leading.nullable,
            })
            .collect();
        match self.ties {
            Ties::Title => terms.push(column(metadata_item::Column::SortKey, false)),
            Ties::TitleExceptSeries => terms.push(Term {
                expr: title_except_series(),
                descending: false,
                nullable: false,
            }),
            Ties::Arrival => {}
        }
        terms.push(column(metadata_item::Column::Id, false));
        terms
    }

    /// Restricts `select` to the rows past `cursor`: after it, or before it.
    pub(crate) fn resume(
        &self,
        select: Select<metadata_item::Entity>,
        cursor: &Cursor,
        after: bool,
    ) -> Result<Select<metadata_item::Entity>, MetadataError> {
        resume_terms(&self.terms(), select, cursor, after)
    }

    /// Orders `select`, reading from the far end for a backward page.
    pub(crate) fn sort(
        &self,
        mut select: Select<metadata_item::Entity>,
        reversed: bool,
    ) -> Select<metadata_item::Entity> {
        for (leading, alias) in self.leading.iter().zip(KEY_COLUMNS) {
            select = select.column_as(leading.expr.clone(), alias);
        }
        sort_terms(&self.terms(), select, reversed)
    }

    /// The cursor locating a row of a listing ordered by [`ItemOrder::sort`].
    pub(crate) fn cursor(
        &self,
        row: &QueryResult,
        item: &metadata_item::Model,
    ) -> Result<Cursor, sea_orm::DbErr> {
        let sort_key = CursorKey::Text(item.sort_key.clone());
        if self.leading.is_empty() {
            return Ok(Cursor::new(sort_key, item.id));
        }
        let mut keys = Vec::with_capacity(self.leading.len() + 1);
        for (leading, alias) in self.leading.iter().zip(KEY_COLUMNS) {
            let value = match leading.key_type {
                KeyType::Int => row.try_get::<Option<i64>>("", alias)?.map(CursorKey::Int),
                KeyType::Real => row.try_get::<Option<f64>>("", alias)?.map(CursorKey::Real),
                KeyType::Text => row
                    .try_get::<Option<String>>("", alias)?
                    .map(CursorKey::Text),
            };
            keys.push(value.unwrap_or(CursorKey::Absent));
        }
        match self.ties {
            Ties::TitleExceptSeries if item.kind == kind_name(ItemKind::Show) => {
                keys.push(CursorKey::Text(String::new()));
            }
            Ties::Title | Ties::TitleExceptSeries => keys.push(sort_key),
            Ties::Arrival => {}
        }
        Ok(Cursor { keys, id: item.id })
    }
}

/// The order of a search across libraries, whose cursors `SearchQuery::cursor` writes.
#[derive(Debug, Clone)]
pub(crate) struct SearchRanking {
    terms: Vec<Term>,
}

impl SearchRanking {
    /// By relevance, `hundredths` is the score's leading part, as
    /// `open_plex_core::search::hundredths` computes it for each row.
    pub(crate) fn new(order: SearchOrder, hundredths: SimpleExpr) -> Self {
        let column = |column: metadata_item::Column, descending| Term {
            expr: Expr::col((metadata_item::Entity, column)).into(),
            descending,
            nullable: false,
        };
        let mut terms = Vec::new();
        if order == SearchOrder::Relevance {
            terms.push(Term {
                expr: hundredths,
                descending: true,
                nullable: false,
            });
            // Absent counts as zero, as it does in the score.
            terms.push(Term {
                expr: Func::coalesce([
                    Expr::col((metadata_item::Entity, metadata_item::Column::AudienceRating))
                        .into(),
                    Expr::val(0.0).into(),
                ])
                .into(),
                descending: true,
                nullable: false,
            });
        }
        terms.push(column(metadata_item::Column::SortKey, false));
        terms.push(column(metadata_item::Column::Id, false));
        Self { terms }
    }

    pub(crate) fn resume(
        &self,
        select: Select<metadata_item::Entity>,
        cursor: &Cursor,
        after: bool,
    ) -> Result<Select<metadata_item::Entity>, MetadataError> {
        resume_terms(&self.terms, select, cursor, after)
    }

    pub(crate) fn sort(
        &self,
        select: Select<metadata_item::Entity>,
        reversed: bool,
    ) -> Select<metadata_item::Entity> {
        sort_terms(&self.terms, select, reversed)
    }
}

fn resume_terms(
    terms: &[Term],
    select: Select<metadata_item::Entity>,
    cursor: &Cursor,
    after: bool,
) -> Result<Select<metadata_item::Entity>, MetadataError> {
    // Every term but the id carries a value: a cursor from another ordering cannot say
    // where this one stands.
    if cursor.keys.len() + 1 != terms.len() {
        return Err(MetadataError::CursorMismatch);
    }
    let values: Vec<CursorKey> = cursor
        .keys
        .iter()
        .cloned()
        .chain([CursorKey::Int(cursor.id)])
        .collect();
    if values
        .iter()
        .zip(terms)
        .any(|(value, term)| matches!(value, CursorKey::Absent) && !term.nullable)
    {
        return Err(MetadataError::CursorMismatch);
    }
    Ok(select.filter(lexicographic(terms, &values, after)))
}

fn sort_terms(
    terms: &[Term],
    mut select: Select<metadata_item::Entity>,
    reversed: bool,
) -> Select<metadata_item::Entity> {
    for term in terms {
        let descending = term.descending != reversed;
        let order = if descending { Order::Desc } else { Order::Asc };
        select = if term.nullable {
            // Smallest: first ascending, last descending.
            let nulls = if descending {
                NullOrdering::Last
            } else {
                NullOrdering::First
            };
            select.order_by_with_nulls(term.expr.clone(), order, nulls)
        } else {
            select.order_by(term.expr.clone(), order)
        };
    }
    select
}

/// An item's shortest or longest version by `column`: the shortest leads an ascending
/// listing, the longest a descending one.
///
/// The reference joins an item to each of its versions and keeps the first row, so an item
/// with a 1038-line and a 1040-line version ranks as 1038 ascending and as 1040 descending.
fn version_extreme(column: media::Column, direction: Direction) -> Leading {
    let value = Expr::col((media::Entity, column));
    let extreme = if direction.is_descending() {
        Func::max(value)
    } else {
        Func::min(value)
    };
    Leading {
        expr: scalar(
            Query::select()
                .expr(extreme)
                .from(media::Entity)
                .and_where(
                    Expr::col((media::Entity, media::Column::MetadataItemId))
                        .equals((metadata_item::Entity, metadata_item::Column::Id)),
                )
                .to_owned(),
        ),
        key_type: KeyType::Int,
        nullable: true,
    }
}

/// When the first or the latest episode of the series in the row was added, by the rule of
/// [`version_extreme`]: measured on 1,980 series, an ascending listing follows the first.
fn episode_added_at(direction: Direction) -> SimpleExpr {
    let episode = Alias::new("episode");
    let season = Alias::new("season");
    let seasons = Query::select()
        .column((season.clone(), metadata_item::Column::Id))
        .from_as(metadata_item::Entity, season.clone())
        .and_where(
            Expr::col((season, metadata_item::Column::ParentId))
                .equals((metadata_item::Entity, metadata_item::Column::Id)),
        )
        .to_owned();
    let added_at = Expr::col((episode.clone(), metadata_item::Column::AddedAt));
    let extreme = if direction.is_descending() {
        Func::max(added_at)
    } else {
        Func::min(added_at)
    };
    scalar(
        Query::select()
            .expr(extreme)
            .from_as(metadata_item::Entity, episode.clone())
            .and_where(Expr::col((episode, metadata_item::Column::ParentId)).in_subquery(seasons))
            .to_owned(),
    )
}

/// The position of the row's library in `sections`.
///
/// A `CASE` over the list the caller gave rather than a join on the library's title: which
/// order libraries come in is the caller's choice, and a list of a dozen libraries makes a
/// short expression.
fn library_rank(sections: &[open_plex_core::SectionId]) -> SimpleExpr {
    let mut rank = CaseStatement::new();
    for (position, section) in (0_i64..).zip(sections) {
        rank = rank.case(
            Expr::col((
                metadata_item::Entity,
                metadata_item::Column::LibrarySectionId,
            ))
            .eq(section.get()),
            Expr::val(position),
        );
    }
    rank.finally(Expr::val(i64::try_from(sections.len()).unwrap_or(i64::MAX)))
        .into()
}

/// The values [`ItemSort::Episodes`] leads with: the series' sort key, the season's number, the
/// episode's, its release date.
///
/// Only an episode has them; anything else reads its own sort key and nothing after, so that
/// it falls into title order. An absent number or date reads as [`ABSENT_LAST`] rather than
/// `NULL`, because absence sorts last here and first everywhere else.
fn episode_keys() -> Vec<Leading> {
    let season = Alias::new("season");
    let series = Alias::new("series");
    let own = |column: metadata_item::Column| Expr::col((metadata_item::Entity, column));
    let is_episode = || own(metadata_item::Column::Kind).eq(kind_name(ItemKind::Episode));
    let of_season = |column: metadata_item::Column| {
        scalar(
            Query::select()
                .column((season.clone(), column))
                .from_as(metadata_item::Entity, season.clone())
                .and_where(
                    Expr::col((season.clone(), metadata_item::Column::Id))
                        .equals((metadata_item::Entity, metadata_item::Column::ParentId)),
                )
                .to_owned(),
        )
    };
    let series_key = scalar(
        Query::select()
            .column((series.clone(), metadata_item::Column::SortKey))
            .from_as(metadata_item::Entity, series.clone())
            .join_as(
                JoinType::InnerJoin,
                metadata_item::Entity,
                season.clone(),
                Expr::col((season.clone(), metadata_item::Column::ParentId))
                    .equals((series.clone(), metadata_item::Column::Id)),
            )
            .and_where(
                Expr::col((season.clone(), metadata_item::Column::Id))
                    .equals((metadata_item::Entity, metadata_item::Column::ParentId)),
            )
            .to_owned(),
    );
    let last_when_absent = |value: SimpleExpr| -> SimpleExpr {
        CaseStatement::new()
            .case(
                is_episode(),
                Func::coalesce([value, Expr::val(ABSENT_LAST).into()]),
            )
            .finally(Expr::val(ABSENT_LAST))
            .into()
    };
    vec![
        Leading {
            expr: CaseStatement::new()
                .case(is_episode(), series_key)
                .finally(own(metadata_item::Column::SortKey))
                .into(),
            key_type: KeyType::Text,
            nullable: false,
        },
        Leading {
            expr: last_when_absent(of_season(metadata_item::Column::Position)),
            key_type: KeyType::Int,
            nullable: false,
        },
        Leading {
            expr: last_when_absent(own(metadata_item::Column::Position).into()),
            key_type: KeyType::Int,
            nullable: false,
        },
        Leading {
            expr: last_when_absent(own(metadata_item::Column::ReleasedOn).into()),
            key_type: KeyType::Int,
            nullable: false,
        },
    ]
}

/// The sort key, or an empty one for a series, so that series tie and fall to their ids.
fn title_except_series() -> SimpleExpr {
    CaseStatement::new()
        .case(
            Expr::col((metadata_item::Entity, metadata_item::Column::Kind))
                .eq(kind_name(ItemKind::Show)),
            Expr::val(""),
        )
        .finally(Expr::col((
            metadata_item::Entity,
            metadata_item::Column::SortKey,
        )))
        .into()
}

/// A pseudo-random rank for each id, fixed by the seed.
///
/// Arithmetic rather than the engine's `RANDOM()`, which is neither portable nor
/// repeatable: the same seed must give the same order, or a second page would not follow
/// the first. The seed is mixed first, so that neighbouring seeds share nothing, and the
/// square breaks the linearity that would make every order a rotation of one sequence.
/// Products stay below 2⁶³: ids and factors are below 2³¹.
fn shuffled(seed: u32) -> SimpleExpr {
    let mixed = splitmix64(u64::from(seed));
    // Both below the modulus, and the factor never zero.
    let factor = i64::try_from(mixed % 2_147_483_646).unwrap_or(0) + 1;
    let offset = i64::try_from((mixed >> 32) % 2_147_483_647).unwrap_or(0);
    let id = Expr::col((metadata_item::Entity, metadata_item::Column::Id));
    let spread = Expr::expr(id.mul(factor).add(offset)).modulo(SHUFFLE_MODULUS);
    Expr::expr(Expr::expr(spread.clone()).mul(spread)).modulo(SHUFFLE_MODULUS)
}

/// Sebastiano Vigna's `SplitMix64` finalizer: every input bit reaches every output bit.
const fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn scalar(query: sea_orm::sea_query::SelectStatement) -> SimpleExpr {
    SimpleExpr::SubQuery(None, Box::new(query.into_sub_query_statement()))
}

/// `(t₀, t₁, …) > (v₀, v₁, …)` written out, since SQL has no portable row comparison: the
/// first term past its value, or equal on it and the rest past theirs.
fn lexicographic(terms: &[Term], values: &[CursorKey], after: bool) -> Condition {
    let mut any = Condition::any();
    for (position, (term, value)) in terms.iter().zip(values).enumerate() {
        let Some(past) = past(term, value, after) else {
            continue;
        };
        let mut all = Condition::all();
        for (earlier, earlier_value) in terms.iter().zip(values).take(position) {
            all = all.add(equal(earlier, earlier_value));
        }
        any = any.add(all.add(past));
    }
    any
}

fn bind(value: &CursorKey) -> SimpleExpr {
    match value {
        CursorKey::Text(text) => Expr::value(text.clone()),
        CursorKey::Int(number) => Expr::value(*number),
        CursorKey::Real(number) => Expr::value(*number),
        CursorKey::Absent => Expr::value(Option::<i64>::None),
    }
}

fn equal(term: &Term, value: &CursorKey) -> SimpleExpr {
    let expr = Expr::expr(term.expr.clone());
    match value {
        CursorKey::Absent => expr.is_null(),
        value => expr.eq(bind(value)),
    }
}

/// The rows past `value` on this term, or `None` when there are none: nothing sorts before
/// an absent value.
fn past(term: &Term, value: &CursorKey, after: bool) -> Option<SimpleExpr> {
    let expr = Expr::expr(term.expr.clone());
    let greater = term.descending != after;
    match (value, greater) {
        (CursorKey::Absent, true) => Some(expr.is_not_null()),
        (CursorKey::Absent, false) => None,
        (value, true) => Some(expr.gt(bind(value))),
        (value, false) if term.nullable => Some(
            Expr::expr(term.expr.clone())
                .lt(bind(value))
                .or(expr.is_null()),
        ),
        (value, false) => Some(expr.lt(bind(value))),
    }
}
