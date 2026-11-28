//! Films and series found through the people a search term names.
//!
//! The reference fills a search's film or series shelf this way when no title answers the
//! term: `tom hanks` finds no title, and its film shelf lists Tom Hanks' films instead, each
//! with the credit it came through. Measured on `/hubs/search` over thirteen names, and six
//! limits for two of them:
//!
//! - **Who.** The first actor, and the first director, of the people the search lists for
//!   the term — ranked by how well the name answers it, then library by library in title
//!   order, the most credited first, and cut to the limit — whose library holds that kind.
//!   So `hanks` finds Tom Hanks' films, in *VF - Films*, and Colin Hanks' series, in *VF -
//!   Series*, where he is the most credited Hanks. At a limit of 3, the three Tom Hanks
//!   listed are in film libraries: no series is found.
//! - **What.** The `limit` items that person's tag was put on last in that library, of any
//!   kind, then those of the kind wanted: the actor's, then the director's not already
//!   listed, up to the limit. Plex credits a guest on the series as well as on the episode,
//!   so Colin Hanks' series come, but only from 100 on: his 30 newest credits are episodes.
//!   `nolan` with a limit of 100 lists Nolan North's 16 films and Christopher Nolan's 11;
//!   with a limit of 3, the three Nolan North films added last, and no director's.
//! - **Rank.** Then best first, scored as a title matched anywhere: the audience rating alone
//!   tells them apart.

use crate::library::{LibrarySection, SectionKind};
use crate::metadata::{
    ItemDepth, ItemFilter, ItemKind, ItemQuery, ItemSort, MetadataError, MetadataItem,
    MetadataRepository, TagCondition, TagKind, TagRef, TagSearch, TagUse,
};
use crate::page::{Direction, Window, MAX_ITEMS};
use crate::search::{self, Rung};
use crate::UserId;

/// An item found through a credit, with the credit and its score.
#[derive(Debug, Clone)]
pub struct CreditedMatch {
    pub item: MetadataItem,
    /// The person in the library they were found in.
    pub credit: TagUse,
    pub score: f64,
}

/// The roles a person is found in, in the order their works are gathered.
const ROLES: [TagKind; 2] = [TagKind::Actor, TagKind::Director];

/// Films or series, `kind`, found through the people `term` names, best first, at most
/// `limit`. `libraries` are the caller's; `viewer` whose watch state the items carry.
///
/// # Errors
///
/// Fails when storage does.
pub async fn search_credits(
    metadata: &dyn MetadataRepository,
    term: &str,
    kind: ItemKind,
    libraries: &[LibrarySection],
    viewer: Option<UserId>,
    limit: u32,
) -> Result<Vec<CreditedMatch>, MetadataError> {
    let holding = match kind {
        ItemKind::Movie => SectionKind::Movie,
        ItemKind::Show => SectionKind::Show,
        ItemKind::Season | ItemKind::Episode => return Ok(Vec::new()),
    };
    let limit = limit.min(MAX_ITEMS);
    if libraries.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let mut found: Vec<CreditedMatch> = Vec::new();
    for role in ROLES {
        if found.len() >= limit as usize {
            break;
        }
        let listed = listed_people(metadata, term, role, libraries, limit).await?;
        let Some(credit) = listed.into_iter().find(|credit| {
            libraries
                .iter()
                .any(|library| library.id == credit.section && library.kind == holding)
        }) else {
            continue;
        };
        let tagged = ItemQuery::new(credit.section)
            .filtered(ItemFilter {
                depth: ItemDepth::All,
                tags: vec![TagCondition {
                    kind: role,
                    any_of: vec![TagRef::Id(credit.tag.id)],
                }],
                viewer,
                ..ItemFilter::default()
            })
            .sorted_by(ItemSort::AddedAt, Direction::Descending);
        let newest = metadata.items(&tagged, Window::first(limit)).await?.items;
        for listed in newest {
            if found.len() >= limit as usize {
                break;
            }
            if listed.item.kind != kind || found.iter().any(|known| known.item.id == listed.item.id)
            {
                continue;
            }
            found.push(CreditedMatch {
                score: search::score(kind, Rung::Anywhere, listed.item.audience_rating),
                item: listed.item,
                credit: credit.clone(),
            });
        }
    }
    // Stable: equal scores keep the order they were gathered in, newest first.
    found.sort_by(|left, right| right.score.total_cmp(&left.score));
    Ok(found)
}

/// The people in `role` a search lists for `term`, in its order and cut to `limit`: by how
/// well the name answers the term, then by library title, then the most credited, then the
/// newest tag.
async fn listed_people(
    metadata: &dyn MetadataRepository,
    term: &str,
    role: TagKind,
    libraries: &[LibrarySection],
    limit: u32,
) -> Result<Vec<TagUse>, MetadataError> {
    let query = TagSearch {
        word_prefix: term.to_owned(),
        kinds: vec![role],
        sections: Some(libraries.iter().map(|library| library.id).collect()),
    };
    let mut uses = metadata
        .search_tags(&query, Window::first(MAX_ITEMS))
        .await?
        .items;
    uses.sort_by(|left, right| {
        search::tag_order(term, &listed(left, libraries), &listed(right, libraries))
    });
    uses.truncate(limit as usize);
    Ok(uses)
}

