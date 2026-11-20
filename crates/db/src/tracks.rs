//! Filtering and browsing a library by what its items' tracks hold: codecs, channel layouts.

use klapz_core::metadata::{StreamKind, TrackCondition, TrackField};
use sea_orm::sea_query::{Expr, Query, SelectStatement};
use sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter, QuerySelect, QueryTrait, Select};

use crate::entity::{media, media_part, media_stream, metadata_item};

pub(crate) const fn stream_kind_name(kind: StreamKind) -> &'static str {
    match kind {
        StreamKind::Video => "video",
        StreamKind::Audio => "audio",
        StreamKind::Subtitle => "subtitle",
    }
}

pub(crate) const fn track_column(field: TrackField) -> media_stream::Column {
    match field {
        TrackField::AudioLayout => media_stream::Column::ChannelLayout,
        TrackField::VideoCodec | TrackField::AudioCodec | TrackField::SubtitleCodec => {
            media_stream::Column::Codec
        }
        TrackField::AudioLanguage | TrackField::SubtitleLanguage => {
            media_stream::Column::DeclaredLanguage
        }
    }
}

/// The files of the versions a subquery returns, as a subquery of file ids.
///
/// `IN`s rather than joins, for the reason the resolution filter is an `EXISTS`: a join
/// returns an item once per file, and a value once per track holding it.
fn files_of(versions: SelectStatement) -> SelectStatement {
    Query::select()
        .column((media_part::Entity, media_part::Column::Id))
        .from(media_part::Entity)
        .and_where(
            Expr::col((media_part::Entity, media_part::Column::MediaId)).in_subquery(versions),
        )
        .to_owned()
}

/// The versions of the items whose ids `items` returns.
fn versions_of(items: SelectStatement) -> SelectStatement {
    Query::select()
        .column((media::Entity, media::Column::Id))
        .from(media::Entity)
        .and_where(Expr::col((media::Entity, media::Column::MetadataItemId)).in_subquery(items))
        .to_owned()
}

/// Whether the item has a track of the condition's kind holding one of its values, or none.
pub(crate) fn has_track(condition: &TrackCondition) -> Condition {
    // Correlated on the outer item.
    let versions = Query::select()
        .column((media::Entity, media::Column::Id))
        .from(media::Entity)
        .and_where(
            Expr::col((media::Entity, media::Column::MetadataItemId))
                .equals((metadata_item::Entity, metadata_item::Column::Id)),
        )
        .to_owned();
    let files = files_of(versions);
    let column = track_column(condition.field);
    let exists = Expr::exists(
        Query::select()
            .expr(Expr::val(1))
            .from(media_stream::Entity)
            .and_where(
                Expr::col((media_stream::Entity, media_stream::Column::MediaPartId))
                    .in_subquery(files),
            )
            .and_where(
                Expr::col((media_stream::Entity, media_stream::Column::Kind))
                    .eq(stream_kind_name(condition.field.kind())),
            )
            .and_where(
                Expr::col((media_stream::Entity, column)).is_in(condition.any_of.iter().cloned()),
            )
            .to_owned(),
    );
    let condition = if condition.negated {
        exists.not()
    } else {
        exists
    };
    Condition::all().add(condition)
}

/// The distinct values the tracks of a selection's items hold in a field, absent ones left
/// out, with no ordering.
pub(crate) fn values_held(
    items: Select<metadata_item::Entity>,
    field: TrackField,
) -> Select<media_stream::Entity> {
    let column = track_column(field);
    let items = items
        .select_only()
        .column(metadata_item::Column::Id)
        .into_query();
    media_stream::Entity::find()
        .select_only()
        .column(column)
        .filter(media_stream::Column::Kind.eq(stream_kind_name(field.kind())))
        .filter(column.is_not_null())
        .filter(media_stream::Column::MediaPartId.in_subquery(files_of(versions_of(items))))
        .distinct()
}
