//! Extras: the trailers, deleted scenes, interviews and featurettes that come with a film, a
//! series, a season or an episode.
//!
//! An extra belongs to one item and is no item of a library: it is not listed, searched or
//! counted with them, and whoever reads its item reads it. On the reference nearly all of them
//! are streams a metadata provider supplies rather than files on the server's disk, 30,253 of
//! 30,260 here; a local extra is a file found beside its item.
//!
//! Its versions are [`MediaVersion`](crate::MediaVersion)s like an item's, with files and
//! tracks, and name the item the extra belongs to as theirs: that is what lets a version or a
//! file be refetched by whoever reads the item, and by nobody else.

use crate::{ExtraId, RatingKey};

/// What an extra is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExtraKind {
    Trailer,
    DeletedScene,
    Interview,
    MusicVideo,
    BehindTheScenes,
    Scene,
    LiveMusicVideo,
    LyricMusicVideo,
    Concert,
    Featurette,
    Short,
    Other,
}

impl ExtraKind {
    pub const ALL: [Self; 12] = [
        Self::Trailer,
        Self::DeletedScene,
        Self::Interview,
        Self::MusicVideo,
        Self::BehindTheScenes,
        Self::Scene,
        Self::LiveMusicVideo,
        Self::LyricMusicVideo,
        Self::Concert,
        Self::Featurette,
        Self::Short,
        Self::Other,
    ];
}

/// One extra of one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extra {
    pub id: ExtraId,
    /// The item it comes with.
    pub item: RatingKey,
    pub kind: ExtraKind,
    /// Where it stands among its item's extras: they are listed by it, then by id, an extra
    /// without one first, as the reference's SQL orders them.
    pub position: Option<i32>,
    /// Its identity at its source: the provider's asset, or the file's URL.
    pub guid: String,
    pub title: Option<String>,
    pub title_sort: Option<String>,
    pub summary: Option<String>,
    pub year: Option<i32>,
    /// The release date, as its midnight UTC in seconds since the Unix epoch.
    pub released_on: Option<i64>,
    pub duration_ms: Option<i64>,
    /// Unknown on 863 of the reference library's extras.
    pub added_at: Option<i64>,
    /// Never set on the provider's extras of the reference library, which were written once.
    pub updated_at: Option<i64>,
}

/// The trailer a client plays as its item's: the first of the trailers in the order extras
/// are listed in. `None` without a trailer, whatever other extras there are.
pub fn primary(extras: &[Extra]) -> Option<&Extra> {
    extras
        .iter()
        .filter(|extra| extra.kind == ExtraKind::Trailer)
        .min_by_key(|extra| (extra.position, extra.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extra(id: i64, kind: ExtraKind, position: Option<i32>) -> Extra {
        Extra {
            id: ExtraId::new(id),
            item: RatingKey::new(1),
            kind,
            position,
            guid: format!("iva://{id}"),
            title: None,
            title_sort: None,
            summary: None,
            year: None,
            released_on: None,
            duration_ms: None,
            added_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn the_primary_extra_is_the_first_trailer_by_position_then_id() {
        let extras = [
            extra(10, ExtraKind::BehindTheScenes, Some(1)),
            extra(12, ExtraKind::Trailer, Some(3)),
            extra(11, ExtraKind::Trailer, Some(2)),
            extra(9, ExtraKind::Trailer, Some(2)),
        ];
        assert_eq!(primary(&extras).map(|extra| extra.id.get()), Some(9));
    }

    #[test]
    fn a_trailer_without_a_position_comes_first() {
        let extras = [
            extra(9, ExtraKind::Trailer, Some(1)),
            extra(12, ExtraKind::Trailer, None),
        ];
        assert_eq!(primary(&extras).map(|extra| extra.id.get()), Some(12));
    }

    #[test]
    fn an_item_without_a_trailer_has_no_primary_extra() {
        let extras = [extra(10, ExtraKind::BehindTheScenes, Some(1))];
        assert_eq!(primary(&extras), None);
    }
}