fn listed<'a>(credit: &'a TagUse, libraries: &'a [LibrarySection]) -> search::ListedTag<'a> {
    search::ListedTag {
        name: &credit.tag.name,
        library_title: libraries
            .iter()
            .find(|library| library.id == credit.section)
            .map_or("", |library| library.title.as_str()),
        count: credit.item_count,
        id: credit.tag.id.get(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::metadata::{ItemTag, Tag};
    use crate::testing::InMemoryMetadata;
    use crate::{RatingKey, SectionId, TagId};

    fn library(id: i64, kind: SectionKind, title: &str) -> LibrarySection {
        LibrarySection {
            id: SectionId::new(id),
            kind,
            title: title.to_owned(),
            title_sort: None,
            language: crate::language::LanguageTag::parse("fr-FR").unwrap(),
            uuid: format!("uuid-{id}"),
            created_at: 0,
            updated_at: 0,
            scanned_at: None,
            content_changed_at: 0,
            hidden: false,
        }
    }

    fn item(id: i64, section: i64, kind: ItemKind, added_at: i64, rating: f64) -> MetadataItem {
        MetadataItem {
            id: RatingKey::new(id),
            section_id: SectionId::new(section),
            parent_id: (kind == ItemKind::Episode).then(|| RatingKey::new(1)),
            kind,
            guid: format!("plex://item/{id}"),
            title: Some(format!("Item {id}")),
            title_sort: None,
            original_title: None,
            summary: None,
            tagline: None,
            studio: None,
            content_rating: None,
            content_rating_age: None,
            position: None,
            year: None,
            released_on: None,
            duration_ms: None,
            rating: None,
            audience_rating: Some(rating),
            added_at,
            updated_at: added_at,
            refreshed_at: None,
            chapter_source: None,
        }
    }

    fn credit(item: i64, tag: i64, kind: TagKind, name: &str) -> ItemTag {
        ItemTag {
            item_id: RatingKey::new(item),
            tag: Tag {
                id: TagId::new(tag),
                kind,
                name: name.to_owned(),
                person: None,
            },
            role: None,
        }
    }

    /// Shaped on `nolan`: Nolan North acts in three films, Christopher Nolan directs two,
    /// and in the series library Nolan North guests on an episode and on its series.
    fn nolan() -> (InMemoryMetadata, Vec<LibrarySection>) {
        let items = vec![
            item(10, 4, ItemKind::Movie, 300, 9.4),
            item(11, 4, ItemKind::Movie, 200, 8.7),
            item(12, 4, ItemKind::Movie, 100, 5.0),
            item(20, 4, ItemKind::Movie, 400, 9.1),
            item(21, 4, ItemKind::Movie, 50, 7.6),
            item(30, 5, ItemKind::Show, 10, 8.2),
            item(31, 5, ItemKind::Episode, 20, 7.0),
        ];
        let tags = vec![
            credit(10, 467_043, TagKind::Actor, "Nolan North"),
            credit(11, 467_043, TagKind::Actor, "Nolan North"),
            credit(12, 467_043, TagKind::Actor, "Nolan North"),
            credit(20, 553_828, TagKind::Director, "Christopher Nolan"),
            credit(21, 553_828, TagKind::Director, "Christopher Nolan"),
            credit(30, 467_043, TagKind::Actor, "Nolan North"),
            credit(31, 467_043, TagKind::Actor, "Nolan North"),
        ];
        let libraries = vec![
            library(4, SectionKind::Movie, "VF - Films"),
            library(5, SectionKind::Show, "VF - Series"),
        ];
        (InMemoryMetadata::new(items).with_tags(tags), libraries)
    }

    fn found(matches: &[CreditedMatch]) -> Vec<(i64, i64)> {
        matches
            .iter()
            .map(|found| (found.item.id.get(), found.credit.tag.id.get()))
            .collect()
    }

    async fn credited(libraries: &[LibrarySection], kind: ItemKind, limit: u32) -> Vec<(i64, i64)> {
        let (metadata, _) = nolan();
        let matches = search_credits(&metadata, "nolan", kind, libraries, None, limit)
            .await
            .unwrap();
        found(&matches)
    }

    #[tokio::test]
    async fn lists_the_actor_then_the_director_best_first() {
        let (metadata, libraries) = nolan();
        let matches = search_credits(&metadata, "nolan", ItemKind::Movie, &libraries, None, 100)
            .await
            .unwrap();
        assert_eq!(
            found(&matches),
            [
                (10, 467_043),
                (20, 553_828),
                (11, 467_043),
                (21, 553_828),
                (12, 467_043)
            ]
        );
        assert!((matches[0].score - 0.33094).abs() < 1e-12);
    }

    #[tokio::test]
    async fn the_limit_takes_the_newest_credits_and_leaves_no_room_for_the_director() {
        let (_, libraries) = nolan();
        // The two films Nolan North's tag was put on last, ranked: 10 (9.4) then 11 (8.7).
        assert_eq!(
            credited(&libraries, ItemKind::Movie, 2).await,
            [(10, 467_043), (11, 467_043)]
        );
    }

    #[tokio::test]
    async fn a_series_comes_from_a_series_library_among_the_newest_credits() {
        let (_, libraries) = nolan();
        assert_eq!(
            credited(&libraries, ItemKind::Show, 100).await,
            [(30, 467_043)]
        );
        // His newest credit there is the episode: a limit of one finds no series.
        assert!(credited(&libraries, ItemKind::Show, 1).await.is_empty());
        assert!(credited(&libraries, ItemKind::Episode, 100)
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn an_actor_past_the_listed_people_leaves_the_shelf_to_the_director() {
        let (_, libraries) = nolan();
        // Listed first by library title, the series library's Nolan North fills a list of
        // one: no actor is found in a film library, and the director fills the shelf.
        let libraries: Vec<LibrarySection> = libraries
            .into_iter()
            .map(|mut library| {
                if library.id == SectionId::new(5) {
                    "A - Series".clone_into(&mut library.title);
                }
                library
            })
            .collect();
        assert_eq!(
            credited(&libraries, ItemKind::Movie, 1).await,
            [(20, 553_828)]
        );
    }
}
