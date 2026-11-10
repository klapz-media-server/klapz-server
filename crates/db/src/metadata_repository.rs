//! Storage adapter for library contents.
//!
//! The interesting part is keyset pagination: a window becomes `WHERE`, `ORDER BY` and
//! `LIMIT` so the database returns one page of a 171,018-row table rather than all of it.
//! Ordering by anything but the primary key needs a composite comparison, which is what the
//! cursor's sort key is for.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use open_plex_core::edit::{ItemEdit, ItemField};
use open_plex_core::metadata::{
    first_character_bucket, AudioAnalysis, ColorDescription, DolbyVision, Edited, EpisodeCredit,
    ExternalId, FieldValue, FileLayout, FirstCharacter, FrameRange, ItemFilter, ItemKind,
    ItemQuery, ItemTag, Listed, Loudness, MediaFile, MediaStream, MediaVersion, MetadataError,
    MetadataItem, MetadataRepository, Person, Rating, ScanType, SearchQuery, StreamKind,
    SubtitleAnalysis, Tag, TagCondition, TagKind, TagRef, TagScope, TrackTraits, ValueField,
    VideoAnalysis,
};
use open_plex_core::page::{Cursor, CursorKey, Direction, Page, Window};
use open_plex_core::recent::EpisodeAddition;
use open_plex_core::search;
use open_plex_core::similar::TagOverlap;
use open_plex_core::watch::Viewer;
use open_plex_core::{MediaId, PartId, PersonId, RatingKey, SectionId, StreamId, TagId};
use sea_orm::sea_query::{Alias, Expr, Func, Query, SimpleExpr};
use sea_orm::QueryTrait;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait, FromQueryResult,
    JoinType, Order, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, RelationTrait, Select,
    TransactionTrait,
};

use crate::entity::{
    item_external_id, item_field_lock, item_rating, item_tag, item_title_word, media, media_part,
    media_stream, metadata_item, person, stream_dolby_vision, stream_loudness, tag, watch_state,
};
use crate::item_order::ItemOrder;
use crate::watch_repository::viewer_id;
use crate::write;

#[derive(Debug, Clone)]
pub struct SqlMetadataRepository {
    connection: DatabaseConnection,
}

impl SqlMetadataRepository {
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }

    /// Everything a query says about *which* rows, without ordering or paging.
    ///
    /// Each filter is a conjunct, so an absent one places no constraint. The resolution
    /// filter is the only one that leaves the item table, and it is expressed as an
    /// `EXISTS` subquery rather than a join: a join would multiply rows when an item has
    /// several versions, and 8 films here do.
    fn selection(query: &ItemQuery) -> Select<metadata_item::Entity> {
        let filter = &query.filter;
        let mut select = metadata_item::Entity::find().filter(
            metadata_item::Column::LibrarySectionId
                .is_in(query.sections.iter().map(|section| section.get())),
        );

        if let Some(kind) = filter.kind {
            select = select.filter(metadata_item::Column::Kind.eq(kind_name(kind)));
        }
        if filter.top_level_only() {
            select = select.filter(metadata_item::Column::ParentId.is_null());
        }
        if let Some(from) = filter.year_from {
            select = select.filter(metadata_item::Column::Year.gte(from));
        }
        if let Some(to) = filter.year_to {
            select = select.filter(metadata_item::Column::Year.lte(to));
        }
        if let Some(rating) = &filter.content_rating {
            select = select.filter(metadata_item::Column::ContentRating.eq(rating.clone()));
        }
        if let Some(studio) = &filter.studio_contains {
            // `LOWER(col) LIKE LOWER(pattern)` rather than relying on `LIKE` being
            // case-insensitive: SQLite's is for ASCII, PostgreSQL's is not at all, and the
            // engine is meant to stay swappable.
            select = select.filter(
                Expr::expr(Func::lower(Expr::col((
                    metadata_item::Entity,
                    metadata_item::Column::Studio,
                ))))
                .like(contains_pattern(&studio.to_lowercase())),
            );
        }
        if let Some(prefix) = &filter.title_word_prefix {
            // Driven from the word index rather than the title column: the reference matches
            // a prefix of *any* word, so `hell` has to find "Good News About Hell".
            select = select.filter(term_matches(prefix));
        }
        if let Some(excluded) = filter.excluded {
            select = select.filter(metadata_item::Column::Id.ne(excluded.get()));
        }
        if let Some(range) = filter.frame {
            select = select.filter(Expr::exists(
                Query::select()
                    .expr(Expr::val(1))
                    .from(media::Entity)
                    .and_where(
                        Expr::col((media::Entity, media::Column::MetadataItemId))
                            .equals((metadata_item::Entity, metadata_item::Column::Id)),
                    )
                    .cond_where(frame_condition(range))
                    .to_owned(),
            ));
        }
        for condition in &filter.tags {
            select = select.filter(carries_tag(condition));
        }
        select = select.filter(watch_conditions(filter));
        if let Some(from) = &filter.released_from {
            select = select.filter(metadata_item::Column::ReleasedOn.gte(from.clone()));
        }
        if let Some(from) = filter.audience_rating_from {
            select = select.filter(metadata_item::Column::AudienceRating.gte(from));
        }

        select
    }
}

impl SqlMetadataRepository {
    /// Completes tracks with their Dolby Vision and loudness rows, two batched queries for
    /// however many tracks: few tracks have either, 2,651 and 89 of 945,000 here.
    async fn with_analyses(
        &self,
        rows: Vec<media_stream::Model>,
    ) -> Result<Vec<MediaStream>, MetadataError> {
        let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
        let mut dolby_vision: HashMap<i64, stream_dolby_vision::Model> =
            stream_dolby_vision::Entity::find()
                .filter(stream_dolby_vision::Column::StreamId.is_in(ids.clone()))
                .all(&self.connection)
                .await
                .map_err(storage)?
                .into_iter()
                .map(|row| (row.stream_id, row))
                .collect();
        let mut loudness: HashMap<i64, stream_loudness::Model> = stream_loudness::Entity::find()
            .filter(stream_loudness::Column::StreamId.is_in(ids))
            .all(&self.connection)
            .await
            .map_err(storage)?
            .into_iter()
            .map(|row| (row.stream_id, row))
            .collect();

        Ok(rows
            .into_iter()
            .map(|row| {
                let id = row.id;
                to_track(row, dolby_vision.remove(&id), loudness.remove(&id))
            })
            .collect())
    }

    /// The tags in scope that a query's items carry, each once, with no column selected.
    ///
    /// `IN` over the attachments rather than a join, which would return a tag once per item
    /// carrying it: Drama is on 2,416 films.
    fn tags_carried(query: &ItemQuery, scope: TagScope) -> Select<tag::Entity> {
        let items = Self::selection(query)
            .select_only()
            .column(metadata_item::Column::Id)
            .into_query();
        let mut attachments = Query::select()
            .column(item_tag::Column::TagId)
            .from(item_tag::Entity)
            .and_where(Expr::col(item_tag::Column::ItemId).in_subquery(items))
            .to_owned();
        if let Some(limit) = scope.billed_within {
            // Positions number an item's attachments of one kind from 0.
            attachments.and_where(Expr::col(item_tag::Column::Position).lt(limit));
        }
        tag::Entity::find()
            .select_only()
            .filter(tag::Column::Kind.eq(tag::kind_name(scope.kind)))
            .filter(tag::Column::Id.in_subquery(attachments))
    }

