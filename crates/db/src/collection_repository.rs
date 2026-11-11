//! Stored collections.
//!
//! Every read of an entry joins it to its item, so an entry whose item is gone is hidden. A
//! collection lies in one library, so who may read it is its library's question, answered
//! above storage.

use open_plex_core::collection::{
    new_guid, Collection, CollectionEdit, CollectionEntry, CollectionError, CollectionField,
    CollectionMode, CollectionOrder, CollectionQuery, CollectionRepository, CollectionSort,
    CollectionTotals, Destination, Membership, NewCollection,
};
use open_plex_core::edit::derived_title_sort;
use open_plex_core::page::{Cursor, Page, Window};
use open_plex_core::playlist::{new_entries, position_between, POSITION_GAP};
use open_plex_core::{search, CollectionId, RatingKey, SectionId};
use sea_orm::sea_query::{Expr, Query, SimpleExpr};
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, JoinType,
    Order, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, RelationTrait, Select,
    TransactionTrait,
};

use crate::entity::{collection, collection_entry, collection_word, metadata_item};
use crate::metadata_repository::{kind_name, parse_kind, prefix_condition};
use crate::playlist_repository::{compared, int_key, past, text_key, Term};
use crate::write;

/// `SQLite` refuses a statement with too many bound values.
const BATCH: usize = 200;

#[derive(Debug, Clone)]
pub struct SqlCollectionRepository {
    connection: DatabaseConnection,
}

impl SqlCollectionRepository {
    #[must_use]
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }
}

fn storage(error: DbErr) -> CollectionError {
    CollectionError::Storage(Box::new(error))
}

/// How a mode is stored.
pub(crate) const fn mode_name(mode: CollectionMode) -> &'static str {
    match mode {
        CollectionMode::LibraryDefault => "default",
        CollectionMode::Hidden => "hidden",
        CollectionMode::InsteadOfItems => "instead",
        CollectionMode::WithItems => "with",
    }
}

fn mode_of(stored: &str) -> CollectionMode {
    match stored {
        "hidden" => CollectionMode::Hidden,
        "instead" => CollectionMode::InsteadOfItems,
        "with" => CollectionMode::WithItems,
        _ => CollectionMode::LibraryDefault,
    }
}

/// How an order is stored.
pub(crate) const fn order_name(order: CollectionOrder) -> &'static str {
    match order {
        CollectionOrder::ReleaseDate => "released",
        CollectionOrder::Title => "title",
        CollectionOrder::Custom => "custom",
    }
}

fn order_of(stored: &str) -> CollectionOrder {
    match stored {
        "title" => CollectionOrder::Title,
        "custom" => CollectionOrder::Custom,
        _ => CollectionOrder::ReleaseDate,
    }
}

fn to_collection(row: collection::Model) -> Collection {
    let locked = [
        (row.title_locked, CollectionField::Title),
        (row.title_sort_locked, CollectionField::TitleSort),
        (row.summary_locked, CollectionField::Summary),
        (row.content_rating_locked, CollectionField::ContentRating),
        (row.poster_locked, CollectionField::Poster),
        (row.background_locked, CollectionField::Background),
    ]
    .into_iter()
    .filter_map(|(locked, field)| locked.then_some(field))
    .collect();
    Collection {
        id: CollectionId::new(row.id),
        section: SectionId::new(row.library_section_id),
        kind: parse_kind(&row.kind),
        guid: row.guid,
        tag_number: row.tag_number,
        title: row.title,
        title_sort: row.title_sort,
        summary: row.summary,
        content_rating: row.content_rating,
        rule: row.rule_listing,
        source_uri: row.source_uri,
        mode: mode_of(&row.mode),
        order: order_of(&row.item_order),
        locked,
        thumb_url: row.thumb_url,
        art_url: row.art_url,
        added_at: row.added_at,
        updated_at: row.updated_at,
    }
}

