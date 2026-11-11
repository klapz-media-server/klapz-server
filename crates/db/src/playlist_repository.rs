//! Stored playlists.
//!
//! Every read of an entry joins it to its item, so that what a reader may see is decided by
//! the item's library at the moment of reading, and an entry whose item is gone is hidden.

use open_plex_core::page::{Cursor, CursorKey, Page, Window};
use open_plex_core::playlist::{
    new_entries, new_guid, position_between, sort_key, Destination, NewPlaylist, Playlist,
    PlaylistEdit, PlaylistEntry, PlaylistError, PlaylistField, PlaylistKind, PlaylistQuery,
    PlaylistRepository, PlaylistSort, PlaylistTotals, SmartRule, POSITION_GAP,
};
use open_plex_core::watch::Viewer;
use open_plex_core::{PlaylistEntryId, PlaylistId, RatingKey, SectionId, UserId};
use sea_orm::sea_query::{Alias, Expr, Func, SimpleExpr};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection,
    DbErr, EntityTrait, JoinType, Order, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    RelationTrait, Select, TransactionTrait,
};

use crate::entity::{metadata_item, playlist, playlist_entry};
use crate::watch_repository::viewer_id;

/// `SQLite` refuses a statement with too many bound values.
const BATCH: usize = 200;

#[derive(Debug, Clone)]
pub struct SqlPlaylistRepository {
    connection: DatabaseConnection,
}

impl SqlPlaylistRepository {
    #[must_use]
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }
}

fn storage(error: DbErr) -> PlaylistError {
    PlaylistError::Storage(Box::new(error))
}

const fn kind_name(kind: PlaylistKind) -> &'static str {
    match kind {
        PlaylistKind::Video => "video",
        PlaylistKind::Audio => "audio",
        PlaylistKind::Photo => "photo",
    }
}

fn kind_of(name: &str) -> PlaylistKind {
    match name {
        "audio" => PlaylistKind::Audio,
        "photo" => PlaylistKind::Photo,
        _ => PlaylistKind::Video,
    }
}

fn owner_of(stored: i64) -> Viewer {
    if stored == 0 {
        Viewer::Operator
    } else {
        Viewer::User(UserId::new(stored))
    }
}

fn to_playlist(row: playlist::Model) -> Playlist {
    let rule = row
        .rule_section_id
        .zip(row.rule_listing)
        .map(|(section, listing)| SmartRule {
            section: SectionId::new(section),
            listing,
        });
    let locked = [
        (row.title_locked, PlaylistField::Title),
        (row.summary_locked, PlaylistField::Summary),
    ]
    .into_iter()
    .filter_map(|(locked, field)| locked.then_some(field))
    .collect();
    Playlist {
        id: PlaylistId::new(row.id),
        owner: owner_of(row.owner),
        guid: row.guid,
        title: row.title,
        summary: row.summary,
        kind: kind_of(&row.kind),
        rule,
        source_uri: row.source_uri,
        locked,
        view_count: u64::try_from(row.view_count).unwrap_or(0),
        last_viewed_at: row.last_viewed_at,
        added_at: row.added_at,
        updated_at: row.updated_at,
    }
}

fn to_entry(row: playlist_entry::Model) -> PlaylistEntry {
    PlaylistEntry {
        id: PlaylistEntryId::new(row.id),
        playlist: PlaylistId::new(row.playlist_id),
        item: RatingKey::new(row.item_id),
        position: row.position,
    }
}

/// One ordering term, read against a cursor's value: the rows after it in the listing's
/// order, the rows before it, and the rows level with it.
struct Term {
    ahead: Condition,
    behind: Condition,
    level: Condition,
}

fn never() -> Condition {
    Condition::all().add(Expr::val(1).eq(0))
}

