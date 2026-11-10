//! Storage tests for watch state: one row per viewer and work, the episodes counted below a
//! series or a season, and playbacks remembered for a while.
//!
//! Items are the reference's test library: Bull, one season of two episodes, and Soul.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::watch::{
    Viewer, Viewing, WatchRepository, WatchState, VIEWING_LIFETIME_SECONDS,
};
use open_plex_core::{RatingKey, UserId};
use open_plex_db::entity::{library_section, metadata_item};
use open_plex_db::write;
use open_plex_db::SqlWatchRepository;
use sea_orm::{ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

const SHOW: i64 = 577_987;
const SEASON: i64 = 577_988;
const EPISODE_1: i64 = 577_989;
const EPISODE_2: i64 = 577_990;
const NOW: i64 = 1_794_318_000;

fn guid(id: i64) -> String {
    format!("tv.plex.agents.none://{id}")
}

fn item(id: i64, kind: &str, parent: Option<i64>) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(28),
        parent_id: ActiveValue::Set(parent),
        kind: ActiveValue::Set(kind.to_owned()),
        guid: ActiveValue::Set(guid(id)),
        title: ActiveValue::Set(Some(format!("item {id}"))),
        title_sort: ActiveValue::Set(None),
        sort_key: ActiveValue::Set(format!("item {id}")),
        original_title: ActiveValue::Set(None),
        summary: ActiveValue::Set(None),
        tagline: ActiveValue::Set(None),
        studio: ActiveValue::Set(None),
        content_rating: ActiveValue::Set(None),
        content_rating_age: ActiveValue::Set(None),
        position: ActiveValue::Set(Some(1)),
        year: ActiveValue::Set(None),
        released_on: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(None),
        rating: ActiveValue::Set(None),
        audience_rating: ActiveValue::Set(None),
        added_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
    }
}

async fn repository() -> SqlWatchRepository {
    let connection = open_plex_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    seed(&connection).await;
    SqlWatchRepository::new(connection)
}