    /// The distinct values a query's items hold in a field, absent ones left out.
    fn values_held(query: &ItemQuery, field: ValueField) -> Select<metadata_item::Entity> {
        let column = value_column(field);
        Self::selection(query)
            .select_only()
            .column(column)
            .filter(column.is_not_null())
            .distinct()
    }
}

const fn value_column(field: ValueField) -> metadata_item::Column {
    match field {
        ValueField::Year => metadata_item::Column::Year,
        ValueField::ContentRating => metadata_item::Column::ContentRating,
        ValueField::Studio => metadata_item::Column::Studio,
    }
}

/// Years newest first, text in ascending order, as [`FieldValue`] promises.
const fn value_direction(field: ValueField) -> Direction {
    match field {
        ValueField::Year => Direction::Descending,
        ValueField::ContentRating | ValueField::Studio => Direction::Ascending,
    }
}

/// A version's frame in the range, as [`FrameRange::contains`] states it. A version of
/// unknown size compares as NULL and is in no range, there as here.
fn frame_condition(range: FrameRange) -> Condition {
    let width = || Expr::col((media::Entity, media::Column::Width));
    let height = || Expr::col((media::Entity, media::Column::Height));
    let mut condition = Condition::all()
        .add(width().is_not_null())
        .add(height().is_not_null());
    if let Some(bound) = range.reaching {
        condition = condition.add(
            Condition::any()
                .add(width().gte(bound.width))
                .add(height().gte(bound.height)),
        );
    }
    if let Some(bound) = range.short_of {
        condition = condition
            .add(width().lt(bound.width))
            .add(height().lt(bound.height));
    }
    condition
}

/// Whether the item carries one of the condition's tags.
///
/// An `EXISTS` for the same reason as the title words: a join would return an item once per
/// matching tag, and "a comedy or a drama" matches most comedies twice.
///
/// The wanted tags are resolved in their own, uncorrelated subquery, so the engine computes
/// that set once and then probes each item's attachments by primary key. Joining `tag`
/// inside the correlated part instead checks every tag of every item against the names:
/// measured at 300 to 500 ms for `genre=action` over 7,167 films, against 40 ms by id.
fn carries_tag(condition: &TagCondition) -> Condition {
    if condition.any_of.is_empty() {
        // An empty `OR` renders as no constraint at all, which would turn "carries none of
        // these" into "matches everything". The bindings refuse an empty condition; this
        // keeps storage safe if one gets through.
        return Condition::all().add(Expr::val(1).eq(0));
    }
    let mut which = Condition::any();
    for wanted in &condition.any_of {
        which = which.add(match wanted {
            TagRef::Id(id) => Expr::col((tag::Entity, tag::Column::Id)).eq(id.get()),
            // `LOWER` on both sides rather than lowercasing the value here, so the stored
            // name and the wanted one fold by the same engine's rules, whatever they are.
            TagRef::Name(name) => {
                Expr::expr(Func::lower(Expr::col((tag::Entity, tag::Column::Name))))
                    .eq(Func::lower(Expr::val(name.clone())))
            }
        });
    }

    let wanted = Query::select()
        .column((tag::Entity, tag::Column::Id))
        .from(tag::Entity)
        .and_where(Expr::col((tag::Entity, tag::Column::Kind)).eq(tag::kind_name(condition.kind)))
        .cond_where(which)
        .to_owned();

    Condition::all().add(Expr::exists(
        Query::select()
            .expr(Expr::val(1))
            .from(item_tag::Entity)
            .and_where(
                Expr::col((item_tag::Entity, item_tag::Column::ItemId))
                    .equals((metadata_item::Entity, metadata_item::Column::Id)),
            )
            .and_where(Expr::col((item_tag::Entity, item_tag::Column::TagId)).in_subquery(wanted))
            .to_owned(),
    ))
}

/// Whether the item has an indexed title word beginning with `prefix`.
///
/// An `EXISTS` rather than a join: an item has 4.3 words on average, and a join would return
/// it once per matching word.
/// Every word of the term must begin some word of the title.
///
/// The reference also wants them in order (`bad breaking` finds nothing), which the index
/// cannot tell: it holds each title's distinct words, without positions. So a reversed term
/// finds a little more here, the harmless direction for a search.
fn term_matches(term: &str) -> Condition {
    search::words(term).fold(Condition::all(), |all, word| all.add(word_exists(&word)))
}

fn word_exists(prefix: &str) -> Condition {
    Condition::all().add(Expr::exists(
        Query::select()
            .expr(Expr::val(1))
            .from(item_title_word::Entity)
            .and_where(
                Expr::col((item_title_word::Entity, item_title_word::Column::ItemId))
                    .equals((metadata_item::Entity, metadata_item::Column::Id)),
            )
            .cond_where(prefix_condition(
                Expr::col((item_title_word::Entity, item_title_word::Column::Word)),
                prefix,
            ))
            .to_owned(),
    ))
}

/// What the filter asks of the viewer's watch state, each condition reading its own rows.
fn watch_conditions(filter: &ItemFilter) -> Condition {
    let mut condition = Condition::all();
    let own_guid = Expr::col((metadata_item::Entity, metadata_item::Column::Guid));
    if let Some(watched) = filter.watched {
        let viewed = watch_state_where(filter.viewer, own_guid.clone(), viewed());
        condition = condition.add(holds(viewed, watched));
    }
    if let Some(in_progress) = filter.in_progress {
        let kept = watch_state_where(
            filter.viewer,
            own_guid,
            Expr::col((watch_state::Entity, watch_state::Column::OffsetMs)).is_not_null(),
        );
        condition = condition.add(holds(kept, in_progress));
    }
    if let Some(episodes_watched) = filter.episodes_watched {
        let unwatched =
            episode_below(|guid| watch_state_where(filter.viewer, guid, viewed()).not());
        condition = condition.add(holds(unwatched, !episodes_watched));
    }
    if let Some(episodes_started) = filter.episodes_started {
        let started = episode_below(|guid| {
            watch_state_where(
                filter.viewer,
                guid,
                viewed().or(
                    Expr::col((watch_state::Entity, watch_state::Column::OffsetMs)).is_not_null(),
                ),
            )
        });
        condition = condition.add(holds(started, episodes_started));
    }
    let last_viewed = || Expr::col((watch_state::Entity, watch_state::Column::LastViewedAt));
    if let Some(from) = filter.last_viewed_from {
        let guid = Expr::col((metadata_item::Entity, metadata_item::Column::Guid));
        condition = condition.add(watch_state_where(
            filter.viewer,
            guid,
            last_viewed().gte(from),
        ));
    }
    if let Some(to) = filter.last_viewed_to {
        let guid = Expr::col((metadata_item::Entity, metadata_item::Column::Guid));
        condition = condition.add(watch_state_where(
            filter.viewer,
            guid,
            last_viewed().lte(to),
        ));
    }
    condition
}

