//! Storage tests for source metadata: a series matched through `match_library` with a provider
//! standing in for TVDB, its record stored once, and its items shown in their library's
//! language.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use klapz_core::edit::ItemField;
use klapz_core::language::LanguageTag;
use klapz_core::metadata::{ItemKind, TagKind};
use klapz_core::source_metadata::{
    match_library, AgentError, Candidate, Credit, ExternalId, Genre, MatchQuery, MetadataSource,
    MetadataSources, SourceRecord, Translation,
};
use klapz_core::SectionId;
use klapz_db::entity::{
    item_external_id, item_source, item_tag, library_section, metadata_item, person,
    source_metadata, tag, watch_state,
};
use klapz_db::write;
use klapz_db::SqlSourceMetadataRepository;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
};

const SECTION: i64 = 1;
const SHOW: i64 = 10;
const SEASON: i64 = 11;
const EPISODE: i64 = 12;
const NOW: i64 = 1_794_318_000;

fn item(
    id: i64,
    kind: &str,
    parent: Option<i64>,
    title: Option<&str>,
    position: i32,
) -> metadata_item::ActiveModel {
    metadata_item::ActiveModel {
        id: ActiveValue::Set(id),
        library_section_id: ActiveValue::Set(SECTION),
        parent_id: ActiveValue::Set(parent),
        kind: ActiveValue::Set(kind.to_owned()),
        guid: ActiveValue::Set(format!("local://{id}")),
        title: ActiveValue::Set(title.map(str::to_owned)),
        title_sort: ActiveValue::Set(None),
        sort_key: ActiveValue::Set(title.unwrap_or_default().to_lowercase()),
        original_title: ActiveValue::Set(None),
        summary: ActiveValue::Set(None),
        tagline: ActiveValue::Set(None),
        studio: ActiveValue::Set(None),
        content_rating: ActiveValue::Set(None),
        content_rating_age: ActiveValue::Set(None),
        position: ActiveValue::Set(Some(position)),
        year: ActiveValue::Set(None),
        released_on: ActiveValue::Set(None),
        duration_ms: ActiveValue::Set(None),
        rating: ActiveValue::Set(None),
        audience_rating: ActiveValue::Set(None),
        added_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        refreshed_at: ActiveValue::Set(None),
        chapter_source: ActiveValue::Set(None),
    }
}

async fn seeded() -> DatabaseConnection {
    let connection = klapz_db::connect("sqlite::memory:").await.unwrap();
    library_section::Entity::insert(library_section::ActiveModel {
        id: ActiveValue::Set(SECTION),
        kind: ActiveValue::Set("show".to_owned()),
        title: ActiveValue::Set("Series".to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("fr-FR".to_owned()),
        uuid: ActiveValue::Set("uuid-1".to_owned()),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    })
    .exec(&connection)
    .await
    .unwrap();
    write::insert_items(
        &connection,
        vec![
            item(SHOW, "show", None, Some("Fringe"), 1),
            item(SEASON, "season", Some(SHOW), None, 3),
            item(EPISODE, "episode", Some(SEASON), None, 19),
        ],
    )
    .await
    .unwrap();
    connection
}

fn text(language: &str, title: &str) -> Translation {
    Translation {
        language: LanguageTag::parse(language).unwrap(),
        title: Some(title.to_owned()),
        summary: None,
        tagline: None,
    }
}

fn record(
    kind: ItemKind,
    id: &str,
    position: Option<i32>,
    translations: Vec<Translation>,
) -> SourceRecord {
    SourceRecord {
        kind,
        agent: "tvdb".to_owned(),
        provider_id: id.to_owned(),
        position,
        original_language: LanguageTag::parse("en"),
        original_title: None,
        translations,
        year: Some(2008),
        released_on: None,
        duration_ms: None,
        studio: None,
        rating: None,
        certifications: Vec::new(),
        external_ids: Vec::new(),
        images: Vec::new(),
        genres: Vec::new(),
        countries: Vec::new(),
        credits: Vec::new(),
        ratings: Vec::new(),
        children: Vec::new(),
    }
}

/// TVDB's *Fringe*, reduced to the episode the library holds.
#[derive(Debug, Default)]
struct FakeTvdb {
    searches: AtomicUsize,
}

#[async_trait::async_trait]
impl MetadataSource for FakeTvdb {
    fn agent(&self) -> &'static str {
        "tvdb"
    }

    async fn search(&self, _query: &MatchQuery) -> Result<Vec<Candidate>, AgentError> {
        self.searches.fetch_add(1, Ordering::SeqCst);
        Ok(vec![Candidate {
            provider_id: "82066".to_owned(),
            names: vec!["Fringe".to_owned()],
            aliases: Vec::new(),
            year: Some(2008),
        }])
    }

    async fn fetch(&self, provider_id: &str) -> Result<SourceRecord, AgentError> {
        let mut episode = record(
            ItemKind::Episode,
            "3971331",
            Some(19),
            vec![
                text("en", "6B"),
                text("fr", "Qui veut la peau d'Olivia Dunham ?"),
            ],
        );
        episode.external_ids.push(ExternalId {
            source: "tvdb".to_owned(),
            value: "3971331".to_owned(),
        });
        let mut season = record(ItemKind::Season, "40000", Some(3), Vec::new());
        season.children.push(episode);
        let mut show = record(
            ItemKind::Show,
            provider_id,
            None,
            vec![text("en", "Fringe"), text("fr", "Fringe")],
        );
        show.external_ids = vec![
            ExternalId {
                source: "imdb".to_owned(),
                value: "tt1119644".to_owned(),
            },
            ExternalId {
                source: "imdb".to_owned(),
                value: "tt1119644".to_owned(),
            },
        ];
        show.genres = vec![Genre {
            names: vec![
                (
                    LanguageTag::parse("en").unwrap(),
                    "Science Fiction".to_owned(),
                ),
                (
                    LanguageTag::parse("fr").unwrap(),
                    "Science-Fiction".to_owned(),
                ),
            ],
        }];
        show.credits = vec![Credit {
            kind: TagKind::Actor,
            name: "Anna Torv".to_owned(),
            role: Some("Olivia Dunham".to_owned()),
            person_key: "tvdb:290366".to_owned(),
            photo: Some("https://artworks.thetvdb.com/anna.jpg".to_owned()),
        }];
        show.children.push(season);
        Ok(show)
    }
}

