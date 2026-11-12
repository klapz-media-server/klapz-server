//! Ranking titles by distance to a term without reading the library.
//!
//! A distance is at least the difference of the two lengths, so titles are read in bands of
//! length around the term's — the same length, then one more or less, then two to three,
//! four to seven, doubling — and the search stops once the titles kept are closer than any
//! band left could hold. `dune` with the reference's default of three reads the 1,608 films
//! and series of three to five units, of 22,337, and stops there.
//!
//! A length is a weak bound once the closest titles are a few edits away, because titles
//! cluster around a dozen units: `breaking bad` reads 7,928 of them, `kaamelott`, whose
//! third match is 5 away, 13,745, and a term near nothing 19,891. What keeps that cheap is
//! what is read: one covering index range per kind (`m0032_folded_title`), the id and the
//! folded title of each, measured by one pass of a bit-parallel distance — the items
//! themselves are loaded for the page alone. The reference reads every item of the library
//! instead, ranking them in SQL (`ORDER BY LEVENSHTEIN(…) LIMIT n`).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use open_plex_core::closest::{self, ClosestTitles, Distance};
use open_plex_core::metadata::MetadataError;
use open_plex_core::page::{Cursor, CursorKey, Page, Window};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, QueryFilter, QuerySelect,
};

use crate::entity::item_folded_title;
use crate::metadata_repository::{kind_name, storage};

/// One title's place in the ranking.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ranked {
    pub distance: u32,
    pub key: String,
    pub id: i64,
}

impl Ranked {
    fn from_cursor(cursor: &Cursor) -> Result<Self, MetadataError> {
        let [CursorKey::Int(distance), CursorKey::Text(key)] = cursor.keys.as_slice() else {
            return Err(MetadataError::CursorMismatch);
        };
        Ok(Self {
            distance: u32::try_from(*distance).map_err(|_| MetadataError::CursorMismatch)?,
            key: key.clone(),
            id: cursor.id,
        })
    }
}

#[derive(Debug, FromQueryResult)]
struct FoldedRow {
    item_id: i64,
    folded_title: String,
}

/// The ranking's page, in reading order: who is in it and at what distance.
///
/// # Errors
///
/// Fails on a cursor of another ordering, or when storage fails.
pub async fn rank<C: ConnectionTrait>(
    connection: &C,
    query: &ClosestTitles,
    window: &Window,
) -> Result<Page<Ranked>, MetadataError> {
    let after = window.after.as_ref().map(Ranked::from_cursor).transpose()?;
    let before = window
        .before
        .as_ref()
        .map(Ranked::from_cursor)
        .transpose()?;
    let distance = Distance::new(&closest::term_key(&query.term));
    let needed =
        usize::try_from(window.skip.saturating_add(window.fetch_limit())).unwrap_or(usize::MAX);
    let mut kept = Kept::new(window.is_reversed(), needed);

    if !query.kinds.is_empty() {
        let length = i64::try_from(distance.length()).unwrap_or(i64::MAX);
        // Nothing before the `before` cursor is farther than it, nor so much longer.
        let reach = before.as_ref().map(|before| i64::from(before.distance));
        for (nearest, farthest) in bands() {
            if reach.is_some_and(|reach| nearest > reach) || kept.closer_than(nearest) {
                break;
            }
            let farthest = match (farthest, reach) {
                (Some(farthest), Some(reach)) => Some(farthest.min(reach)),
                (farthest, reach) => farthest.or(reach),
            };
            let rows = band(connection, query, length, nearest, farthest).await?;
            for row in rows {
                let ranked = Ranked {
                    distance: distance.to(&row.folded_title),
                    key: row.folded_title,
                    id: row.item_id,
                };
                if after.as_ref().is_some_and(|after| ranked <= *after)
                    || before.as_ref().is_some_and(|before| ranked >= *before)
                {
                    continue;
                }
                kept.offer(ranked);
            }
            if farthest.is_none() {
                break;
            }
        }
    }

    let mut ordered = kept.into_reading_order();
    ordered.drain(
        ..usize::try_from(window.skip)
            .unwrap_or(usize::MAX)
            .min(ordered.len()),
    );
    let mut page = Page::from_overfetch(ordered, window);
    if window.is_reversed() {
        page.items.reverse();
    }
    Ok(page)
}

/// The bands of length difference read in turn, as inclusive ranges: 0, 1, 2–3, 4–7… to
/// 64–127, then everything farther at once. `None` is no upper bound.
fn bands() -> impl Iterator<Item = (i64, Option<i64>)> {
    let mut next = Some((0, Some(0)));
    std::iter::from_fn(move || {
        let current = next?;
        next = match current {
            (_, None) => None,
            (_, Some(farthest)) if farthest >= 64 => Some((farthest + 1, None)),
            (_, Some(farthest)) => Some((farthest + 1, Some(2 * farthest + 1))),
        };
        Some(current)
    })
}