/// `condition`, or its negation.
fn holds(condition: SimpleExpr, expected: bool) -> SimpleExpr {
    if expected {
        condition
    } else {
        condition.not()
    }
}

/// Whether the viewer has a watch-state row for the work `guid` satisfying `predicate`.
///
/// A caller with no watch state has none, so the condition is false rather than absent:
/// "unwatched" then keeps everything and "watched" nothing.
fn watch_state_where(viewer: Option<Viewer>, guid: Expr, predicate: SimpleExpr) -> SimpleExpr {
    let Some(viewer) = viewer else {
        return Expr::value(1).eq(0);
    };
    Expr::exists(
        Query::select()
            .expr(Expr::val(1))
            .from(watch_state::Entity)
            .and_where(
                Expr::col((watch_state::Entity, watch_state::Column::Viewer)).eq(viewer_id(viewer)),
            )
            .and_where(Expr::col((watch_state::Entity, watch_state::Column::Guid)).eq(guid))
            .and_where(predicate)
            .to_owned(),
    )
}

/// A watch-state row's work was watched to the end at least once.
fn viewed() -> SimpleExpr {
    Expr::col((watch_state::Entity, watch_state::Column::ViewCount)).gt(0)
}

/// Whether the item in the row has an episode, as a child or a grandchild, for which
/// `condition` holds; `condition` is handed the episode's `guid`.
fn episode_below(condition: impl FnOnce(Expr) -> SimpleExpr) -> SimpleExpr {
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
    let holds = condition(Expr::col((episode.clone(), metadata_item::Column::Guid)));
    Expr::exists(
        Query::select()
            .expr(Expr::val(1))
            .from_as(metadata_item::Entity, episode.clone())
            .and_where(
                Expr::col((episode.clone(), metadata_item::Column::Kind))
                    .eq(kind_name(ItemKind::Episode)),
            )
            .cond_where(
                Condition::any()
                    .add(
                        Expr::col((episode.clone(), metadata_item::Column::ParentId))
                            .equals((metadata_item::Entity, metadata_item::Column::Id)),
                    )
                    .add(
                        Expr::col((episode, metadata_item::Column::ParentId)).in_subquery(seasons),
                    ),
            )
            .and_where(holds)
            .to_owned(),
    )
}