/// The row a collection is stored as. The sort key is `write::save_collection`'s to derive.
pub(crate) fn to_row(collection: &Collection) -> collection::Model {
    let locked = |field| collection.locked.contains(&field);
    collection::Model {
        id: collection.id.get(),
        library_section_id: collection.section.get(),
        kind: kind_name(collection.kind).to_owned(),
        guid: collection.guid.clone(),
        tag_number: collection.tag_number,
        title: collection.title.clone(),
        title_sort: collection.title_sort.clone(),
        sort_key: String::new(),
        summary: collection.summary.clone(),
        content_rating: collection.content_rating.clone(),
        rule_listing: collection.rule.clone(),
        source_uri: collection.source_uri.clone(),
        mode: mode_name(collection.mode).to_owned(),
        item_order: order_name(collection.order).to_owned(),
        title_locked: locked(CollectionField::Title),
        title_sort_locked: locked(CollectionField::TitleSort),
        summary_locked: locked(CollectionField::Summary),
        content_rating_locked: locked(CollectionField::ContentRating),
        poster_locked: locked(CollectionField::Poster),
        background_locked: locked(CollectionField::Background),
        thumb_url: collection.thumb_url.clone(),
        art_url: collection.art_url.clone(),
        added_at: collection.added_at,
        updated_at: collection.updated_at,
    }
}

fn column(column: collection::Column) -> SimpleExpr {
    Expr::col((collection::Entity, column)).into()
}

/// The order of a collection listing, and the terms a cursor is compared on.
struct ListingOrder {
    sort: CollectionSort,
    descending: bool,
}

impl ListingOrder {
    fn leading(&self) -> Option<SimpleExpr> {
        match self.sort {
            CollectionSort::Title => None,
            CollectionSort::AddedAt => Some(column(collection::Column::AddedAt)),
            CollectionSort::UpdatedAt => Some(column(collection::Column::UpdatedAt)),
        }
    }

    /// By title in the listing's direction; or by the value, then the title ascending.
    fn columns(&self) -> Vec<(SimpleExpr, bool)> {
        let title_descending = self.leading().is_none() && self.descending;
        let mut columns: Vec<(SimpleExpr, bool)> = self
            .leading()
            .map(|leading| (leading, self.descending))
            .into_iter()
            .collect();
        columns.push((column(collection::Column::SortKey), title_descending));
        columns.push((column(collection::Column::Id), title_descending));
        columns
    }

    fn terms(&self, cursor: &Cursor) -> Vec<Term> {
        let title_descending = self.leading().is_none() && self.descending;
        let mut terms = Vec::new();
        let title_at = match self.leading() {
            Some(leading) => {
                terms.push(compared(
                    &leading,
                    int_key(cursor, 0).into(),
                    self.descending,
                ));
                1
            }
            None => 0,
        };
        terms.push(compared(
            &column(collection::Column::SortKey),
            text_key(cursor, title_at).into(),
            title_descending,
        ));
        terms.push(compared(
            &column(collection::Column::Id),
            cursor.id.into(),
            title_descending,
        ));
        terms
    }
}

fn filtered(query: &CollectionQuery) -> Select<collection::Entity> {
    let mut select = collection::Entity::find().filter(
        collection::Column::LibrarySectionId.is_in(query.sections.iter().map(|id| id.get())),
    );
    if let Some(kind) = query.kind {
        select = select.filter(collection::Column::Kind.eq(kind_name(kind)));
    }
    match query.smart {
        Some(true) => select = select.filter(collection::Column::RuleListing.is_not_null()),
        Some(false) => select = select.filter(collection::Column::RuleListing.is_null()),
        None => {}
    }
    if let Some(number) = query.tag_number {
        select = select.filter(collection::Column::TagNumber.eq(number));
    }
    if query.with_entries {
        select = select.filter(
            collection::Column::Id.in_subquery(
                Query::select()
                    .column((
                        collection_entry::Entity,
                        collection_entry::Column::CollectionId,
                    ))
                    .from(collection_entry::Entity)
                    .inner_join(
                        metadata_item::Entity,
                        Expr::col((metadata_item::Entity, metadata_item::Column::Id))
                            .equals((collection_entry::Entity, collection_entry::Column::ItemId)),
                    )
                    .to_owned(),
            ),
        );
    }
    if let Some(term) = &query.title_word_prefix {
        for word in search::words(term) {
            select = select.filter(
                collection::Column::Id.in_subquery(
                    Query::select()
                        .column(collection_word::Column::CollectionId)
                        .from(collection_word::Entity)
                        .cond_where(prefix_condition(
                            Expr::col((collection_word::Entity, collection_word::Column::Word)),
                            &word,
                        ))
                        .to_owned(),
                ),
            );
        }
    }
    select
}