/// The titles whose length differs from `length` by `nearest` to `farthest` units.
///
/// One query per side rather than an `OR` of both: `SQLite` seeks a single range of the
/// index, and scans the whole kind for the two together.
async fn band<C: ConnectionTrait>(
    connection: &C,
    query: &ClosestTitles,
    length: i64,
    nearest: i64,
    farthest: Option<i64>,
) -> Result<Vec<FoldedRow>, MetadataError> {
    let shorter = (length - nearest >= 0).then(|| {
        let lowest = farthest.map_or(0, |farthest| (length - farthest).max(0));
        (lowest, Some(length - nearest))
    });
    // The term's own length is the band of no difference, read once.
    let longer = (nearest > 0).then_some((length + nearest, farthest.map(|far| length + far)));
    let mut rows = Vec::new();
    for (lowest, highest) in shorter.into_iter().chain(longer) {
        rows.extend(lengths_between(connection, query, lowest, highest).await?);
    }
    Ok(rows)
}

/// The titles from `lowest` to `highest` units long, both included; `None` has no ceiling.
async fn lengths_between<C: ConnectionTrait>(
    connection: &C,
    query: &ClosestTitles,
    lowest: i64,
    highest: Option<i64>,
) -> Result<Vec<FoldedRow>, MetadataError> {
    let column = || Expr::col(item_folded_title::Column::FoldedLength);
    let mut select = item_folded_title::Entity::find()
        .select_only()
        .column(item_folded_title::Column::ItemId)
        .column(item_folded_title::Column::FoldedTitle)
        .filter(
            item_folded_title::Column::Kind.is_in(query.kinds.iter().map(|kind| kind_name(*kind))),
        )
        .filter(column().gte(lowest));
    if let Some(highest) = highest {
        select = select.filter(column().lte(highest));
    }
    if let Some(sections) = &query.sections {
        select = select.filter(
            item_folded_title::Column::LibrarySectionId
                .is_in(sections.iter().map(|section| section.get())),
        );
    }
    select
        .into_model::<FoldedRow>()
        .all(connection)
        .await
        .map_err(storage)
}

/// The best `needed` titles seen so far: the closest for a page read forward, the farthest
/// for one read backward.
#[derive(Debug)]
enum Kept {
    Closest {
        needed: usize,
        heap: BinaryHeap<Ranked>,
    },
    Farthest {
        needed: usize,
        heap: BinaryHeap<Reverse<Ranked>>,
    },
}

impl Kept {
    fn new(reversed: bool, needed: usize) -> Self {
        if reversed {
            Self::Farthest {
                needed,
                heap: BinaryHeap::new(),
            }
        } else {
            Self::Closest {
                needed,
                heap: BinaryHeap::new(),
            }
        }
    }

    /// Whether every title kept is closer than a title `difference` units longer or shorter
    /// could be, so that no band from there on can change the page. Never for a page read
    /// backward, which wants the farthest.
    fn closer_than(&self, difference: i64) -> bool {
        match self {
            Self::Closest { needed, heap } => {
                heap.len() >= *needed
                    && heap
                        .peek()
                        .is_some_and(|worst| i64::from(worst.distance) < difference)
            }
            Self::Farthest { .. } => false,
        }
    }

    fn offer(&mut self, ranked: Ranked) {
        match self {
            Self::Closest { needed, heap } => {
                heap.push(ranked);
                if heap.len() > *needed {
                    heap.pop();
                }
            }
            Self::Farthest { needed, heap } => {
                heap.push(Reverse(ranked));
                if heap.len() > *needed {
                    heap.pop();
                }
            }
        }
    }

    /// Closest first for a page read forward, farthest first for one read backward, as
    /// [`Page::from_overfetch`] expects them.
    fn into_reading_order(self) -> Vec<Ranked> {
        match self {
            Self::Closest { heap, .. } => heap.into_sorted_vec(),
            Self::Farthest { heap, .. } => heap
                .into_sorted_vec()
                .into_iter()
                .map(|Reverse(ranked)| ranked)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_double_then_open() {
        let bands: Vec<_> = bands().collect();
        assert_eq!(
            bands,
            [
                (0, Some(0)),
                (1, Some(1)),
                (2, Some(3)),
                (4, Some(7)),
                (8, Some(15)),
                (16, Some(31)),
                (32, Some(63)),
                (64, Some(127)),
                (128, None),
            ]
        );
    }
}