/// Wraps `text` in `LIKE` wildcards, escaping the ones it contains.
///
/// Without the escaping a studio name holding `%` would match everything, and `_` would
/// match any character. The default escape character is backslash on every engine we target,
/// so it has to be escaped first or it would escape the escapes.
fn contains_pattern(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

#[async_trait::async_trait]
impl MetadataRepository for SqlMetadataRepository {
    async fn items(
        &self,
        query: &ItemQuery,
        window: Window,
    ) -> Result<Page<Listed>, MetadataError> {
        let order = ItemOrder::new(query.sort, query.direction);
        let mut select = Self::selection(query);
        if let Some(after) = window.after.as_ref() {
            select = order.resume(select, after, true)?;
        }
        if let Some(before) = window.before.as_ref() {
            select = order.resume(select, before, false)?;
        }

        // `last:` reads from the far end; the items are put back in reading order below.
        let statement = order
            .sort(select, window.is_reversed())
            .offset(window.skip)
            .limit(window.fetch_limit())
            .build(self.connection.get_database_backend());
        let rows = self
            .connection
            .query_all(statement)
            .await
            .map_err(storage)?;
        let listed = rows
            .iter()
            .map(|row| {
                let model = metadata_item::Model::from_query_result(row, "")?;
                let cursor = order.cursor(row, &model)?;
                Ok(Listed {
                    item: to_domain(model),
                    cursor,
                })
            })
            .collect::<Result<Vec<_>, sea_orm::DbErr>>()
            .map_err(storage)?;

        let mut page = Page::from_overfetch(listed, &window);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn item_count(&self, query: &ItemQuery) -> Result<u64, MetadataError> {
        Self::selection(query)
            .count(&self.connection)
            .await
            .map_err(storage)
    }

    async fn search(
        &self,
        query: &SearchQuery,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        // A blank term matches nothing rather than everything: a client clearing its search
        // box must not be handed the whole server.
        if query.is_blank() {
            return Ok(Page {
                items: Vec::new(),
                has_previous: false,
                has_next: false,
            });
        }

        let mut select = metadata_item::Entity::find().filter(term_matches(&query.word_prefix));
        if !query.kinds.is_empty() {
            let kinds = query.kinds.iter().map(|kind| kind_name(*kind));
            select = select.filter(metadata_item::Column::Kind.is_in(kinds));
        }
        if let Some(sections) = &query.sections {
            let sections = sections.iter().map(|section| section.get());
            select = select.filter(metadata_item::Column::LibrarySectionId.is_in(sections));
        }

        // Title ordering, so the cursor is the one the listing already builds.
        let sort_column = metadata_item::Column::SortKey;
        if let Some(after) = window.after.as_ref() {
            select = select.filter(beyond(sort_column, after, Direction::Ascending, true)?);
        }
        if let Some(before) = window.before.as_ref() {
            select = select.filter(beyond(sort_column, before, Direction::Ascending, false)?);
        }

        let order = if window.is_reversed() {
            Order::Desc
        } else {
            Order::Asc
        };
        let rows = select
            .order_by(sort_column, order.clone())
            .order_by(metadata_item::Column::Id, order)
            .offset(window.skip)
            .limit(window.fetch_limit())
            .all(&self.connection)
            .await
            .map_err(storage)?;

        let mut page = Page::from_overfetch(rows, &window).map(to_domain);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn first_characters(
        &self,
        query: &ItemQuery,
    ) -> Result<Vec<FirstCharacter>, MetadataError> {
        // Grouped on the first stored character, then folded and merged here. Folding in SQL
        // would need a different expression per engine — and the fold has to be the same one
        // the search uses, which lives in `core`. The group-by returns a few hundred rows at
        // most, so the merge is free.
        #[derive(FromQueryResult)]
        struct Bucket {
            first: String,
            count: i64,
        }

        let rows = Self::selection(query)
            .select_only()
            .column_as(
                SimpleExpr::FunctionCall(Func::cust(Alias::new("substr")).args([
                    Expr::col(metadata_item::Column::SortKey).into(),
                    Expr::val(1).into(),
                    Expr::val(1).into(),
                ])),
                "first",
            )
            .column_as(metadata_item::Column::Id.count(), "count")
            .group_by(Expr::cust("1"))
            .into_model::<Bucket>()
            .all(&self.connection)
            .await
            .map_err(storage)?;

        let mut merged: BTreeMap<String, u64> = BTreeMap::new();
        for row in rows {
            *merged
                .entry(first_character_bucket(&row.first))
                .or_default() += u64::try_from(row.count).unwrap_or(0);
        }

        // `#` first, then the letters — the order the reference server lists them in.
        let mut buckets: Vec<FirstCharacter> = merged
            .into_iter()
            .map(|(character, count)| FirstCharacter { character, count })
            .collect();
        buckets.sort_by(|left, right| {
            let rank = |bucket: &FirstCharacter| u8::from(bucket.character != "#");
            rank(left)
                .cmp(&rank(right))
                .then_with(|| left.character.cmp(&right.character))
        });
        Ok(buckets)
    }

    async fn item(&self, id: RatingKey) -> Result<MetadataItem, MetadataError> {
        metadata_item::Entity::find_by_id(id.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(to_domain)
            .ok_or(MetadataError::ItemNotFound(id))
    }

    async fn items_by_id(&self, ids: &[RatingKey]) -> Result<Vec<MetadataItem>, MetadataError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<i64> = ids.iter().map(|id| id.get()).collect();
        let rows = metadata_item::Entity::find()
            .filter(metadata_item::Column::Id.is_in(keys))
            .all(&self.connection)
            .await
            .map_err(storage)?;
        Ok(rows.into_iter().map(to_domain).collect())
    }

    async fn children(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        let mut select =
            metadata_item::Entity::find().filter(metadata_item::Column::ParentId.eq(parent.get()));

        // Children are ordered by their number, so the cursor's tiebreaker is enough.
        if let Some(after) = window.after.as_ref() {
            select = select.filter(metadata_item::Column::Id.gt(after.id));
        }
        if let Some(before) = window.before.as_ref() {
            select = select.filter(metadata_item::Column::Id.lt(before.id));
        }

        let order = if window.is_reversed() {
            Order::Desc
        } else {
            Order::Asc
        };
        let rows = select
            .order_by(metadata_item::Column::Position, order.clone())
            .order_by(metadata_item::Column::Id, order)
            .offset(window.skip)
            .limit(window.fetch_limit())
            .all(&self.connection)
            .await
            .map_err(storage)?;

        let mut page = Page::from_overfetch(rows, &window).map(to_domain);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn grandchildren(
        &self,
        parent: RatingKey,
        window: Window,
    ) -> Result<Page<MetadataItem>, MetadataError> {
        // The middle level is joined in only to order by it and to reach the grandparent;
        // nothing of it is selected.
        let child = Alias::new("child");
        let mut select = metadata_item::Entity::find()
            .join_as(
                sea_orm::JoinType::InnerJoin,
                metadata_item::Relation::Parent.def(),
                child.clone(),
            )
            .filter(Expr::col((child.clone(), metadata_item::Column::ParentId)).eq(parent.get()));

        if let Some(after) = window.after.as_ref() {
            select = select.filter(metadata_item::Column::Id.gt(after.id));
        }
        if let Some(before) = window.before.as_ref() {
            select = select.filter(metadata_item::Column::Id.lt(before.id));
        }

        let order = if window.is_reversed() {
            Order::Desc
        } else {
            Order::Asc
        };
        let rows = select
            .order_by(
                Expr::col((child, metadata_item::Column::Position)),
                order.clone(),
            )
            .order_by(metadata_item::Column::Position, order.clone())
            .order_by(metadata_item::Column::Id, order)
            .offset(window.skip)
            .limit(window.fetch_limit())
            .all(&self.connection)
            .await
            .map_err(storage)?;

        let mut page = Page::from_overfetch(rows, &window).map(to_domain);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn child_counts(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError> {
        self.counts_by_parent(parents, false).await
    }

    async fn leaf_counts(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError> {
        self.counts_by_parent(parents, true).await
    }

    async fn versions_for(&self, items: &[RatingKey]) -> Result<Vec<MediaVersion>, MetadataError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<i64> = items.iter().map(|id| id.get()).collect();
        let rows = media::Entity::find()
            .filter(media::Column::MetadataItemId.is_in(keys))
            .order_by_asc(media::Column::Id)
            .all(&self.connection)
            .await
            .map_err(storage)?;

        Ok(rows.into_iter().map(to_version).collect())
    }

    async fn files_for(&self, versions: &[MediaId]) -> Result<Vec<MediaFile>, MetadataError> {
        if versions.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<i64> = versions.iter().map(|id| id.get()).collect();
        let rows = media_part::Entity::find()
            .filter(media_part::Column::MediaId.is_in(keys))
            .order_by_asc(media_part::Column::Position)
            .order_by_asc(media_part::Column::Id)
            .all(&self.connection)
            .await
            .map_err(storage)?;

        Ok(rows.into_iter().map(to_file).collect())
    }

    async fn streams_for(&self, files: &[PartId]) -> Result<Vec<MediaStream>, MetadataError> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<i64> = files.iter().map(|id| id.get()).collect();
        let rows = media_stream::Entity::find()
            .filter(media_stream::Column::MediaPartId.is_in(keys))
            .order_by_asc(media_stream::Column::Position)
            .all(&self.connection)
            .await
            .map_err(storage)?;

        self.with_analyses(rows).await
    }

    async fn version(&self, id: MediaId) -> Result<Option<MediaVersion>, MetadataError> {
        Ok(media::Entity::find_by_id(id.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(to_version))
    }

    async fn file(&self, id: PartId) -> Result<Option<MediaFile>, MetadataError> {
        Ok(media_part::Entity::find_by_id(id.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(to_file))
    }

    async fn stream(&self, id: StreamId) -> Result<Option<MediaStream>, MetadataError> {
        let rows = media_stream::Entity::find_by_id(id.get())
            .all(&self.connection)
            .await
            .map_err(storage)?;
        Ok(self.with_analyses(rows).await?.into_iter().next())
    }

    async fn tags_in(
        &self,
        query: &ItemQuery,
        scope: TagScope,
        window: Window,
    ) -> Result<Page<Tag>, MetadataError> {
        let mut select = Self::tags_carried(query, scope);
        let name = Expr::col((tag::Entity, tag::Column::Name));
        let id = Expr::col((tag::Entity, tag::Column::Id));
        for (cursor, after) in [(&window.after, true), (&window.before, false)] {
            if let Some(cursor) = cursor {
                select = select.filter(keyset(
                    name.clone(),
                    Some(id.clone()),
                    cursor,
                    Direction::Ascending,
                    after,
                )?);
            }
        }
        let order = if window.is_reversed() {
            Order::Desc
        } else {
            Order::Asc
        };
        let rows = with_tag_columns(select)
            .join(JoinType::LeftJoin, tag::Relation::Person.def())
            .order_by(tag::Column::Name, order.clone())
            .order_by(tag::Column::Id, order)
            .offset(window.skip)
            .limit(window.fetch_limit())
            .into_model::<TagRow>()
            .all(&self.connection)
            .await
            .map_err(storage)?;

        let mut page = Page::from_overfetch(rows, &window);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(Page {
            items: page
                .items
                .into_iter()
                .filter_map(TagRow::into_tag)
                .collect(),
            has_previous: page.has_previous,
            has_next: page.has_next,
        })
    }

    async fn tag_count_in(&self, query: &ItemQuery, scope: TagScope) -> Result<u64, MetadataError> {
        Self::tags_carried(query, scope)
            .column(tag::Column::Id)
            .count(&self.connection)
            .await
            .map_err(storage)
    }

    async fn values_in(
        &self,
        query: &ItemQuery,
        field: ValueField,
        window: Window,
    ) -> Result<Page<FieldValue>, MetadataError> {
        let column = value_column(field);
        let direction = value_direction(field);
        let mut select = Self::values_held(query, field);
        for (cursor, after) in [(&window.after, true), (&window.before, false)] {
            if let Some(cursor) = cursor {
                select = select.filter(keyset(
                    Expr::col((metadata_item::Entity, column)),
                    None,
                    cursor,
                    direction,
                    after,
                )?);
            }
        }
        let order = if direction.is_descending() == window.is_reversed() {
            Order::Asc
        } else {
            Order::Desc
        };
        let select = select
            .order_by(column, order)
            .offset(window.skip)
            .limit(window.fetch_limit());

        let values: Vec<FieldValue> = if field == ValueField::Year {
            select
                .into_tuple::<i32>()
                .all(&self.connection)
                .await
                .map_err(storage)?
                .into_iter()
                .map(FieldValue::Year)
                .collect()
        } else {
            select
                .into_tuple::<String>()
                .all(&self.connection)
                .await
                .map_err(storage)?
                .into_iter()
                .map(FieldValue::Text)
                .collect()
        };

        let mut page = Page::from_overfetch(values, &window);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn value_count_in(
        &self,
        query: &ItemQuery,
        field: ValueField,
    ) -> Result<u64, MetadataError> {
        Self::values_held(query, field)
            .count(&self.connection)
            .await
            .map_err(storage)
    }

    async fn descendants(&self, item: RatingKey) -> Result<Vec<MetadataItem>, MetadataError> {
        let children = Query::select()
            .column(metadata_item::Column::Id)
            .from(metadata_item::Entity)
            .and_where(metadata_item::Column::ParentId.eq(item.get()))
            .to_owned();
        let rows = metadata_item::Entity::find()
            .filter(
                Condition::any()
                    .add(metadata_item::Column::ParentId.eq(item.get()))
                    .add(metadata_item::Column::ParentId.in_subquery(children)),
            )
            .all(&self.connection)
            .await
            .map_err(storage)?;
        Ok(rows.into_iter().map(to_domain).collect())
    }

    async fn items_by_guid(
        &self,
        guids: &[String],
        sections: Option<&[SectionId]>,
    ) -> Result<Vec<MetadataItem>, MetadataError> {
        if guids.is_empty() {
            return Ok(Vec::new());
        }
        let mut select = metadata_item::Entity::find()
            .filter(metadata_item::Column::Guid.is_in(guids.iter().cloned()));
        if let Some(sections) = sections {
            select = select.filter(
                metadata_item::Column::LibrarySectionId.is_in(sections.iter().map(|id| id.get())),
            );
        }
        let rows = select.all(&self.connection).await.map_err(storage)?;
        Ok(rows.into_iter().map(to_domain).collect())
    }

    async fn tags_for(&self, items: &[RatingKey]) -> Result<Vec<ItemTag>, MetadataError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<i64> = items.iter().map(|id| id.get()).collect();
        let rows = with_tag_columns(item_tag::Entity::find().select_only())
            .column(item_tag::Column::ItemId)
            .column(item_tag::Column::Role)
            .join(JoinType::InnerJoin, item_tag::Relation::Tag.def())
            .join(JoinType::LeftJoin, tag::Relation::Person.def())
            .filter(item_tag::Column::ItemId.is_in(keys))
            .order_by_asc(item_tag::Column::ItemId)
            .order_by_asc(tag::Column::Kind)
            .order_by_asc(item_tag::Column::Position)
            .into_model::<AttachedRow>()
            .all(&self.connection)
            .await
            .map_err(storage)?;

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some(ItemTag {
                    item_id: RatingKey::new(row.item_id),
                    role: row.role.clone(),
                    tag: row.tag.into_tag()?,
                })
            })
            .collect())
    }

    async fn credits_below(
        &self,
        parents: &[RatingKey],
    ) -> Result<Vec<EpisodeCredit>, MetadataError> {
        if parents.is_empty() {
            return Ok(Vec::new());
        }
        // Seasons and series in one list, each matched at its own depth. An id is one or
        // the other, so no episode is counted twice.
        let mut credits = self.credits_at_depth(parents, false).await?;
        credits.extend(self.credits_at_depth(parents, true).await?);
        Ok(credits)
    }

    async fn external_ids_for(
        &self,
        items: &[RatingKey],
    ) -> Result<Vec<ExternalId>, MetadataError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<i64> = items.iter().map(|id| id.get()).collect();
        let rows = item_external_id::Entity::find()
            .filter(item_external_id::Column::ItemId.is_in(keys))
            .order_by_asc(item_external_id::Column::ItemId)
            .order_by_asc(item_external_id::Column::Position)
            .all(&self.connection)
            .await
            .map_err(storage)?;

        Ok(rows
            .into_iter()
            .map(|row| ExternalId {
                item_id: RatingKey::new(row.item_id),
                source: row.source,
                value: row.value,
            })
            .collect())
    }

    async fn ratings_for(&self, items: &[RatingKey]) -> Result<Vec<Rating>, MetadataError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<i64> = items.iter().map(|id| id.get()).collect();
        let rows = item_rating::Entity::find()
            .filter(item_rating::Column::ItemId.is_in(keys))
            .order_by_asc(item_rating::Column::ItemId)
            .order_by_asc(item_rating::Column::Position)
            .all(&self.connection)
            .await
            .map_err(storage)?;

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some(Rating {
                    item_id: RatingKey::new(row.item_id),
                    kind: item_rating::parse_kind(&row.kind)?,
                    source: row.source,
                    value: row.value,
                    image: row.image,
                })
            })
            .collect())
    }

    async fn tag(&self, id: TagId) -> Result<Option<Tag>, MetadataError> {
        Ok(
            with_tag_columns(tag::Entity::find_by_id(id.get()).select_only())
                .join(JoinType::LeftJoin, tag::Relation::Person.def())
                .into_model::<TagRow>()
                .one(&self.connection)
                .await
                .map_err(storage)?
                .and_then(TagRow::into_tag),
        )
    }

    async fn items_sharing_tags(
        &self,
        section: SectionId,
        kind: ItemKind,
        tags: &[TagId],
    ) -> Result<Vec<TagOverlap>, MetadataError> {
        if tags.is_empty() {
            return Ok(Vec::new());
        }
        let rows = item_tag::Entity::find()
            .select_only()
            .column(item_tag::Column::ItemId)
            .column(metadata_item::Column::Year)
            .column_as(item_tag::Column::TagId.count(), "shared")
            .join(JoinType::InnerJoin, item_tag::Relation::MetadataItem.def())
            .filter(item_tag::Column::TagId.is_in(tags.iter().map(|tag| tag.get())))
            .filter(metadata_item::Column::LibrarySectionId.eq(section.get()))
            .filter(metadata_item::Column::Kind.eq(kind_name(kind)))
            .group_by(item_tag::Column::ItemId)
            .into_model::<OverlapRow>()
            .all(&self.connection)
            .await
            .map_err(storage)?;
        Ok(rows
            .into_iter()
            .map(|row| TagOverlap {
                item: RatingKey::new(row.item_id),
                shared: u32::try_from(row.shared).unwrap_or(0),
                year: row.year,
            })
            .collect())
    }

    async fn episode_additions(
        &self,
        section: SectionId,
        after: Option<(i64, RatingKey)>,
        limit: u32,
    ) -> Result<Vec<EpisodeAddition>, MetadataError> {
        let season = Alias::new("season");
        let mut select = metadata_item::Entity::find()
            .select_only()
            .column_as(metadata_item::Column::Id, "episode")
            .column_as(metadata_item::Column::AddedAt, "added_at")
            .column_as(
                Expr::col((season.clone(), metadata_item::Column::Id)),
                "season",
            )
            .column_as(
                Expr::col((season.clone(), metadata_item::Column::ParentId)),
                "show",
            )
            .join_as(
                JoinType::InnerJoin,
                metadata_item::Relation::Parent.def(),
                season,
            )
            .filter(metadata_item::Column::LibrarySectionId.eq(section.get()))
            .filter(metadata_item::Column::Kind.eq(kind_name(ItemKind::Episode)));
        if let Some((added_at, episode)) = after {
            // Strictly after, in the newest-first order the index serves.
            select = select.filter(
                Condition::any()
                    .add(metadata_item::Column::AddedAt.lt(added_at))
                    .add(
                        Condition::all()
                            .add(metadata_item::Column::AddedAt.eq(added_at))
                            .add(metadata_item::Column::Id.lt(episode.get())),
                    ),
            );
        }
        let rows = select
            .order_by_desc(metadata_item::Column::AddedAt)
            .order_by_desc(metadata_item::Column::Id)
            .limit(u64::from(limit))
            .into_model::<AdditionRow>()
            .all(&self.connection)
            .await
            .map_err(storage)?;

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some(EpisodeAddition {
                    episode: RatingKey::new(row.episode),
                    season: RatingKey::new(row.season),
                    // A season with no series is an orphan the importer skips; reading one
                    // back would be a defect, and it is left out rather than guessed at.
                    show: RatingKey::new(row.show?),
                    added_at: row.added_at,
                })
            })
            .collect())
    }

    async fn person(&self, id: PersonId) -> Result<Option<Person>, MetadataError> {
        Ok(person::Entity::find_by_id(id.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(|row| Person {
                id: PersonId::new(row.id),
                name: row.name,
                provider_key: row.provider_key,
                thumb_url: row.thumb_url,
            }))
    }

    async fn locked_fields(
        &self,
        items: &[RatingKey],
    ) -> Result<Vec<(RatingKey, ItemField)>, MetadataError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let rows = item_field_lock::Entity::find()
            .filter(item_field_lock::Column::ItemId.is_in(items.iter().map(|id| id.get())))
            .all(&self.connection)
            .await
            .map_err(storage)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let field = write::parse_field_name(&row.field);
                if field.is_none() {
                    tracing::warn!(field = row.field, "unknown locked field in storage");
                }
                Some((RatingKey::new(row.item_id), field?))
            })
            .collect())
    }

    async fn edit_item(&self, id: RatingKey, edit: &ItemEdit) -> Result<Edited, MetadataError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let mut item = metadata_item::Entity::find_by_id(id.get())
            .one(&transaction)
            .await
            .map_err(storage)?
            .map(to_domain)
            .filter(|item| item.kind == edit.kind())
            .ok_or(MetadataError::ItemNotFound(id))?;
        let mut locked: BTreeSet<ItemField> = item_field_lock::Entity::find()
            .filter(item_field_lock::Column::ItemId.eq(id.get()))
            .all(&transaction)
            .await
            .map_err(storage)?
            .into_iter()
            .filter_map(|row| write::parse_field_name(&row.field))
            .collect();

        edit.apply(&mut item, &mut locked);
        write::update_item(&transaction, &item, &locked)
            .await
            .map_err(storage)?;
        transaction.commit().await.map_err(storage)?;
        Ok(Edited {
            item,
            locked: locked.into_iter().collect(),
        })
    }
}