/// A collection's entries whose items exist.
fn present_entries(collection: CollectionId) -> Select<collection_entry::Entity> {
    collection_entry::Entity::find()
        .join(JoinType::InnerJoin, collection_entry::Relation::Item.def())
        .filter(collection_entry::Column::CollectionId.eq(collection.get()))
}

async fn find<C: ConnectionTrait>(
    connection: &C,
    id: CollectionId,
) -> Result<collection::Model, CollectionError> {
    collection::Entity::find_by_id(id.get())
        .one(connection)
        .await
        .map_err(storage)?
        .ok_or(CollectionError::NotFound(id))
}

async fn touch<C: ConnectionTrait>(
    connection: &C,
    id: CollectionId,
    now: i64,
) -> Result<(), CollectionError> {
    collection::Entity::update_many()
        .col_expr(collection::Column::UpdatedAt, Expr::value(now))
        .filter(collection::Column::Id.eq(id.get()))
        .exec(connection)
        .await
        .map_err(storage)?;
    Ok(())
}

/// The next id of the numbering collections share with items.
async fn next_id<C: ConnectionTrait>(connection: &C) -> Result<i64, CollectionError> {
    let highest_item: Option<i64> = metadata_item::Entity::find()
        .select_only()
        .column_as(metadata_item::Column::Id.max(), "id")
        .into_tuple()
        .one(connection)
        .await
        .map_err(storage)?
        .flatten();
    let highest_collection: Option<i64> = collection::Entity::find()
        .select_only()
        .column_as(collection::Column::Id.max(), "id")
        .into_tuple()
        .one(connection)
        .await
        .map_err(storage)?
        .flatten();
    Ok(highest_item.max(highest_collection).unwrap_or(0) + 1)
}

/// The next number items carry a collection under, past every library's.
async fn next_tag_number<C: ConnectionTrait>(connection: &C) -> Result<i64, CollectionError> {
    let highest: Option<i64> = collection::Entity::find()
        .select_only()
        .column_as(collection::Column::TagNumber.max(), "tag_number")
        .into_tuple()
        .one(connection)
        .await
        .map_err(storage)?
        .flatten();
    Ok(highest.unwrap_or(0) + 1)
}

/// The items of a collection and their positions but one, in custom order: every entry,
/// present or not.
async fn positions<C: ConnectionTrait>(
    connection: &C,
    collection: CollectionId,
    except: RatingKey,
) -> Result<Vec<(i64, i64)>, CollectionError> {
    collection_entry::Entity::find()
        .select_only()
        .column(collection_entry::Column::ItemId)
        .column(collection_entry::Column::Position)
        .filter(collection_entry::Column::CollectionId.eq(collection.get()))
        .filter(collection_entry::Column::ItemId.ne(except.get()))
        .order_by_asc(collection_entry::Column::Position)
        .order_by_asc(collection_entry::Column::ItemId)
        .into_tuple()
        .all(connection)
        .await
        .map_err(storage)
}

async fn set_position<C: ConnectionTrait>(
    connection: &C,
    collection: CollectionId,
    item: i64,
    position: i64,
) -> Result<(), CollectionError> {
    collection_entry::Entity::update_many()
        .col_expr(collection_entry::Column::Position, Expr::value(position))
        .filter(collection_entry::Column::CollectionId.eq(collection.get()))
        .filter(collection_entry::Column::ItemId.eq(item))
        .exec(connection)
        .await
        .map_err(storage)?;
    Ok(())
}