fn compared(expr: &SimpleExpr, value: SimpleExpr, descending: bool) -> Term {
    let greater = Condition::all().add(Expr::expr(expr.clone()).gt(value.clone()));
    let less = Condition::all().add(Expr::expr(expr.clone()).lt(value.clone()));
    let (ahead, behind) = if descending {
        (less, greater)
    } else {
        (greater, less)
    };
    Term {
        ahead,
        behind,
        level: Condition::all().add(Expr::expr(expr.clone()).eq(value)),
    }
}

/// Rows strictly past the cursor in reading order: ahead on the first term, or level on it
/// and past on the rest.
fn past(terms: &[Term], forward: bool) -> Condition {
    match terms.split_first() {
        None => never(),
        Some((term, rest)) => {
            let beyond = if forward { &term.ahead } else { &term.behind };
            Condition::any().add(beyond.clone()).add(
                Condition::all()
                    .add(term.level.clone())
                    .add(past(rest, forward)),
            )
        }
    }
}

fn int_key(cursor: &Cursor, at: usize) -> i64 {
    match cursor.keys.get(at) {
        Some(CursorKey::Int(value)) => *value,
        _ => 0,
    }
}

fn text_key(cursor: &Cursor, at: usize) -> String {
    match cursor.keys.get(at) {
        Some(CursorKey::Text(value)) => value.clone(),
        _ => String::new(),
    }
}

fn column(column: playlist::Column) -> SimpleExpr {
    Expr::col((playlist::Entity, column)).into()
}

/// The order of a playlist listing, and the terms a cursor is compared on.
struct PlaylistOrder {
    sort: PlaylistSort,
    descending: bool,
}

impl PlaylistOrder {
    fn leading(&self) -> Option<SimpleExpr> {
        match self.sort {
            PlaylistSort::Title => None,
            PlaylistSort::AddedAt => Some(column(playlist::Column::AddedAt)),
            PlaylistSort::UpdatedAt => Some(column(playlist::Column::UpdatedAt)),
            PlaylistSort::LastViewedAt => Some(column(playlist::Column::LastViewedAt)),
        }
    }

    /// Smart first, then the title, both in the listing's direction; or the value, then
    /// the title ascending.
    fn columns(&self) -> Vec<(SimpleExpr, bool)> {
        let smart = Expr::col((playlist::Entity, playlist::Column::RuleSectionId)).is_null();
        match self.leading() {
            None => vec![
                (smart, false),
                (column(playlist::Column::SortKey), self.descending),
                (column(playlist::Column::Id), self.descending),
            ],
            Some(leading) => vec![
                (leading, self.descending),
                (column(playlist::Column::SortKey), false),
                (column(playlist::Column::Id), false),
            ],
        }
    }

    fn terms(&self, cursor: &Cursor) -> Vec<Term> {
        let leading = match self.leading() {
            None => {
                let smart = Expr::col((playlist::Entity, playlist::Column::RuleSectionId));
                let is_smart = Condition::all().add(smart.clone().is_not_null());
                let is_plain = Condition::all().add(smart.is_null());
                if int_key(cursor, 0) == 1 {
                    Term {
                        ahead: is_plain,
                        behind: never(),
                        level: is_smart,
                    }
                } else {
                    Term {
                        ahead: never(),
                        behind: is_smart,
                        level: is_plain,
                    }
                }
            }
            Some(leading) => compared(&leading, int_key(cursor, 0).into(), self.descending),
        };
        let title_descending = self.leading().is_none() && self.descending;
        vec![
            leading,
            compared(
                &column(playlist::Column::SortKey),
                text_key(cursor, 1).into(),
                title_descending,
            ),
            compared(
                &column(playlist::Column::Id),
                cursor.id.into(),
                title_descending,
            ),
        ]
    }
}

fn filtered(query: &PlaylistQuery) -> Select<playlist::Entity> {
    let mut select =
        playlist::Entity::find().filter(playlist::Column::Owner.eq(viewer_id(query.owner)));
    let filter = &query.filter;
    if let Some(kind) = filter.kind {
        select = select.filter(playlist::Column::Kind.eq(kind_name(kind)));
    }
    match filter.smart {
        Some(true) => select = select.filter(playlist::Column::RuleSectionId.is_not_null()),
        Some(false) => select = select.filter(playlist::Column::RuleSectionId.is_null()),
        None => {}
    }
    if let Some(library) = filter.library {
        select = select.filter(
            Condition::any()
                .add(playlist::Column::RuleSectionId.is_null())
                .add(playlist::Column::RuleSectionId.eq(library.get())),
        );
    }
    select
}