/// Selects a tag and its person, as [`TagRow`] reads them back. The person's name is
/// aliased, or it would collide with the tag's.
fn with_tag_columns<E: EntityTrait>(select: Select<E>) -> Select<E> {
    select
        .column(tag::Column::Id)
        .column(tag::Column::Kind)
        .column(tag::Column::Name)
        .column(tag::Column::PersonId)
        .column_as(person::Column::Name, "person_name")
        .column(person::Column::ProviderKey)
        .column(person::Column::ThumbUrl)
}

/// A tag joined to its person, as a read selects it.
#[derive(Debug, FromQueryResult)]
struct TagRow {
    id: i64,
    kind: String,
    name: String,
    person_id: Option<i64>,
    person_name: Option<String>,
    provider_key: Option<String>,
    thumb_url: Option<String>,
}

impl TagRow {
    /// `None` for a kind this build does not know, which is skipped rather than guessed at:
    /// a genre presented as a director would be worse than a missing one.
    fn into_tag(self) -> Option<Tag> {
        let Some(kind) = tag::parse_kind(&self.kind) else {
            tracing::warn!(
                kind = self.kind,
                id = self.id,
                "unknown tag kind in storage, skipped"
            );
            return None;
        };
        let person = match (self.person_id, self.person_name) {
            (Some(id), Some(name)) => Some(Person {
                id: PersonId::new(id),
                name,
                provider_key: self.provider_key,
                thumb_url: self.thumb_url,
            }),
            _ => None,
        };
        Some(Tag {
            id: TagId::new(self.id),
            kind,
            name: self.name,
            person,
        })
    }
}