fn sources(tvdb: Arc<FakeTvdb>) -> MetadataSources {
    MetadataSources {
        films: None,
        series: Some(tvdb),
    }
}

async fn guid_of(connection: &DatabaseConnection, id: i64) -> String {
    metadata_item::Entity::find_by_id(id)
        .one(connection)
        .await
        .unwrap()
        .unwrap()
        .guid
}

#[tokio::test]
async fn shows_a_series_and_its_episodes_in_the_library_language() {
    let connection = seeded().await;
    let repository = SqlSourceMetadataRepository::new(connection.clone());
    let tvdb = Arc::new(FakeTvdb::default());

    let report = match_library(
        &repository,
        &sources(Arc::clone(&tvdb)),
        None,
        SectionId::new(SECTION),
        false,
        NOW,
    )
    .await
    .unwrap();

    assert_eq!(report.matched.len(), 1);
    assert_eq!(report.matched[0].children, 2);
    let episode = metadata_item::Entity::find_by_id(EPISODE)
        .one(&connection)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        episode.title.as_deref(),
        Some("Qui veut la peau d'Olivia Dunham ?")
    );
    assert_eq!(episode.guid, "klapz://episode/tvdb-3971331");
    assert_eq!(episode.refreshed_at, Some(NOW));
    assert_eq!(guid_of(&connection, SHOW).await, "klapz://show/tvdb-82066");
    let show_ids = item_external_id::Entity::find()
        .filter(item_external_id::Column::ItemId.eq(SHOW))
        .count(&connection)
        .await
        .unwrap();
    assert_eq!(show_ids, 2, "tvdb and imdb, the repeated imdb id once");
    assert_eq!(
        item_source::Entity::find()
            .count(&connection)
            .await
            .unwrap(),
        3
    );
}

#[tokio::test]
async fn a_refresh_fetches_the_same_record_again_without_searching() {
    let connection = seeded().await;
    let repository = SqlSourceMetadataRepository::new(connection.clone());
    let tvdb = Arc::new(FakeTvdb::default());
    let section = SectionId::new(SECTION);

    match_library(
        &repository,
        &sources(Arc::clone(&tvdb)),
        None,
        section,
        false,
        NOW,
    )
    .await
    .unwrap();
    let nothing_left = match_library(
        &repository,
        &sources(Arc::clone(&tvdb)),
        None,
        section,
        false,
        NOW,
    )
    .await
    .unwrap();
    let refreshed = match_library(
        &repository,
        &sources(Arc::clone(&tvdb)),
        None,
        section,
        true,
        NOW + 60,
    )
    .await
    .unwrap();

    assert!(nothing_left.matched.is_empty());
    assert_eq!(refreshed.matched.len(), 1);
    assert_eq!(tvdb.searches.load(Ordering::SeqCst), 1);
    assert_eq!(
        source_metadata::Entity::find()
            .count(&connection)
            .await
            .unwrap(),
        3
    );
}