/// A playlist's entries whose items exist and lie in `sections`.
fn visible_entries(
    playlist: PlaylistId,
    sections: Option<&[SectionId]>,
) -> Select<playlist_entry::Entity> {
    let mut select = playlist_entry::Entity::find()
        .join(JoinType::InnerJoin, playlist_entry::Relation::Item.def())
        .filter(playlist_entry::Column::PlaylistId.eq(playlist.get()));
    if let Some(sections) = sections {
        select = select.filter(
            metadata_item::Column::LibrarySectionId.is_in(sections.iter().map(|id| id.get())),
        );
    }
    select
}

async fn find_entry<C: ConnectionTrait>(
    connection: &C,
    id: PlaylistEntryId,
) -> Result<playlist_entry::Model, PlaylistError> {
    playlist_entry::Entity::find_by_id(id.get())
        .one(connection)
        .await
        .map_err(storage)?
        .ok_or(PlaylistError::EntryNotFound(id))
}

async fn touch<C: ConnectionTrait>(
    connection: &C,
    id: PlaylistId,
    now: i64,
) -> Result<(), PlaylistError> {
    let updated = playlist::Entity::update_many()
        .col_expr(playlist::Column::UpdatedAt, Expr::value(now))
        .filter(playlist::Column::Id.eq(id.get()))
        .exec(connection)
        .await
        .map_err(storage)?;
    if updated.rows_affected == 0 {
        return Err(PlaylistError::NotFound(id));
    }
    Ok(())
}

/// The positions of a playlist's entries but one, in order: every entry, hidden or not,
/// since a reader who is given a library back finds its entries where they were.
async fn positions<C: ConnectionTrait>(
    connection: &C,
    playlist: i64,
    except: i64,
) -> Result<Vec<(i64, i64)>, PlaylistError> {
    playlist_entry::Entity::find()
        .select_only()
        .column(playlist_entry::Column::Id)
        .column(playlist_entry::Column::Position)
        .filter(playlist_entry::Column::PlaylistId.eq(playlist))
        .filter(playlist_entry::Column::Id.ne(except))
        .order_by_asc(playlist_entry::Column::Position)
        .order_by_asc(playlist_entry::Column::Id)
        .into_tuple()
        .all(connection)
        .await
        .map_err(storage)
}

async fn set_position<C: ConnectionTrait>(
    connection: &C,
    entry: i64,
    position: i64,
) -> Result<(), PlaylistError> {
    playlist_entry::Entity::update_many()
        .col_expr(playlist_entry::Column::Position, Expr::value(position))
        .filter(playlist_entry::Column::Id.eq(entry))
        .exec(connection)
        .await
        .map_err(storage)?;
    Ok(())
}

/// The neighbours a moved entry goes between, as indexes into `order`.
fn neighbours(order: &[(i64, i64)], destination: Destination) -> Result<usize, PlaylistError> {
    match destination {
        Destination::First => Ok(0),
        Destination::After(after) => order
            .iter()
            .position(|(id, _)| *id == after.get())
            .map(|at| at + 1)
            .ok_or(PlaylistError::EntryNotFound(after)),
    }
}

