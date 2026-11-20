//! Searching across libraries: items by title, ranked, and items found through the people a
//! term names.

use async_graphql::connection::{self, Connection, ConnectionNameType, Edge, EdgeNameType};
use async_graphql::{Context, Enum, OutputType, Result, SimpleObject};
use klapz_core::credited::search_credits;
use klapz_core::metadata::{SearchOrder, SearchQuery};
use klapz_core::page::{Cursor, MAX_ITEMS};
use klapz_core::search::{self, Rung};

use crate::cursor::OpaqueCursor;
use crate::filter::ItemType;
use crate::item::Item;
use crate::schema::{self, window_for, DEFAULT_PAGE_SIZE};
use crate::tag::TagUseObject;

/// How a search orders what it found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum, Default)]
pub enum SearchOrderInput {
    /// The best match first: a title equal to the term, then one whose first word is the
    /// term's, then one starting with it, then the rest; the audience rating breaks ties
    /// within each, then the sort title.
    #[default]
    Relevance,
    /// By sort title.
    Title,
}

impl From<SearchOrderInput> for SearchOrder {
    fn from(order: SearchOrderInput) -> Self {
        match order {
            SearchOrderInput::Relevance => Self::Relevance,
            SearchOrderInput::Title => Self::Title,
        }
    }
}

/// How well an item's title answers a search term, from the best.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum TitleMatch {
    /// The title or the sort title is the term, accents and case aside.
    Exact,
    /// The first word of the title or of the original title is the term's first word. A
    /// word ends at a space or an apostrophe here, so `Amelie's Miracle` opens with `amelie`.
    LeadingWord,
    /// The title or the original title begins with the term's first word.
    LeadingPrefix,
    /// Some word of a title begins with each word of the term.
    Anywhere,
}

impl From<Rung> for TitleMatch {
    fn from(rung: Rung) -> Self {
        match rung {
            Rung::Exact => Self::Exact,
            Rung::LeadingWord => Self::LeadingWord,
            Rung::LeadingPrefix => Self::LeadingPrefix,
            Rung::Anywhere => Self::Anywhere,
        }
    }
}

/// What a search says of each item it found.
#[derive(Debug, Clone, SimpleObject)]
pub struct SearchHit {
    /// How relevant the item is, between 0 and 1: two tenths for each step of `titleMatch`
    /// from 0.33 (0.31 for an episode), plus a ten-thousandth per point of audience rating.
    /// The Plex server's own scale.
    pub score: f64,
    /// How well the title answers the term.
    pub title_match: TitleMatch,
}

#[derive(Debug)]
pub struct ItemSearchConnectionName;

impl ConnectionNameType for ItemSearchConnectionName {
    fn type_name<T: OutputType>() -> String {
        "ItemSearchConnection".to_owned()
    }
}

#[derive(Debug)]
pub struct ItemSearchEdgeName;

impl EdgeNameType for ItemSearchEdgeName {
    fn type_name<T: OutputType>() -> String {
        "ItemSearchEdge".to_owned()
    }
}

/// A page of a search, each item with its score.
pub type ItemSearchConnection = Connection<
    OpaqueCursor,
    Item,
    connection::EmptyFields,
    SearchHit,
    ItemSearchConnectionName,
    ItemSearchEdgeName,
>;

/// Items whose title matches a term, across the libraries the caller can see.
pub(crate) async fn search(
    context: &Context<'_>,
    term: String,
    kinds: Option<Vec<ItemType>>,
    (order, episodes_by_series): (SearchOrderInput, bool),
    (after, before, first, last): (Option<String>, Option<String>, Option<i32>, Option<i32>),
) -> Result<ItemSearchConnection> {
    let mut search = SearchQuery::new(term);
    search.sections = crate::access::visible_sections(context);
    search.order = order.into();
    search.episodes_by_series = episodes_by_series;
    let kinds = kinds.unwrap_or_default();
    search.kinds = kinds.iter().filter_map(|kind| kind.held()).collect();
    // Every kind asked for is one this server holds none of. Not passed on as an empty
    // list, which storage reads as every kind.
    if !kinds.is_empty() && search.kinds.is_empty() {
        return Ok(Connection::new(false, false));
    }

    connection::query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let window = window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?;
            let page = schema::metadata(context)?.search(&search, window).await?;

            let mut connection = Connection::new(page.has_previous, page.has_next);
            connection.edges.extend(page.items.into_iter().map(|item| {
                let rung = search.rung(&item);
                let hit = SearchHit {
                    score: search::score(item.kind, rung, item.audience_rating),
                    title_match: rung.into(),
                };
                Edge::with_additional_fields(
                    OpaqueCursor(search.cursor(&item)),
                    Item::from_domain(item),
                    hit,
                )
            }));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// What a search through credits says of each item it found.
#[derive(Debug, Clone, SimpleObject)]
pub struct CreditedHit {
    /// How relevant the item is, on the scale of `ItemSearchEdge.score`: a title matched
    /// nowhere in particular, 0.33 plus a ten-thousandth per point of audience rating.
    pub score: f64,
    /// The person the item was found through, in the library it was found in.
    pub credit: TagUseObject,
}

#[derive(Debug)]
pub struct CreditedItemConnectionName;

impl ConnectionNameType for CreditedItemConnectionName {
    fn type_name<T: OutputType>() -> String {
        "CreditedItemConnection".to_owned()
    }
}

#[derive(Debug)]
pub struct CreditedItemEdgeName;

impl EdgeNameType for CreditedItemEdgeName {
    fn type_name<T: OutputType>() -> String {
        "CreditedItemEdge".to_owned()
    }
}

/// Items found through credits, each with the credit and its score.
pub type CreditedItemConnection = Connection<
    OpaqueCursor,
    Item,
    connection::EmptyFields,
    CreditedHit,
    CreditedItemConnectionName,
    CreditedItemEdgeName,
>;

/// Films or series found through the people a term names. See `Query.searchCredits`.
pub(crate) async fn credited(
    context: &Context<'_>,
    term: String,
    kind: ItemType,
    first: Option<i32>,
) -> Result<CreditedItemConnection> {
    let limit = match first {
        None => DEFAULT_PAGE_SIZE,
        Some(count) => u32::try_from(count)
            .ok()
            .filter(|count| *count <= MAX_ITEMS)
            .ok_or_else(|| {
                async_graphql::Error::new(format!("first must lie between 0 and {MAX_ITEMS}"))
            })?,
    };
    let Some(kind) = kind.held() else {
        return Ok(Connection::new(false, false));
    };
    let libraries = crate::tag::libraries_in_view(context).await?;
    let found = search_credits(
        schema::metadata(context)?.as_ref(),
        &term,
        kind,
        &libraries,
        crate::access::viewer_id(context),
        limit,
    )
    .await?;

    let mut connection = Connection::new(false, false);
    connection
        .edges
        .extend(found.into_iter().enumerate().map(|(position, found)| {
            Edge::with_additional_fields(
                OpaqueCursor(Cursor::by_id(i64::try_from(position).unwrap_or(i64::MAX))),
                Item::from_domain(found.item),
                CreditedHit {
                    score: found.score,
                    credit: TagUseObject(found.credit),
                },
            )
        }));
    Ok(connection)
}
