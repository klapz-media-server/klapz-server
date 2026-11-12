//! `closestTitles`: items ranked by how far their title is from a term.

use async_graphql::connection::{query, Connection, Edge};
use async_graphql::{Context, Object, Result};
use open_plex_core::closest::{ClosestTitles, TitleMatch, MAX_TERM_BYTES};

use crate::cursor::OpaqueCursor;
use crate::filter::ItemType;
use crate::item::Item;

/// An item and the distance of its title to the term searched.
#[derive(Debug, Clone)]
pub struct ClosestTitleObject(pub TitleMatch);

#[Object(name = "ClosestTitle")]
impl ClosestTitleObject {
    /// The item.
    async fn item(&self) -> Item {
        Item::from_domain(self.0.item.clone())
    }

    /// How many edits — a character inserted, removed or replaced — turn the term into the
    /// item's title, both read as `closestTitles` reads them. 0 is the title itself.
    async fn distance(&self) -> u32 {
        self.0.distance
    }
}

/// A page of items closest to a term.
pub type ClosestTitleConnection = Connection<OpaqueCursor, ClosestTitleObject>;

/// The items whose title is closest to a term, among the libraries the caller can see.
pub(crate) async fn closest_titles(
    context: &Context<'_>,
    term: String,
    kinds: Vec<ItemType>,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<ClosestTitleConnection> {
    if term.len() > MAX_TERM_BYTES {
        return Err(format!("a term is at most {MAX_TERM_BYTES} bytes long").into());
    }
    let search = ClosestTitles {
        term,
        kinds: kinds.iter().filter_map(|kind| kind.held()).collect(),
        sections: crate::access::visible_sections(context),
    };
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let window = crate::schema::window_for(
                after,
                before,
                first,
                last,
                crate::schema::DEFAULT_PAGE_SIZE,
            )?;
            let page = crate::schema::metadata(context)?
                .closest_titles(&search, window)
                .await?;
            let mut connection = Connection::new(page.has_previous, page.has_next);
            connection.edges.extend(
                page.items.into_iter().map(|found| {
                    Edge::new(OpaqueCursor(found.cursor()), ClosestTitleObject(found))
                }),
            );
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}