/// One episode addition, with the season and series it belongs to.
#[derive(Debug, FromQueryResult)]
struct AdditionRow {
    episode: i64,
    added_at: i64,
    season: i64,
    show: Option<i64>,
}

/// A tag as attached to an item.
#[derive(Debug, FromQueryResult)]
struct AttachedRow {
    item_id: i64,
    role: Option<String>,
    #[sea_orm(nested)]
    tag: TagRow,
}

/// A credit counted over the episodes below one parent.
#[derive(Debug, FromQueryResult)]
struct TalliedRow {
    parent: i64,
    episodes: i64,
    #[sea_orm(nested)]
    tag: TagRow,
}

/// One row of a grouped count.
#[derive(Debug, FromQueryResult)]
struct ParentCount {
    parent_id: i64,
    total: i64,
}

impl SqlMetadataRepository {
    /// The credits of the episodes directly below `parents` — seasons — or, when `deep`, two
    /// levels below — series.
    async fn credits_at_depth(
        &self,
        parents: &[RatingKey],
        deep: bool,
    ) -> Result<Vec<EpisodeCredit>, MetadataError> {
        let keys: Vec<i64> = parents.iter().map(|id| id.get()).collect();
        let season = Alias::new("season");
        let parent = if deep {
            Expr::col((season.clone(), metadata_item::Column::ParentId))
        } else {
            Expr::col((metadata_item::Entity, metadata_item::Column::ParentId))
        };
        let credit_kinds = [
            TagKind::Director,
            TagKind::Writer,
            TagKind::Actor,
            TagKind::Producer,
        ]
        .map(tag::kind_name);

        let mut select = with_tag_columns(metadata_item::Entity::find().select_only())
            .column_as(parent.clone(), "parent")
            .column_as(
                Expr::col((metadata_item::Entity, metadata_item::Column::Id)).count_distinct(),
                "episodes",
            )
            .join(JoinType::InnerJoin, metadata_item::Relation::ItemTag.def())
            .join(JoinType::InnerJoin, item_tag::Relation::Tag.def())
            .join(JoinType::LeftJoin, tag::Relation::Person.def())
            .filter(Expr::col((tag::Entity, tag::Column::Kind)).is_in(credit_kinds))
            .filter(parent.clone().is_in(keys))
            .group_by(parent.clone())
            .group_by(tag::Column::Id)
            .group_by(tag::Column::Kind)
            .group_by(tag::Column::Name)
            .group_by(tag::Column::PersonId)
            .group_by(person::Column::Name)
            .group_by(person::Column::ProviderKey)
            .group_by(person::Column::ThumbUrl);
        if deep {
            select = select.join_as(
                JoinType::InnerJoin,
                metadata_item::Relation::Parent.def(),
                season,
            );
        }

        let rows = select
            .order_by_asc(parent)
            .order_by_desc(Expr::col(Alias::new("episodes")))
            .order_by_asc(tag::Column::Name)
            .into_model::<TalliedRow>()
            .all(&self.connection)
            .await
            .map_err(storage)?;

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some(EpisodeCredit {
                    parent: RatingKey::new(row.parent),
                    episodes: u32::try_from(row.episodes).unwrap_or(u32::MAX),
                    tag: row.tag.into_tag()?,
                })
            })
            .collect())
    }

    /// Counts children, or grandchildren when `deep`.
    ///
    /// A show's episode count is two levels down, so it groups by the grandparent — one query
    /// either way, which is what makes the loader worth having.
    async fn counts_by_parent(
        &self,
        parents: &[RatingKey],
        deep: bool,
    ) -> Result<Vec<(RatingKey, u64)>, MetadataError> {
        if parents.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<i64> = parents.iter().map(|id| id.get()).collect();

        let rows = if deep {
            let parent = Alias::new("parent");
            metadata_item::Entity::find()
                .join_as(
                    sea_orm::JoinType::InnerJoin,
                    metadata_item::Relation::Parent.def(),
                    parent.clone(),
                )
                .select_only()
                .column_as(
                    Expr::col((parent.clone(), metadata_item::Column::ParentId)),
                    "parent_id",
                )
                .column_as(metadata_item::Column::Id.count(), "total")
                .filter(Expr::col((parent.clone(), metadata_item::Column::ParentId)).is_in(keys))
                .group_by(Expr::col((parent, metadata_item::Column::ParentId)))
                .into_model::<ParentCount>()
                .all(&self.connection)
                .await
        } else {
            metadata_item::Entity::find()
                .select_only()
                .column_as(metadata_item::Column::ParentId, "parent_id")
                .column_as(metadata_item::Column::Id.count(), "total")
                .filter(metadata_item::Column::ParentId.is_in(keys))
                .group_by(metadata_item::Column::ParentId)
                .into_model::<ParentCount>()
                .all(&self.connection)
                .await
        }
        .map_err(storage)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    RatingKey::new(row.parent_id),
                    u64::try_from(row.total).unwrap_or(0),
                )
            })
            .collect())
    }
}

