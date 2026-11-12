//! Windows into ordered collections.
//!
//! Every collection the API exposes is paginated, so storage is never asked for everything.
//! That is a correctness requirement rather than a nicety: a library holds hundreds of
//! thousands of metadata items, and a port returning a `Vec` would load all of them to serve
//! one page.
//!
//! The vocabulary is Relay's, because the official API is Relay-compliant and translating
//! between two pagination models at every layer would be pure friction. Nothing here depends
//! on GraphQL.

/// How many items a window may request.
///
/// A client-controlled count needs a server-side ceiling, or one request can exhaust the
/// machine. The value is generous enough that no legitimate page reaches it.
pub const MAX_ITEMS: u32 = 500;

#[derive(Debug, thiserror::Error)]
pub enum WindowError {
    #[error("a page must be bounded: pass either `first` or `last`")]
    Unbounded,
    #[error("`first` and `last` cannot both be given")]
    Ambiguous,
    #[error("a page of {requested} exceeds the maximum of {MAX_ITEMS}")]
    TooLarge { requested: u32 },
}

/// Which end of the range the page is taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// The first `n` items of the range.
    First(u32),
    /// The last `n` items of the range.
    Last(u32),
}

impl Bound {
    pub const fn limit(self) -> u32 {
        match self {
            Self::First(n) | Self::Last(n) => n,
        }
    }
}

/// One ordering value at a cursor's position.
///
/// A collection ordered by anything other than its primary key needs that value to resume
/// from: paginating a title-sorted list by id alone would skip or repeat rows.
#[derive(Debug, Clone, PartialEq)]
pub enum CursorKey {
    Text(String),
    Int(i64),
    Real(f64),
    /// The ordering value is missing here: a film with no rating, sorted by rating. Absence
    /// sorts before every value, as `NULL` does on `SQLite` and on the reference server.
    Absent,
}

/// A position in an ordered collection.
///
/// `keys` are the ordering values, most significant first, and `id` breaks the last tie,
/// which is what makes the cursor stable: two films sharing a sort title still have a total
/// order, so no row is skipped or repeated across pages. No keys means the collection is
/// ordered by id alone — the case for library sections.
#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    pub keys: Vec<CursorKey>,
    pub id: i64,
}

impl Cursor {
    pub const fn by_id(id: i64) -> Self {
        Self {
            keys: Vec::new(),
            id,
        }
    }

    pub fn new(key: CursorKey, id: i64) -> Self {
        Self {
            keys: vec![key],
            id,
        }
    }
}

/// Ascending or descending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Ascending,
    Descending,
}

impl Direction {
    pub const fn is_descending(self) -> bool {
        matches!(self, Self::Descending)
    }
}

/// A requested slice of an ordered collection.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub after: Option<Cursor>,
    pub before: Option<Cursor>,
    pub bound: Bound,
    /// Rows to discard before the page begins.
    ///
    /// Present for one reason: Plex clients page by absolute offset
    /// (`X-Plex-Container-Start`), and no cursor can express "the 9,000th row" — that is
    /// exactly the position a keyset cursor avoids needing to know. Cursors remain the way
    /// to walk a collection; this is how a caller lands in the middle of one.
    ///
    /// Costly by nature: the database must still traverse the discarded rows. Bounded by the
    /// size of the collection rather than by client input, which is why it needs no cap of
    /// its own.
    pub skip: u64,
}

impl Window {
    /// Builds a window from Relay's four arguments.
    ///
    /// Requiring a bound is deliberate. Relay permits an unbounded connection, but a server
    /// honoring that has no cap on result size, so a page size is mandatory here and the API
    /// supplies a default when the client omits one.
    ///
    /// # Errors
    ///
    /// Fails when neither bound is given, when both are, or when the count exceeds
    /// [`MAX_ITEMS`].
    pub fn new(
        after: Option<Cursor>,
        before: Option<Cursor>,
        first: Option<u32>,
        last: Option<u32>,
    ) -> Result<Self, WindowError> {
        let bound = match (first, last) {
            (Some(_), Some(_)) => return Err(WindowError::Ambiguous),
            (Some(n), None) => Bound::First(n),
            (None, Some(n)) => Bound::Last(n),
            (None, None) => return Err(WindowError::Unbounded),
        };

        if bound.limit() > MAX_ITEMS {
            return Err(WindowError::TooLarge {
                requested: bound.limit(),
            });
        }

        Ok(Self {
            after,
            before,
            bound,
            skip: 0,
        })
    }

    /// The first `count` rows, at most [`MAX_ITEMS`]: for a caller in `core` reading a
    /// bounded list, which has no client input to refuse.
    pub fn first(count: u32) -> Self {
        Self {
            after: None,
            before: None,
            bound: Bound::First(count.min(MAX_ITEMS)),
            skip: 0,
        }
    }

    /// Discards `rows` before the page begins. See [`Window::skip`].
    #[must_use]
    pub const fn skipping(mut self, rows: u64) -> Self {
        self.skip = rows;
        self
    }

    /// A window over a collection ordered by id, which is the common case.
    ///
    /// # Errors
    ///
    /// Same as [`Window::new`].
    pub fn by_id(
        after: Option<i64>,
        before: Option<i64>,
        first: Option<u32>,
        last: Option<u32>,
    ) -> Result<Self, WindowError> {
        Self::new(
            after.map(Cursor::by_id),
            before.map(Cursor::by_id),
            first,
            last,
        )
    }