#[async_trait::async_trait]
impl PlaylistRepository for SqlPlaylistRepository {
    async fn create(&self, new: NewPlaylist, now: i64) -> Result<Playlist, PlaylistError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let (rule_section_id, rule_listing) = new
            .rule
            .map(|rule| (rule.section.get(), rule.listing))
            .unzip();
        let row = playlist::ActiveModel {
            id: ActiveValue::NotSet,
            owner: ActiveValue::Set(viewer_id(new.owner)),
            guid: ActiveValue::Set(new_guid()),
            sort_key: ActiveValue::Set(sort_key(&new.title)),
            title: ActiveValue::Set(new.title),
            summary: ActiveValue::Set(new.summary),
            kind: ActiveValue::Set(kind_name(new.kind).to_owned()),
            rule_section_id: ActiveValue::Set(rule_section_id),
            rule_listing: ActiveValue::Set(rule_listing),
            source_uri: ActiveValue::Set(new.source_uri),
            title_locked: ActiveValue::Set(false),
            summary_locked: ActiveValue::Set(false),
            // The reference counts a playlist's creation as its first play.
            view_count: ActiveValue::Set(1),
            last_viewed_at: ActiveValue::Set(now),
            added_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(&transaction)
        .await
        .map_err(storage)?;
        insert_entries(
            &transaction,
            row.id,
            &new_entries(&new.items, &[]),
            None,
            now,
        )
        .await?;
        transaction.commit().await.map_err(storage)?;
        Ok(to_playlist(row))
    }