/// Builds the half-open comparison that resumes from a cursor.
///
/// Keyset pagination needs `(sort, id) > (key, id)` rather than an offset, and SQL has no
/// portable row comparison, so it is written out: `sort > key OR (sort = key AND id > cid)`.
/// The tiebreaker uses the same operator as the sort key, which is what makes the ordering
/// total and keeps a row from being skipped or repeated between pages.
///
/// Which operator depends on both the sort direction and which bound this is:
///
/// | direction | bound | operator |
/// | --- | --- | --- |
/// | ascending | `after` | `>` |
/// | ascending | `before` | `<` |
/// | descending | `after` | `<` |
/// | descending | `before` | `>` |
fn beyond(
    sort_column: metadata_item::Column,
    cursor: &Cursor,
    direction: Direction,
    after: bool,
) -> Result<Condition, MetadataError> {
    keyset(
        Expr::col((metadata_item::Entity, sort_column)),
        Some(Expr::col((
            metadata_item::Entity,
            metadata_item::Column::Id,
        ))),
        cursor,
        direction,
        after,
    )
}

/// [`beyond`] over any ordering expression. With no tiebreaker the ordering key must be
/// unique, which it is in a list of distinct values.
fn keyset(
    column: Expr,
    id: Option<Expr>,
    cursor: &Cursor,
    direction: Direction,
    after: bool,
) -> Result<Condition, MetadataError> {
    // One value, present: anything else comes from another ordering.
    let value = match cursor.keys.as_slice() {
        [CursorKey::Text(text)] => Expr::value(text.clone()),
        [CursorKey::Int(number)] => Expr::value(*number),
        [CursorKey::Real(number)] => Expr::value(*number),
        _ => return Err(MetadataError::CursorMismatch),
    };

    let greater = direction.is_descending() != after;
    let beyond_key = if greater {
        column.clone().gt(value.clone())
    } else {
        column.clone().lt(value.clone())
    };
    let Some(id) = id else {
        return Ok(Condition::all().add(beyond_key));
    };
    let beyond_id = if greater {
        id.gt(cursor.id)
    } else {
        id.lt(cursor.id)
    };

    Ok(Condition::any()
        .add(beyond_key)
        .add(Condition::all().add(column.eq(value)).add(beyond_id)))
}

/// Turns a prefix search into a half-open range on the ordering column.
///
/// `LIKE 'jura%'` looks equivalent but is not: `LIKE` is case-insensitive by default on
/// `SQLite`, which stops it using a binary-collated index, so the query degrades to a scan —
/// measured at 20 ms over 10,773 rows. An explicit range seeks instead, and needs no wildcard
/// escaping because the user's text is never interpreted as a pattern.
///
/// The upper bound is the prefix with its last character incremented: everything starting
/// with `jura` sorts before `jurb`.
fn prefix_condition<T: Into<SimpleExpr>>(column: T, prefix: &str) -> Condition {
    let column: SimpleExpr = column.into();
    let Some((lower, upper)) = search::prefix_range(prefix) else {
        // An empty prefix constrains nothing, and expressing that as a range would exclude
        // every row instead.
        return Condition::all();
    };

    let condition = Condition::all().add(Expr::expr(column.clone()).gte(lower));
    match upper {
        Some(upper) => condition.add(Expr::expr(column).lt(upper)),
        // No successor exists, so the prefix is unbounded above and the lower bound is all
        // the constraint there is.
        None => condition,
    }
}