    /// How many rows storage should read to answer this window.
    ///
    /// One more than asked for: the extra row is how we learn whether a further page exists
    /// without running a second counting query.
    pub const fn fetch_limit(&self) -> u64 {
        self.bound.limit() as u64 + 1
    }

    /// Whether storage should read the range in reverse.
    pub const fn is_reversed(&self) -> bool {
        matches!(self.bound, Bound::Last(_))
    }
}

/// One page of results, with the flags a connection needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub has_previous: bool,
    pub has_next: bool,
}

impl<T> Page<T> {
    /// Trims an over-fetched result and derives the boundary flags.
    ///
    /// `rows` is expected to hold up to [`Window::fetch_limit`] items, already ordered as the
    /// caller wants to read them. The presence of the extra row tells us a further page
    /// exists; the opposite boundary is known from the cursor the client supplied.
    pub fn from_overfetch(mut rows: Vec<T>, window: &Window) -> Self {
        let limit = window.bound.limit() as usize;
        let overfetched = rows.len() > limit;
        rows.truncate(limit);

        match window.bound {
            Bound::First(_) => Self {
                items: rows,
                // A skipped page has rows before it by construction, cursor or no cursor.
                has_previous: window.after.is_some() || window.skip > 0,
                has_next: overfetched,
            },
            Bound::Last(_) => Self {
                items: rows,
                has_previous: overfetched,
                has_next: window.before.is_some(),
            },
        }
    }

    pub fn map<U>(self, transform: impl FnMut(T) -> U) -> Page<U> {
        Page {
            items: self.items.into_iter().map(transform).collect(),
            has_previous: self.has_previous,
            has_next: self.has_next,
        }
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn forward(n: u32) -> Window {
        Window::by_id(None, None, Some(n), None).expect("valid")
    }

    #[test]
    fn requires_exactly_one_bound() {
        assert!(matches!(
            Window::by_id(None, None, None, None),
            Err(WindowError::Unbounded)
        ));
        assert!(matches!(
            Window::by_id(None, None, Some(10), Some(10)),
            Err(WindowError::Ambiguous)
        ));
    }

    #[test]
    fn caps_the_page_size() {
        // Without a ceiling, one request can make the server read an entire library.
        assert!(matches!(
            Window::by_id(None, None, Some(MAX_ITEMS + 1), None),
            Err(WindowError::TooLarge { requested }) if requested == MAX_ITEMS + 1
        ));
        assert!(Window::by_id(None, None, Some(MAX_ITEMS), None).is_ok());
    }

    #[test]
    fn over_fetches_by_one_to_detect_a_further_page() {
        assert_eq!(forward(2).fetch_limit(), 3);
    }

    #[test]
    fn forward_page_reports_a_next_page_from_the_extra_row() {
        let page = Page::from_overfetch(vec![1, 2, 3], &forward(2));
        assert_eq!(page.items, vec![1, 2]);
        assert!(page.has_next);
        assert!(!page.has_previous);
    }

    #[test]
    fn forward_page_without_an_extra_row_is_the_last() {
        let window = Window::by_id(Some(7), None, Some(2), None).expect("valid");
        let page = Page::from_overfetch(vec![8, 9], &window);
        assert!(!page.has_next);
        // A cursor was supplied, so items precede this page by definition.
        assert!(page.has_previous);
    }

    #[test]
    fn backward_page_reports_a_previous_page_from_the_extra_row() {
        let window = Window::by_id(None, None, None, Some(2)).expect("valid");
        assert!(window.is_reversed());
        let page = Page::from_overfetch(vec![9, 8, 7], &window);
        assert_eq!(page.items, vec![9, 8]);
        assert!(page.has_previous);
        assert!(!page.has_next);
    }

    #[test]
    fn a_window_ordered_by_id_carries_no_sort_key() {
        let window = Window::by_id(Some(4), None, Some(2), None).expect("valid");
        assert_eq!(window.after, Some(Cursor::by_id(4)));
        assert!(window.after.expect("present").keys.is_empty());
    }

    #[test]
    fn a_sorted_window_carries_the_key_and_the_tiebreaker() {
        // Two films can share a sort title, so the id is what gives the ordering a total
        // order and keeps a row from being skipped between pages.
        let cursor = Cursor::new(CursorKey::Text("Avatar".to_owned()), 407_618);
        let window = Window::new(Some(cursor.clone()), None, Some(10), None).expect("valid");
        assert_eq!(window.after, Some(cursor));
    }

    #[test]
    fn a_skipped_page_has_a_previous_page_without_any_cursor() {
        // Offset paging carries no cursor, so the flag can only come from the offset.
        let window = forward(2).skipping(50);
        let page = Page::from_overfetch(vec![1, 2, 3], &window);
        assert!(page.has_previous);
        assert!(page.has_next);
        assert!(!Page::from_overfetch(vec![1, 2, 3], &forward(2)).has_previous);
    }

    #[test]
    fn maps_items_while_preserving_boundaries() {
        let page = Page::from_overfetch(vec![1, 2, 3], &forward(2)).map(|n| n * 10);
        assert_eq!(page.items, vec![10, 20]);
        assert!(page.has_next);
    }
}