    async fn playlist(&self, id: PlaylistId) -> Result<Option<Playlist>, PlaylistError> {
        Ok(playlist::Entity::find_by_id(id.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(to_playlist))
    }

    async fn playlists(
        &self,
        query: &PlaylistQuery,
        window: Window,
    ) -> Result<Page<Playlist>, PlaylistError> {
        let order = PlaylistOrder {
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
        let mut page = Page::from_overfetch(rows, &window).map(to_playlist);
        if reversed {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn count(&self, query: &PlaylistQuery) -> Result<u64, PlaylistError> {
        filtered(query)
            .count(&self.connection)
            .await
            .map_err(storage)
    }

    async fn edit(
        &self,
        id: PlaylistId,
        edit: &PlaylistEdit,
        now: i64,
    ) -> Result<Playlist, PlaylistError> {
        let row = playlist::Entity::find_by_id(id.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .ok_or(PlaylistError::NotFound(id))?;
        let lock = |field: PlaylistField, current: bool| {
            if edit.lock.contains(&field) {
                true
            } else {
                current && !edit.unlock.contains(&field)
            }
        };
        let mut active: playlist::ActiveModel = row.clone().into();
        if let Some(title) = &edit.title {
            active.sort_key = ActiveValue::Set(sort_key(title));
            active.title = ActiveValue::Set(title.clone());
        }
        if let Some(summary) = &edit.summary {
            active.summary = ActiveValue::Set(summary.clone());
        }
        active.title_locked = ActiveValue::Set(lock(PlaylistField::Title, row.title_locked));
        active.summary_locked = ActiveValue::Set(lock(PlaylistField::Summary, row.summary_locked));
        active.updated_at = ActiveValue::Set(now);
        let row = active.update(&self.connection).await.map_err(storage)?;
        Ok(to_playlist(row))
    }

    async fn delete(&self, id: PlaylistId) -> Result<bool, PlaylistError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        playlist_entry::Entity::delete_many()
            .filter(playlist_entry::Column::PlaylistId.eq(id.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        let deleted = playlist::Entity::delete_by_id(id.get())
            .exec(&transaction)
            .await
            .map_err(storage)?
            .rows_affected;
        transaction.commit().await.map_err(storage)?;
        Ok(deleted > 0)
    }

    async fn entry(&self, id: PlaylistEntryId) -> Result<Option<PlaylistEntry>, PlaylistError> {
        Ok(playlist_entry::Entity::find_by_id(id.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(to_entry))
    }

    async fn entries(
        &self,
        playlist: PlaylistId,
        sections: Option<&[SectionId]>,
        window: Window,
    ) -> Result<Page<PlaylistEntry>, PlaylistError> {
        if sections.is_some_and(<[SectionId]>::is_empty) {
            return Ok(Page::from_overfetch(Vec::new(), &window));
        }
        let position: SimpleExpr =
            Expr::col((playlist_entry::Entity, playlist_entry::Column::Position)).into();
        let id: SimpleExpr = Expr::col((playlist_entry::Entity, playlist_entry::Column::Id)).into();
        let terms = |cursor: &Cursor| {
            vec![
                compared(&position, int_key(cursor, 0).into(), false),
                compared(&id, cursor.id.into(), false),
            ]
        };
        let mut select = visible_entries(playlist, sections);
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
            .order_by(id, order)
            .offset(window.skip)
            .limit(window.fetch_limit())
            .all(&self.connection)
            .await
            .map_err(storage)?;
        let mut page = Page::from_overfetch(rows, &window).map(to_entry);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn totals(
        &self,
        playlists: &[PlaylistId],
        sections: Option<&[SectionId]>,
    ) -> Result<Vec<(PlaylistId, PlaylistTotals)>, PlaylistError> {
        if sections.is_some_and(<[SectionId]>::is_empty) {
            return Ok(Vec::new());
        }
        let mut totals = Vec::with_capacity(playlists.len());
        for batch in playlists.chunks(BATCH) {
            let mut select = playlist_entry::Entity::find()
                .join(JoinType::InnerJoin, playlist_entry::Relation::Item.def())
                .select_only()
                .column(playlist_entry::Column::PlaylistId)
                .column_as(
                    Expr::col((playlist_entry::Entity, playlist_entry::Column::Id)).count(),
                    "entries",
                )
                // A sum is wider than its column on some engines; cast back to what we read.
                .column_as(
                    Expr::expr(Func::cast_as(
                        Func::sum(Expr::col((
                            metadata_item::Entity,
                            metadata_item::Column::DurationMs,
                        ))),
                        Alias::new("BIGINT"),
                    )),
                    "duration_ms",
                )
                .filter(playlist_entry::Column::PlaylistId.is_in(batch.iter().map(|id| id.get())))
                .group_by(playlist_entry::Column::PlaylistId);
            if let Some(sections) = sections {
                select = select.filter(
                    metadata_item::Column::LibrarySectionId
                        .is_in(sections.iter().map(|id| id.get())),
                );
            }
            let rows: Vec<(i64, i64, Option<i64>)> = select
                .into_tuple()
                .all(&self.connection)
                .await
                .map_err(storage)?;
            totals.extend(rows.into_iter().map(|(playlist, entries, duration)| {
                (
                    PlaylistId::new(playlist),
                    PlaylistTotals {
                        entries: u64::try_from(entries).unwrap_or(0),
                        duration_ms: duration.unwrap_or(0),
                    },
                )
            }));
        }
        Ok(totals)
    }

    async fn append(
        &self,
        playlist: PlaylistId,
        items: &[RatingKey],
        now: i64,
    ) -> Result<u64, PlaylistError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        touch(&transaction, playlist, now).await?;
        let mut present = Vec::new();
        for batch in items.chunks(BATCH) {
            let found: Vec<i64> = playlist_entry::Entity::find()
                .select_only()
                .column(playlist_entry::Column::ItemId)
                .filter(playlist_entry::Column::PlaylistId.eq(playlist.get()))
                .filter(playlist_entry::Column::ItemId.is_in(batch.iter().map(|item| item.get())))
                .into_tuple()
                .all(&transaction)
                .await
                .map_err(storage)?;
            present.extend(found.into_iter().map(RatingKey::new));
        }
        let last: Option<i64> = playlist_entry::Entity::find()
            .select_only()
            .column(playlist_entry::Column::Position)
            .filter(playlist_entry::Column::PlaylistId.eq(playlist.get()))
            .order_by_desc(playlist_entry::Column::Position)
            .into_tuple()
            .one(&transaction)
            .await
            .map_err(storage)?;
        let added = new_entries(items, &present);
        insert_entries(&transaction, playlist.get(), &added, last, now).await?;
        transaction.commit().await.map_err(storage)?;
        Ok(u64::try_from(added.len()).unwrap_or(u64::MAX))
    }

    async fn remove(&self, entry: PlaylistEntryId, now: i64) -> Result<(), PlaylistError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let row = find_entry(&transaction, entry).await?;
        playlist_entry::Entity::delete_by_id(row.id)
            .exec(&transaction)
            .await
            .map_err(storage)?;
        touch(&transaction, PlaylistId::new(row.playlist_id), now).await?;
        transaction.commit().await.map_err(storage)
    }

    async fn move_entry(
        &self,
        entry: PlaylistEntryId,
        destination: Destination,
        now: i64,
    ) -> Result<(), PlaylistError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let moved = find_entry(&transaction, entry).await?;
        let mut order = positions(&transaction, moved.playlist_id, moved.id).await?;
        let at = neighbours(&order, destination)?;
        let around = |order: &[(i64, i64)]| {
            let previous = at.checked_sub(1).and_then(|index| order.get(index));
            (
                previous.map(|(_, position)| *position),
                order.get(at).map(|(_, position)| *position),
            )
        };
        let (previous, next) = around(&order);
        let position = match position_between(previous, next) {
            Some(position) => position,
            None => {
                for ((id, position), rank) in order.iter_mut().zip(1_i64..) {
                    *position = rank * POSITION_GAP;
                    set_position(&transaction, *id, *position).await?;
                }
                let (previous, next) = around(&order);
                position_between(previous, next).unwrap_or_default()
            }
        };
        set_position(&transaction, moved.id, position).await?;
        touch(&transaction, PlaylistId::new(moved.playlist_id), now).await?;
        transaction.commit().await.map_err(storage)
    }

    async fn clear(&self, playlist: PlaylistId, now: i64) -> Result<(), PlaylistError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        touch(&transaction, playlist, now).await?;
        playlist_entry::Entity::delete_many()
            .filter(playlist_entry::Column::PlaylistId.eq(playlist.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        transaction.commit().await.map_err(storage)
    }

    async fn replace_rule(
        &self,
        playlist: PlaylistId,
        rule: SmartRule,
        source_uri: Option<String>,
        now: i64,
    ) -> Result<Playlist, PlaylistError> {
        let row = playlist::Entity::find_by_id(playlist.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .ok_or(PlaylistError::NotFound(playlist))?;
        let mut active: playlist::ActiveModel = row.into();
        active.rule_section_id = ActiveValue::Set(Some(rule.section.get()));
        active.rule_listing = ActiveValue::Set(Some(rule.listing));
        active.source_uri = ActiveValue::Set(source_uri);
        active.updated_at = ActiveValue::Set(now);
        Ok(to_playlist(
            active.update(&self.connection).await.map_err(storage)?,
        ))
    }

    async fn record_play(&self, playlist: PlaylistId, now: i64) -> Result<(), PlaylistError> {
        let updated = playlist::Entity::update_many()
            .col_expr(
                playlist::Column::ViewCount,
                Expr::col(playlist::Column::ViewCount).add(1),
            )
            .col_expr(playlist::Column::LastViewedAt, Expr::value(now))
            .filter(playlist::Column::Id.eq(playlist.get()))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        if updated.rows_affected == 0 {
            return Err(PlaylistError::NotFound(playlist));
        }
        Ok(())
    }
}

async fn insert_entries<C: ConnectionTrait>(
    connection: &C,
    playlist: i64,
    items: &[RatingKey],
    after: Option<i64>,
    now: i64,
) -> Result<(), PlaylistError> {
    let mut last = after;
    let mut rows = Vec::with_capacity(items.len());
    for item in items {
        let position = position_between(last, None).unwrap_or_default();
        last = Some(position);
        rows.push(playlist_entry::ActiveModel {
            id: ActiveValue::NotSet,
            playlist_id: ActiveValue::Set(playlist),
            item_id: ActiveValue::Set(item.get()),
            position: ActiveValue::Set(position),
            added_at: ActiveValue::Set(now),
        });
    }
    for batch in rows.chunks(BATCH) {
        playlist_entry::Entity::insert_many(batch.to_vec())
            .exec(connection)
            .await
            .map_err(storage)?;
    }
    Ok(())
}