async fn seed(connection: &DatabaseConnection) {
    library_section::Entity::insert(library_section::ActiveModel {
        id: ActiveValue::Set(28),
        kind: ActiveValue::Set("show".to_owned()),
        title: ActiveValue::Set("open-plex tests".to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("fr-FR".to_owned()),
        uuid: ActiveValue::Set("uuid-28".to_owned()),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    })
    .exec(connection)
    .await
    .expect("section");
    write::insert_items(
        connection,
        vec![
            item(SHOW, "show", None),
            item(SEASON, "season", Some(SHOW)),
            item(EPISODE_1, "episode", Some(SEASON)),
            item(EPISODE_2, "episode", Some(SEASON)),
        ],
    )
    .await
    .expect("items");
}

fn watched(count: u32) -> WatchState {
    WatchState {
        view_count: count,
        offset_ms: None,
        last_viewed_at: Some(NOW),
    }
}

#[tokio::test]
async fn keeps_one_state_per_viewer_and_work() {
    let repository = repository().await;
    let alice = Viewer::User(UserId::new(7));
    repository
        .save(Viewer::Operator, &[(guid(EPISODE_1), watched(1))])
        .await
        .unwrap();
    repository
        .save(Viewer::Operator, &[(guid(EPISODE_1), watched(2))])
        .await
        .unwrap();

    let mine = repository
        .states(Viewer::Operator, &[guid(EPISODE_1), guid(EPISODE_2)])
        .await
        .unwrap();
    assert_eq!(mine, vec![(guid(EPISODE_1), watched(2))]);
    // Another viewer's state is its own.
    assert!(repository
        .states(alice, &[guid(EPISODE_1)])
        .await
        .unwrap()
        .is_empty());

    // An empty state is no row at all.
    repository
        .save(
            Viewer::Operator,
            &[(guid(EPISODE_1), WatchState::default())],
        )
        .await
        .unwrap();
    assert!(repository
        .states(Viewer::Operator, &[guid(EPISODE_1)])
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn counts_the_episodes_watched_below_a_season_and_a_series() {
    let repository = repository().await;
    let in_progress = WatchState {
        view_count: 0,
        offset_ms: Some(120_000),
        last_viewed_at: Some(NOW),
    };
    repository
        .save(
            Viewer::Operator,
            &[
                (guid(EPISODE_1), watched(3)),
                (guid(EPISODE_2), in_progress),
            ],
        )
        .await
        .unwrap();

    let mut counts = repository
        .watched_leaves(
            Viewer::Operator,
            &[RatingKey::new(SHOW), RatingKey::new(SEASON)],
        )
        .await
        .unwrap();
    counts.sort();
    // One episode watched, however many times; one in progress, which does not count.
    assert_eq!(
        counts,
        vec![(RatingKey::new(SHOW), 1), (RatingKey::new(SEASON), 1)]
    );
    assert!(repository
        .watched_leaves(Viewer::User(UserId::new(7)), &[RatingKey::new(SHOW)])
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn remembers_a_playback_until_it_expires() {
    let repository = repository().await;
    let viewing = Viewing {
        item: RatingKey::new(EPISODE_1),
        position_ms: 120_000,
        completed: false,
    };
    repository
        .remember(Viewer::Operator, "client:session", viewing, NOW)
        .await
        .unwrap();
    assert_eq!(
        repository
            .viewing(Viewer::Operator, "client:session", NOW + 10)
            .await
            .unwrap(),
        Some(viewing)
    );
    assert_eq!(
        repository
            .viewing(
                Viewer::Operator,
                "client:session",
                NOW + VIEWING_LIFETIME_SECONDS
            )
            .await
            .unwrap(),
        None
    );

    // A newer report replaces the older one.
    let later = Viewing {
        position_ms: 300_000,
        completed: true,
        ..viewing
    };
    repository
        .remember(Viewer::Operator, "client:session", later, NOW + 20)
        .await
        .unwrap();
    assert_eq!(
        repository
            .viewing(Viewer::Operator, "client:session", NOW + 30)
            .await
            .unwrap(),
        Some(later)
    );
}

#[tokio::test]
async fn a_track_choice_is_kept_per_viewer_and_file_and_an_empty_one_forgotten() {
    use open_plex_core::tracks::{SubtitleChoice, TrackChoice};
    use open_plex_core::{PartId, StreamId};

    let (repository, file) = repository_with_a_file().await;
    let off = TrackChoice {
        audio: Some(StreamId::new(7)),
        subtitle: Some(SubtitleChoice::Off),
    };
    repository
        .choose_tracks(Viewer::Operator, file, off)
        .await
        .unwrap();
    let chosen = TrackChoice {
        audio: None,
        subtitle: Some(SubtitleChoice::Track(StreamId::new(8))),
    };
    repository
        .choose_tracks(Viewer::User(open_plex_core::UserId::new(3)), file, chosen)
        .await
        .unwrap();

    let read = |viewer| {
        let repository = &repository;
        async move {
            repository
                .track_choices(viewer, &[file, PartId::new(404)])
                .await
                .unwrap()
        }
    };
    assert_eq!(read(Viewer::Operator).await, [(file, off)]);
    assert_eq!(
        read(Viewer::User(open_plex_core::UserId::new(3))).await,
        [(file, chosen)]
    );
    repository
        .choose_tracks(Viewer::Operator, file, TrackChoice::default())
        .await
        .unwrap();
    assert!(read(Viewer::Operator).await.is_empty());
}

/// The seeded library, with one file for an episode: a choice of tracks belongs to a file.
async fn repository_with_a_file() -> (SqlWatchRepository, open_plex_core::PartId) {
    use open_plex_db::entity::{media, media_part};

    let connection = open_plex_db::connect("sqlite::memory:")
        .await
        .expect("in-memory database");
    seed(&connection).await;
    let episode = metadata_item::Entity::find()
        .filter(metadata_item::Column::Kind.eq("episode"))
        .one(&connection)
        .await
        .unwrap()
        .expect("an episode")
        .id;
    media::Entity::insert(media::ActiveModel {
        id: ActiveValue::Set(1),
        metadata_item_id: ActiveValue::Set(episode),
        width: ActiveValue::Set(None),
        height: ActiveValue::Set(None),
        aspect_ratio: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(None),
        bitrate: ActiveValue::Set(None),
        container: ActiveValue::Set(None),
        video_codec: ActiveValue::Set(None),
        audio_codec: ActiveValue::Set(None),
        audio_channels: ActiveValue::Set(None),
        frame_rate: ActiveValue::Set(None),
        interlaced: ActiveValue::Set(false),
        optimized_for_streaming: ActiveValue::Set(false),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
    })
    .exec(&connection)
    .await
    .unwrap();
    media_part::Entity::insert(media_part::ActiveModel {
        id: ActiveValue::Set(5),
        media_id: ActiveValue::Set(1),
        position: ActiveValue::Set(0),
        path: ActiveValue::Set("e.mkv".to_owned()),
        size_bytes: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(None),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        has_thumbnail: ActiveValue::Set(false),
        has_64bit_offsets: ActiveValue::Set(None),
        optimized_for_streaming: ActiveValue::Set(None),
        packet_length: ActiveValue::Set(None),
        has_chapter_text_stream: ActiveValue::Set(false),
    })
    .exec(&connection)
    .await
    .unwrap();
    (
        SqlWatchRepository::new(connection),
        open_plex_core::PartId::new(5),
    )
}