#[derive(Debug, FromQueryResult)]
struct OverlapRow {
    item_id: i64,
    year: Option<i32>,
    shared: i64,
}

const fn kind_name(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Movie => "movie",
        ItemKind::Show => "show",
        ItemKind::Season => "season",
        ItemKind::Episode => "episode",
    }
}

fn parse_kind(stored: &str) -> ItemKind {
    match stored {
        "show" => ItemKind::Show,
        "season" => ItemKind::Season,
        "episode" => ItemKind::Episode,
        "movie" => ItemKind::Movie,
        other => {
            tracing::warn!(
                kind = other,
                "unknown item kind in storage, treating as movie"
            );
            ItemKind::Movie
        }
    }
}

fn parse_stream_kind(stored: &str) -> StreamKind {
    match stored {
        "audio" => StreamKind::Audio,
        "subtitle" => StreamKind::Subtitle,
        "video" => StreamKind::Video,
        other => {
            tracing::warn!(
                kind = other,
                "unknown stream kind in storage, treating as video"
            );
            StreamKind::Video
        }
    }
}

fn to_domain(row: metadata_item::Model) -> MetadataItem {
    MetadataItem {
        id: RatingKey::new(row.id),
        section_id: SectionId::new(row.library_section_id),
        parent_id: row.parent_id.map(RatingKey::new),
        kind: parse_kind(&row.kind),
        guid: row.guid,
        title: row.title,
        title_sort: row.title_sort,
        original_title: row.original_title,
        summary: row.summary,
        tagline: row.tagline,
        studio: row.studio,
        content_rating: row.content_rating,
        content_rating_age: row.content_rating_age,
        position: row.position,
        year: row.year,
        released_on: row.released_on,
        duration_ms: row.duration_ms,
        rating: row.rating,
        audience_rating: row.audience_rating,
        added_at: row.added_at,
        updated_at: row.updated_at,
    }
}

fn to_version(row: media::Model) -> MediaVersion {
    MediaVersion {
        id: MediaId::new(row.id),
        item_id: RatingKey::new(row.metadata_item_id),
        width: row.width,
        height: row.height,
        aspect_ratio: row.aspect_ratio,
        duration_ms: row.duration_ms,
        bitrate: row.bitrate,
        container: row.container,
        video_codec: row.video_codec,
        audio_codec: row.audio_codec,
        audio_channels: row.audio_channels,
        frame_rate: row.frame_rate,
        interlaced: row.interlaced,
        optimized_for_streaming: row.optimized_for_streaming,
    }
}

fn to_file(row: media_part::Model) -> MediaFile {
    MediaFile {
        id: PartId::new(row.id),
        version_id: MediaId::new(row.media_id),
        position: row.position,
        path: row.path,
        size_bytes: row.size_bytes,
        duration_ms: row.duration_ms,
        layout: FileLayout {
            has_thumbnail: row.has_thumbnail,
            has_64bit_offsets: row.has_64bit_offsets,
            optimized_for_streaming: row.optimized_for_streaming,
            packet_length: row.packet_length,
            has_chapter_text_stream: row.has_chapter_text_stream,
        },
    }
}

fn to_track(
    row: media_stream::Model,
    dolby_vision: Option<stream_dolby_vision::Model>,
    loudness: Option<stream_loudness::Model>,
) -> MediaStream {
    let kind = parse_stream_kind(&row.kind);
    let video = (kind == StreamKind::Video).then(|| {
        Box::new(VideoAnalysis {
            width: row.width,
            height: row.height,
            coded_width: row.coded_width,
            coded_height: row.coded_height,
            frame_rate: row.frame_rate,
            level: row.level,
            ref_frames: row.ref_frames,
            scan_type: row.scan_type.as_deref().and_then(parse_scan_type),
            chroma_subsampling: row.chroma_subsampling.clone(),
            chroma_location: row.chroma_location.clone(),
            color: ColorDescription {
                range: row.color_range.clone(),
                space: row.color_space.clone(),
                primaries: row.color_primaries.clone(),
                transfer: row.color_transfer.clone(),
            },
            has_scaling_matrix: row.has_scaling_matrix,
            anamorphic: row.is_anamorphic,
            pixel_aspect_ratio: row.pixel_aspect_ratio.clone(),
            fourcc: row.fourcc.clone(),
            closed_captions: row.has_closed_captions,
            separate_fields: row.has_separate_fields,
            hdr10_plus: row.has_hdr10_plus,
            dolby_vision: dolby_vision.map(|record| DolbyVision {
                version: record.version,
                profile: record.profile,
                level: record.level,
                base_layer_compatibility: record.base_layer_compatibility,
                has_base_layer: record.has_base_layer,
                has_enhancement_layer: record.has_enhancement_layer,
                has_rpu: record.has_rpu,
            }),
        })
    });
    let audio = (kind == StreamKind::Audio).then(|| AudioAnalysis {
        sampling_rate: row.sampling_rate,
        channel_layout: row.channel_layout.clone(),
        loudness: loudness.map(|measure| Loudness {
            analysis_version: measure.analysis_version,
            integrated: measure.integrated,
            range: measure.loudness_range,
            true_peak: measure.true_peak,
            threshold: measure.threshold,
            gain_offset: measure.gain_offset,
        }),
    });
    let subtitle = (kind == StreamKind::Subtitle).then(|| SubtitleAnalysis {
        format: row.subtitle_format.clone(),
        embedded_in_video: row.is_embedded_in_video,
    });

    MediaStream {
        id: StreamId::new(row.id),
        file_id: PartId::new(row.media_part_id),
        kind,
        position: row.position,
        codec: row.codec,
        language: row.language,
        channels: row.channels,
        bitrate: row.bitrate,
        is_default: row.is_default,
        is_forced: row.is_forced,
        external_path: row.external_path,
        title: row.title,
        stream_identifier: row.stream_identifier,
        profile: row.profile,
        bit_depth: row.bit_depth,
        traits: TrackTraits {
            original: row.is_original,
            dub: row.is_dub,
            commentary: row.is_commentary,
            hearing_impaired: row.is_hearing_impaired,
            visual_impaired: row.is_visual_impaired,
            header_compression: row.has_header_compression,
        },
        video,
        audio,
        subtitle,
    }
}

/// `None` for a value this build does not know, rather than a guess.
fn parse_scan_type(stored: &str) -> Option<ScanType> {
    match stored {
        "progressive" => Some(ScanType::Progressive),
        "interlaced" => Some(ScanType::Interlaced),
        _ => None,
    }
}

fn storage(error: sea_orm::DbErr) -> MetadataError {
    MetadataError::Storage(Box::new(error))
}