/// Appends entries after `after`, in order.
async fn insert_entries<C: ConnectionTrait>(
    connection: &C,
    collection: i64,
    items: &[RatingKey],
    after: Option<i64>,
) -> Result<(), DbErr> {
    let mut last = after;
    let mut placed = Vec::with_capacity(items.len());
    for item in items {
        let position = position_between(last, None).unwrap_or_default();
        last = Some(position);
        placed.push((*item, position));
    }
    let placed: Vec<(i64, RatingKey, i64)> = placed
        .into_iter()
        .map(|(item, position)| (collection, item, position))
        .collect();
    insert_placed(connection, &placed).await
}

/// Inserts entries — collection, item, position — in the order given, which is the order the
/// items join in.
pub(crate) async fn insert_placed<C: ConnectionTrait>(
    connection: &C,
    placed: &[(i64, RatingKey, i64)],
) -> Result<(), DbErr> {
    let rows: Vec<collection_entry::ActiveModel> = placed
        .iter()
        .map(
            |(collection, item, position)| collection_entry::ActiveModel {
                id: ActiveValue::NotSet,
                collection_id: ActiveValue::Set(*collection),
                item_id: ActiveValue::Set(item.get()),
                position: ActiveValue::Set(*position),
            },
        )
        .collect();
    for batch in rows.chunks(BATCH) {
        collection_entry::Entity::insert_many(batch.to_vec())
            .exec(connection)
            .await?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl CollectionRepository for SqlCollectionRepository {
    async fn create(&self, new: NewCollection, now: i64) -> Result<Collection, CollectionError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let collection = Collection {
            id: CollectionId::new(next_id(&transaction).await?),
            section: new.section,
            kind: new.kind,
            guid: new_guid(),
            tag_number: next_tag_number(&transaction).await?,
            title_sort: derived_title_sort(&new.title),
            title: new.title,
            summary: String::new(),
            content_rating: None,
            rule: new.rule,
            source_uri: new.source_uri,
            mode: CollectionMode::LibraryDefault,
            order: CollectionOrder::ReleaseDate,
            locked: Vec::new(),
            thumb_url: None,
            art_url: None,
            added_at: now,
            updated_at: now,
        };
        write::save_collection(&transaction, to_row(&collection))
            .await
            .map_err(storage)?;
        insert_entries(
            &transaction,
            collection.id.get(),
            &new_entries(&new.items, &[]),
            None,
        )
        .await
        .map_err(storage)?;
        transaction.commit().await.map_err(storage)?;
        Ok(collection)
    }

    async fn collection(&self, id: CollectionId) -> Result<Option<Collection>, CollectionError> {
        Ok(collection::Entity::find_by_id(id.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(to_collection))
    }

    async fn collections(
        &self,
        query: &CollectionQuery,
        window: Window,
    ) -> Result<Page<Collection>, CollectionError> {
        let order = ListingOrder {
            sort: query.sort,
            descending: query.direction.is_descending(),
        };
        let mut select = filtered(query);
        if let Some(after) = &window.after {
            select = select.filter(past(&order.terms(after), true));
        }
        if let Some(before) = &window.before {
            select = select.filter(past(&order.terms(before), false));
        }
        let reversed = window.is_reversed();
        for (expr, descending) in order.columns() {
            let order = if descending == reversed {
                Order::Asc
            } else {
                Order::Desc
            };
            select = select.order_by(expr, order);
        }
        let rows = select
            .offset(window.skip)
            .limit(window.fetch_limit())
            .all(&self.connection)
            .await
            .map_err(storage)?;
        let mut page = Page::from_overfetch(rows, &window).map(to_collection);
        if reversed {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn count(&self, query: &CollectionQuery) -> Result<u64, CollectionError> {
        filtered(query)
            .count(&self.connection)
            .await
            .map_err(storage)
    }

    async fn edit(
        &self,
        id: CollectionId,
        edit: &CollectionEdit,
    ) -> Result<Collection, CollectionError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let mut collection = to_collection(find(&transaction, id).await?);
        edit.apply(&mut collection);
        write::save_collection(&transaction, to_row(&collection))
            .await
            .map_err(storage)?;
        transaction.commit().await.map_err(storage)?;
        Ok(collection)
    }

    async fn delete(&self, id: CollectionId) -> Result<bool, CollectionError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        // Children first, whether or not the engine enforces the cascade.
        collection_entry::Entity::delete_many()
            .filter(collection_entry::Column::CollectionId.eq(id.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        collection_word::Entity::delete_many()
            .filter(collection_word::Column::CollectionId.eq(id.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        let deleted = collection::Entity::delete_by_id(id.get())
            .exec(&transaction)
            .await
            .map_err(storage)?
            .rows_affected;
        transaction.commit().await.map_err(storage)?;
        Ok(deleted > 0)
    }

    async fn entries(
        &self,
        collection: CollectionId,
        window: Window,
    ) -> Result<Page<CollectionEntry>, CollectionError> {
        let position: SimpleExpr =
            Expr::col((collection_entry::Entity, collection_entry::Column::Position)).into();
        let item: SimpleExpr =
            Expr::col((collection_entry::Entity, collection_entry::Column::ItemId)).into();
        let terms = |cursor: &Cursor| {
            vec![
                compared(&position, int_key(cursor, 0).into(), false),
                compared(&item, cursor.id.into(), false),
            ]
        };
        let mut select = present_entries(collection);
        if let Some(after) = &window.after {
            select = select.filter(past(&terms(after), true));
        }
        if let Some(before) = &window.before {
            select = select.filter(past(&terms(before), false));
        }
        let order = if window.is_reversed() {
            Order::Desc
        } else {
            Order::Asc
        };
        let rows = select
            .order_by(position, order.clone())
            .order_by(item, order)
            .offset(window.skip)
            .limit(window.fetch_limit())
            .all(&self.connection)
            .await
            .map_err(storage)?;
        let mut page = Page::from_overfetch(rows, &window).map(|row| CollectionEntry {
            collection: CollectionId::new(row.collection_id),
            item: RatingKey::new(row.item_id),
            position: row.position,
        });
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn totals(
        &self,
        collections: &[CollectionId],
    ) -> Result<Vec<(CollectionId, CollectionTotals)>, CollectionError> {
        let mut totals = Vec::with_capacity(collections.len());
        let year = || Expr::col((metadata_item::Entity, metadata_item::Column::Year));
        for batch in collections.chunks(BATCH) {
            let rows: Vec<(i64, i64, Option<i32>, Option<i32>)> = collection_entry::Entity::find()
                .join(JoinType::InnerJoin, collection_entry::Relation::Item.def())
                .select_only()
                .column(collection_entry::Column::CollectionId)
                .column_as(
                    Expr::col((collection_entry::Entity, collection_entry::Column::Id)).count(),
                    "items",
                )
                .column_as(year().min(), "min_year")
                .column_as(year().max(), "max_year")
                .filter(
                    collection_entry::Column::CollectionId.is_in(batch.iter().map(|id| id.get())),
                )
                .group_by(collection_entry::Column::CollectionId)
                .into_tuple()
                .all(&self.connection)
                .await
                .map_err(storage)?;
            totals.extend(rows.into_iter().map(|(id, items, min_year, max_year)| {
                (
                    CollectionId::new(id),
                    CollectionTotals {
                        items: u64::try_from(items).unwrap_or(0),
                        min_year,
                        max_year,
                    },
                )
            }));
        }
        Ok(totals)
    }

    async fn memberships(&self, items: &[RatingKey]) -> Result<Vec<Membership>, CollectionError> {
        let mut memberships = Vec::new();
        for batch in items.chunks(BATCH) {
            let rows: Vec<(collection_entry::Model, Option<collection::Model>)> =
                collection_entry::Entity::find()
                    .find_also_related(collection::Entity)
                    .filter(collection_entry::Column::ItemId.is_in(batch.iter().map(|id| id.get())))
                    .order_by_asc(collection_entry::Column::Id)
                    .all(&self.connection)
                    .await
                    .map_err(storage)?;
            memberships.extend(rows.into_iter().filter_map(|(entry, collection)| {
                collection.map(|collection| Membership {
                    item: RatingKey::new(entry.item_id),
                    collection: to_collection(collection),
                })
            }));
        }
        Ok(memberships)
    }

    async fn add(
        &self,
        collection: CollectionId,
        items: &[RatingKey],
        now: i64,
    ) -> Result<u64, CollectionError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        find(&transaction, collection).await?;
        let mut present = Vec::new();
        for batch in items.chunks(BATCH) {
            let found: Vec<i64> = collection_entry::Entity::find()
                .select_only()
                .column(collection_entry::Column::ItemId)
                .filter(collection_entry::Column::CollectionId.eq(collection.get()))
                .filter(collection_entry::Column::ItemId.is_in(batch.iter().map(|item| item.get())))
                .into_tuple()
                .all(&transaction)
                .await
                .map_err(storage)?;
            present.extend(found.into_iter().map(RatingKey::new));
        }
        let last: Option<i64> = collection_entry::Entity::find()
            .select_only()
            .column(collection_entry::Column::Position)
            .filter(collection_entry::Column::CollectionId.eq(collection.get()))
            .order_by_desc(collection_entry::Column::Position)
            .into_tuple()
            .one(&transaction)
            .await
            .map_err(storage)?;
        let added = new_entries(items, &present);
        insert_entries(&transaction, collection.get(), &added, last)
            .await
            .map_err(storage)?;
        if !added.is_empty() {
            touch(&transaction, collection, now).await?;
        }
        transaction.commit().await.map_err(storage)?;
        Ok(u64::try_from(added.len()).unwrap_or(u64::MAX))
    }

    async fn remove(
        &self,
        collection: CollectionId,
        item: RatingKey,
        now: i64,
    ) -> Result<bool, CollectionError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        find(&transaction, collection).await?;
        let removed = collection_entry::Entity::delete_many()
            .filter(collection_entry::Column::CollectionId.eq(collection.get()))
            .filter(collection_entry::Column::ItemId.eq(item.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?
            .rows_affected
            > 0;
        if removed {
            touch(&transaction, collection, now).await?;
        }
        transaction.commit().await.map_err(storage)?;
        Ok(removed)
    }

    async fn move_item(
        &self,
        collection: CollectionId,
        item: RatingKey,
        destination: Destination,
    ) -> Result<(), CollectionError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let present = collection_entry::Entity::find()
            .filter(collection_entry::Column::CollectionId.eq(collection.get()))
            .filter(collection_entry::Column::ItemId.eq(item.get()))
            .one(&transaction)
            .await
            .map_err(storage)?;
        if present.is_none() {
            return Err(CollectionError::NotInCollection(item));
        }
        let mut order = positions(&transaction, collection, item).await?;
        let at = match destination {
            Destination::First => 0,
            Destination::After(after) => order
                .iter()
                .position(|(candidate, _)| *candidate == after.get())
                .map(|at| at + 1)
                .ok_or(CollectionError::NotInCollection(after))?,
        };
        let around = |order: &[(i64, i64)]| {
            let previous = at.checked_sub(1).and_then(|index| order.get(index));
            (
                previous.map(|(_, position)| *position),
                order.get(at).map(|(_, position)| *position),
            )
        };
        let (previous, next) = around(&order);
        let position = if let Some(position) = position_between(previous, next) {
            position
        } else {
            for ((entry, position), rank) in order.iter_mut().zip(1_i64..) {
                *position = rank * POSITION_GAP;
                set_position(&transaction, collection, *entry, *position).await?;
            }
            let (previous, next) = around(&order);
            position_between(previous, next).unwrap_or_default()
        };
        set_position(&transaction, collection, item.get(), position).await?;
        transaction.commit().await.map_err(storage)
    }

    async fn clear(&self, collection: CollectionId) -> Result<(), CollectionError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        find(&transaction, collection).await?;
        collection_entry::Entity::delete_many()
            .filter(collection_entry::Column::CollectionId.eq(collection.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        transaction.commit().await.map_err(storage)
    }
}