#[tokio::test]
async fn a_locked_title_survives_matching() {
    let connection = seeded().await;
    write::insert_locks(&connection, [(EPISODE, ItemField::Title)])
        .await
        .unwrap();
    let repository = SqlSourceMetadataRepository::new(connection.clone());

    match_library(
        &repository,
        &sources(Arc::new(FakeTvdb::default())),
        None,
        SectionId::new(SECTION),
        false,
        NOW,
    )
    .await
    .unwrap();

    let episode = metadata_item::Entity::find_by_id(EPISODE)
        .one(&connection)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(episode.title, None);
}

#[tokio::test]
async fn viewers_keep_their_state_when_the_item_changes_identity() {
    let connection = seeded().await;
    watch_state::Entity::insert(watch_state::ActiveModel {
        viewer: ActiveValue::Set(7),
        guid: ActiveValue::Set(format!("local://{EPISODE}")),
        view_count: ActiveValue::Set(1),
        offset_ms: ActiveValue::Set(None),
        last_viewed_at: ActiveValue::Set(Some(NOW)),
    })
    .exec(&connection)
    .await
    .unwrap();
    let repository = SqlSourceMetadataRepository::new(connection.clone());

    match_library(
        &repository,
        &sources(Arc::new(FakeTvdb::default())),
        None,
        SectionId::new(SECTION),
        false,
        NOW,
    )
    .await
    .unwrap();

    let states = watch_state::Entity::find().all(&connection).await.unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].guid, "klapz://episode/tvdb-3971331");
}

#[tokio::test]
async fn copies_of_an_imported_work_keep_its_guid() {
    let connection = seeded().await;
    write::insert_items(
        &connection,
        vec![{
            let mut imported = item(99, "show", None, Some("Fringe"), 1);
            imported.guid = ActiveValue::Set("plex://show/5d9c086c46115600200aa2fe".to_owned());
            imported
        }],
    )
    .await
    .unwrap();
    item_external_id::Entity::insert(item_external_id::ActiveModel {
        item_id: ActiveValue::Set(99),
        source: ActiveValue::Set("tvdb".to_owned()),
        value: ActiveValue::Set("82066".to_owned()),
        position: ActiveValue::Set(0),
    })
    .exec(&connection)
    .await
    .unwrap();
    let repository = SqlSourceMetadataRepository::new(connection.clone());

    match_library(
        &repository,
        &sources(Arc::new(FakeTvdb::default())),
        None,
        SectionId::new(SECTION),
        false,
        NOW,
    )
    .await
    .unwrap();

    assert_eq!(
        guid_of(&connection, SHOW).await,
        "plex://show/5d9c086c46115600200aa2fe"
    );
}

#[tokio::test]
async fn credits_land_on_the_person_the_provider_names() {
    let connection = seeded().await;
    person::Entity::insert(person::ActiveModel {
        id: ActiveValue::Set(500),
        name: ActiveValue::Set("Anna Torv".to_owned()),
        provider_key: ActiveValue::Set(Some("tvdb:290366".to_owned())),
        thumb_url: ActiveValue::Set(None),
    })
    .exec(&connection)
    .await
    .unwrap();
    let repository = SqlSourceMetadataRepository::new(connection.clone());

    match_library(
        &repository,
        &sources(Arc::new(FakeTvdb::default())),
        None,
        SectionId::new(SECTION),
        false,
        NOW,
    )
    .await
    .unwrap();

    let tags = item_tag::Entity::find()
        .filter(item_tag::Column::ItemId.eq(SHOW))
        .find_also_related(tag::Entity)
        .all(&connection)
        .await
        .unwrap();
    let named: Vec<_> = tags
        .iter()
        .map(|(attached, tag)| {
            let tag = tag.as_ref().unwrap();
            (
                tag.kind.as_str(),
                tag.name.as_str(),
                attached.role.as_deref(),
                tag.person_id,
            )
        })
        .collect();
    assert_eq!(
        named,
        [
            ("genre", "Science-Fiction", None, None),
            ("actor", "Anna Torv", Some("Olivia Dunham"), Some(500)),
        ]
    );
    let anna = person::Entity::find_by_id(500)
        .one(&connection)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        anna.thumb_url.as_deref(),
        Some("https://artworks.thetvdb.com/anna.jpg")
    );
    assert_eq!(person::Entity::find().count(&connection).await.unwrap(), 1);
}
