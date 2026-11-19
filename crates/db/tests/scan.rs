//! Storage tests for what a library scan writes, read back through the ports every surface
//! reads with.
//!
//! The analyses are the scanner's own reports on the reference's test files (`fixtures/scan`),
//! deep ones as a scan makes them, and the values asserted are the ones
//! `docs/scanner/analysis.md` measured the reference storing for those same files.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::float_cmp)]

use open_plex_core::ids::{PartId, RatingKey, SectionId};
use open_plex_core::metadata::{
    ItemKind, ItemQuery, MediaStream, MetadataRepository, SearchQuery, StreamKind,
};
use open_plex_core::page::Window;
use open_plex_core::scan::{NewItem, NewMedia, NewPart, NewSidecar, ScanRepository, ShowKey};
use open_plex_db::entity::{
    item_marker, item_title_word, library_section, media, media_stream, metadata_item,
    subtitle_offset, track_choice,
};
use open_plex_db::{SqlMetadataRepository, SqlScanRepository};
use open_plex_scanner::report::{Analysis, Output, Report};
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
};

const FILMS: SectionId = SectionId::new(1);
const SERIES: SectionId = SectionId::new(2);
const NOW: i64 = 1_763_400_000;

fn probe(name: &str) -> Analysis {
    let path = format!(
        "{}/tests/fixtures/scan/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let report: Report = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    match report.output {
        Output::Probe(analysis) => analysis,
        other => panic!("not a probe report: {other:?}"),
    }
}

fn section(id: SectionId, kind: &str) -> library_section::ActiveModel {
    library_section::ActiveModel {
        id: ActiveValue::Set(id.get()),
        kind: ActiveValue::Set(kind.to_owned()),
        title: ActiveValue::Set(kind.to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("fr-FR".to_owned()),
        uuid: ActiveValue::Set(format!("uuid-{}", id.get())),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(41),
        hidden: ActiveValue::Set(false),
    }
}

async fn seeded() -> (DatabaseConnection, SqlScanRepository, SqlMetadataRepository) {
    let connection = open_plex_db::connect("sqlite::memory:").await.unwrap();
    library_section::Entity::insert_many([section(FILMS, "movie"), section(SERIES, "show")])
        .exec(&connection)
        .await
        .unwrap();
    (
        connection.clone(),
        SqlScanRepository::new(connection.clone()),
        SqlMetadataRepository::new(connection),
    )
}

fn part(path: &str, analysis: Option<Analysis>, sidecars: Vec<NewSidecar>) -> NewPart {
    let (size, modified_at) = analysis
        .as_ref()
        .map_or((1_000, 1_600_000_000), |analysis| {
            (analysis.file.size, analysis.file.modified_at)
        });
    NewPart {
        path: path.to_owned(),
        size,
        modified_at,
        analysis,
        sidecars,
    }
}

fn soul() -> NewItem {
    soul_with(probe("soul"))
}

fn soul_with(analysis: Analysis) -> NewItem {
    NewItem::Movie {
        title: "Soul".to_owned(),
        year: Some(2020),
        versions: vec![NewMedia {
            parts: vec![part(
                "C:\\Films\\Soul (2020)\\Soul.mkv",
                Some(analysis),
                vec![NewSidecar {
                    path: "C:\\Films\\Soul (2020)\\Soul.fre.forced.srt".to_owned(),
                    codec: "srt".to_owned(),
                    language: Some("fre".to_owned()),
                    forced: true,
                    hearing_impaired: false,
                }],
            )],
        }],
    }
}

fn bull(episode: i32, analysis: Option<Analysis>) -> NewItem {
    NewItem::Episode {
        show: ShowKey {
            title: "Bull".to_owned(),
            year: Some(2016),
        },
        season: 5,
        episode,
        released_on: None,
        versions: vec![NewMedia {
            parts: vec![part(
                &format!("/srv/series/Bull (2016)/Season 05/Bull S05E0{episode}.mkv"),
                analysis,
                Vec::new(),
            )],
        }],
    }
}

async fn tracks_of(metadata: &SqlMetadataRepository, item: RatingKey) -> Vec<MediaStream> {
    let versions = metadata.versions_for(&[item]).await.unwrap();
    let ids: Vec<_> = versions.iter().map(|version| version.id).collect();
    let files = metadata.files_for(&ids).await.unwrap();
    let ids: Vec<_> = files.iter().map(|file| file.id).collect();
    let mut tracks = metadata.streams_for(&ids).await.unwrap();
    tracks.sort_by_key(|track| track.id.get());
    tracks
}

#[tokio::test]
async fn a_scanned_film_reads_back_as_the_reference_stored_it() {
    let (_, scan, metadata) = seeded().await;
    let key = scan.add_item(FILMS, soul(), NOW).await.unwrap();

    let item = metadata.item(key).await.unwrap();
    assert_eq!(item.kind, ItemKind::Movie);
    assert_eq!(item.title.as_deref(), Some("Soul"));
    assert_eq!(item.year, Some(2020));
    assert_eq!(item.guid, format!("local://{}", key.get()));
    assert_eq!(item.added_at, NOW);
    // The packets' span, which the deep analysis stores over the header's 6031568.
    assert_eq!(item.duration_ms, Some(6_031_567));

    let listed = metadata
        .items(
            &ItemQuery::new(FILMS),
            Window::new(None, None, Some(100), None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.items.len(), 1);
    let found = metadata
        .search(
            &SearchQuery::new("soul"),
            Window::new(None, None, Some(100), None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(found.items[0].id, key);

    let versions = metadata.versions_for(&[key]).await.unwrap();
    let [version] = versions.as_slice() else {
        panic!("one version expected: {versions:?}")
    };
    assert_eq!(version.container.as_deref(), Some("mkv"));
    // Every packet's bytes over the 6031.567 s they span, as the deep analysis measured it.
    assert_eq!(version.bitrate, Some(7_723_977));
    assert_eq!(version.duration_ms, Some(6_031_567));
    assert_eq!((version.width, version.height), (Some(1920), Some(1080)));
    assert_eq!(version.aspect_ratio, Some(1.777_777_791_023_254_4));
    assert_eq!(version.frame_rate, Some(23.976_024_627_685_547));
    assert_eq!(version.video_codec.as_deref(), Some("h264"));
    assert_eq!(version.audio_codec.as_deref(), Some("eac3"));
    assert_eq!(version.audio_channels, Some(6));
    assert!(!version.interlaced);

    let files = metadata.files_for(&[version.id]).await.unwrap();
    assert_eq!(files[0].size_bytes, Some(5_825_269_622));
    assert_eq!(
        files[0].open_subtitles_hash.as_deref(),
        Some("eaa2ddfe303480ac")
    );
}

#[tokio::test]
async fn a_scanned_films_tracks_carry_the_reference_vocabulary() {
    let (_, scan, metadata) = seeded().await;
    let key = scan.add_item(FILMS, soul(), NOW).await.unwrap();
    let tracks = tracks_of(&metadata, key).await;
    let kinds: Vec<_> = tracks.iter().map(|track| track.kind).collect();
    assert_eq!(
        kinds,
        [
            StreamKind::Audio,
            StreamKind::Audio,
            StreamKind::Subtitle,
            StreamKind::Subtitle,
            StreamKind::Subtitle,
            StreamKind::Video,
            StreamKind::Subtitle,
        ]
    );

    let video = &tracks[5];
    assert_eq!(video.position, 5);
    assert_eq!(video.bitrate, Some(6_699_911));
    // A Matroska video track without a language is Matroska's default.
    assert_eq!(video.language.as_deref(), Some("en"));
    assert_eq!(video.profile.as_deref(), Some("high"));
    assert_eq!(video.bit_depth, Some(8));
    let analysis = video.video.as_deref().unwrap();
    assert_eq!(analysis.level, Some(40));
    assert_eq!(analysis.frame_rate, Some(23.976));
    assert_eq!(analysis.chroma_subsampling.as_deref(), Some("4:2:0"));
    assert_eq!(analysis.chroma_location.as_deref(), Some("left"));
    assert_eq!(analysis.color.range.as_deref(), Some("tv"));
    assert_eq!(analysis.color.transfer.as_deref(), Some("bt709"));
    assert_eq!(analysis.pixel_aspect_ratio, None);
    // From the sequence header, where ffprobe says 1080 and one reference frame.
    assert_eq!(
        (analysis.coded_width, analysis.coded_height),
        (Some(1920), Some(1088))
    );
    assert_eq!(analysis.ref_frames, Some(4));
    assert_eq!(analysis.has_scaling_matrix, Some(false));

    let french = &tracks[0];
    assert_eq!(french.codec.as_deref(), Some("eac3"));
    assert_eq!(french.language.as_deref(), Some("fr"));
    assert_eq!(french.bitrate, Some(255_977));
    assert_eq!(french.channels, Some(6));
    assert!(french.is_default);
    let sound = french.audio.as_ref().unwrap();
    assert_eq!(sound.channel_layout.as_deref(), Some("5.1(side)"));
    assert_eq!(sound.sampling_rate, Some(48_000));
    let english = &tracks[1];
    assert_eq!(english.language.as_deref(), Some("en"));
    assert_eq!(english.bitrate, Some(767_936));
    assert_eq!(
        english.profile.as_deref(),
        Some("dolby digital plus + dolby atmos")
    );

    let forced = &tracks[2];
    assert_eq!(forced.codec.as_deref(), Some("srt"));
    assert_eq!(forced.language.as_deref(), Some("fr"));
    assert_eq!(forced.title.as_deref(), Some("French (Forced)"));
    assert!(forced.is_forced && forced.is_default);
    // Ten packets, 318 bytes: a measured 0, which the reference stores as no rate.
    assert_eq!(forced.bitrate, None);
    assert_eq!(tracks[3].bitrate, Some(69));

    let sidecar = &tracks[6];
    assert_eq!(
        sidecar.external_path.as_deref(),
        Some("file:///C:/Films/Soul%20(2020)/Soul.fre.forced.srt")
    );
    assert_eq!(sidecar.codec.as_deref(), Some("srt"));
    assert_eq!(sidecar.language.as_deref(), Some("fr"));
    assert!(sidecar.is_forced);
    assert_eq!(
        sidecar.subtitle.as_ref().unwrap().format.as_deref(),
        Some("srt")
    );
    assert_eq!(
        open_plex_core::sidecar::Sidecar::of(sidecar)
            .unwrap()
            .path(),
        Some("C:\\Films\\Soul (2020)\\Soul.fre.forced.srt")
    );
}

#[tokio::test]
async fn episodes_share_the_show_and_season_they_make() {
    let (connection, scan, metadata) = seeded().await;
    let first = scan
        .add_item(SERIES, bull(1, Some(probe("bull_s05e01"))), NOW)
        .await
        .unwrap();
    let second = scan.add_item(SERIES, bull(2, None), NOW).await.unwrap();

    let first = metadata.item(first).await.unwrap();
    let second = metadata.item(second).await.unwrap();
    assert_eq!(first.parent_id, second.parent_id);
    assert_eq!((first.position, second.position), (Some(1), Some(2)));
    assert_eq!(first.year, Some(2016));
    let season = metadata.item(first.parent_id.unwrap()).await.unwrap();
    assert_eq!(season.kind, ItemKind::Season);
    assert_eq!(season.position, Some(5));
    assert_eq!(season.title, None);
    let show = metadata.item(season.parent_id.unwrap()).await.unwrap();
    assert_eq!(show.kind, ItemKind::Show);
    assert_eq!(show.title.as_deref(), Some("Bull"));
    assert_eq!(show.year, Some(2016));
    assert_eq!(
        metadata_item::Entity::find()
            .count(&connection)
            .await
            .unwrap(),
        4
    );

    let listed = metadata
        .items(
            &ItemQuery::new(SERIES),
            Window::new(None, None, Some(100), None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].item.id, show.id);

    let tracks = tracks_of(&metadata, first.id).await;
    assert_eq!(tracks[0].bitrate, Some(3_110_461));
    let video = tracks[0].video.as_deref().unwrap();
    assert_eq!(video.frame_rate, Some(25.0));
    assert_eq!(
        (video.coded_width, video.coded_height, video.ref_frames),
        (Some(1280), Some(720), Some(2))
    );
    assert_eq!(tracks[1].bitrate, Some(96_000));
    assert_eq!(tracks[1].profile.as_deref(), Some("lc"));
    assert_eq!(tracks[2].bitrate, Some(76));

    // An unanalysed file is listed with its file and no track.
    let versions = metadata.versions_for(&[second.id]).await.unwrap();
    assert_eq!(versions[0].container, None);
    let files = metadata.files_for(&[versions[0].id]).await.unwrap();
    assert_eq!(files[0].size_bytes, Some(1_000));
    assert!(tracks_of(&metadata, second.id).await.is_empty());
}

/// The rates the reference stores before its deep analysis runs, which a scan falls back on
/// when its own cannot finish.
#[tokio::test]
async fn without_a_deep_analysis_rates_are_the_declared_ones() {
    let (_, scan, metadata) = seeded().await;
    let mut analysis = probe("soul");
    analysis.deep = None;
    let film = scan
        .add_item(FILMS, soul_with(analysis), NOW)
        .await
        .unwrap();
    let versions = metadata.versions_for(&[film]).await.unwrap();
    assert_eq!(versions[0].bitrate, Some(7_726_375));
    assert_eq!(versions[0].duration_ms, Some(6_031_568));
    let tracks = tracks_of(&metadata, film).await;
    // The container's rate less the audio's declared ones.
    assert_eq!(tracks[5].bitrate, Some(6_702_375));
    assert_eq!(tracks[0].bitrate, Some(256_000));
    // Soul's subtitles are tagged `BPS-eng`, which the reference does not read.
    assert_eq!(tracks[3].bitrate, None);

    let mut analysis = probe("bull_s05e01");
    analysis.deep = None;
    let episode = scan
        .add_item(SERIES, bull(1, Some(analysis)), NOW)
        .await
        .unwrap();
    // Bull's AAC declares no rate in its header: the video keeps the container's, the audio
    // and subtitle the rates their muxer tagged.
    let tracks = tracks_of(&metadata, episode).await;
    assert_eq!(tracks[0].bitrate, Some(3_208_969));
    assert_eq!(tracks[1].bitrate, Some(96_000));
    assert_eq!(tracks[2].bitrate, Some(77));
}

fn film_at(path: &str, analysis: Analysis) -> NewItem {
    NewItem::Movie {
        title: "Le Film".to_owned(),
        year: Some(2020),
        versions: vec![NewMedia {
            parts: vec![part(path, Some(analysis), Vec::new())],
        }],
    }
}

fn untagged_audio(mut analysis: Analysis) -> Analysis {
    for track in &mut analysis.tracks {
        if track.kind == open_plex_scanner::report::TrackKind::Audio {
            track.language = None;
        }
    }
    analysis
}

#[tokio::test]
async fn a_lone_untagged_audio_track_takes_the_dub_its_file_name_gives() {
    let (_, scan, metadata) = seeded().await;
    let quebec = scan
        .add_item(
            FILMS,
            film_at(
                "D:\\Films\\Le.Film.2020.VFQ.1080p.mkv",
                untagged_audio(probe("bull_s05e01")),
            ),
            NOW,
        )
        .await
        .unwrap();
    let tracks = tracks_of(&metadata, quebec).await;
    assert_eq!(tracks[1].kind, StreamKind::Audio);
    assert_eq!(tracks[1].language.as_deref(), Some("fr-CA"));
    // The subtitle is not the audio, and keeps its own tag.
    assert_eq!(tracks[2].language.as_deref(), Some("fr"));

    let france = scan
        .add_item(
            FILMS,
            film_at(
                "D:\\Films\\Le.Film.2020.VFF.1080p.mkv",
                untagged_audio(probe("bull_s05e01")),
            ),
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(
        tracks_of(&metadata, france).await[1].language.as_deref(),
        Some("fr-FR")
    );

    // A tagged track keeps its tag, and two untagged ones say nothing of which is which.
    let tagged = scan
        .add_item(
            FILMS,
            film_at("D:\\Films\\Le.Film.2020.VFQ.mkv", probe("bull_s05e01")),
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(
        tracks_of(&metadata, tagged).await[1].language.as_deref(),
        Some("fr")
    );
    let two = scan
        .add_item(
            FILMS,
            film_at(
                "D:\\Films\\Le.Film.2020.MULTi.VFQ.mkv",
                untagged_audio(probe("soul")),
            ),
            NOW,
        )
        .await
        .unwrap();
    let tracks = tracks_of(&metadata, two).await;
    assert_eq!(
        (tracks[0].language.as_ref(), tracks[1].language.as_ref()),
        (None, None)
    );
}

#[tokio::test]
async fn known_parts_are_the_ones_under_the_prefix() {
    let (_, scan, _) = seeded().await;
    let film = scan.add_item(FILMS, soul(), NOW).await.unwrap();
    scan.add_item(SERIES, bull(1, None), NOW).await.unwrap();

    let known = scan.known_parts(FILMS, "C:\\Films\\").await.unwrap();
    assert_eq!(known.len(), 1);
    assert_eq!(known[0].item, film);
    assert_eq!(known[0].size, 5_825_269_622);
    assert_eq!(known[0].modified_at, 1_610_180_026);
    assert!(scan
        .known_parts(FILMS, "C:\\Films\\Soul (2019)")
        .await
        .unwrap()
        .is_empty());
    // A wildcard is a character like another.
    assert!(scan
        .known_parts(FILMS, "C:\\Films\\%")
        .await
        .unwrap()
        .is_empty());
    assert_eq!(scan.known_parts(FILMS, "").await.unwrap().len(), 1);
    assert_eq!(scan.known_parts(SERIES, "/srv/").await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_updated_part_keeps_its_ids_and_loses_its_old_tracks() {
    let (connection, scan, metadata) = seeded().await;
    let key = scan.add_item(FILMS, soul(), NOW).await.unwrap();
    let known = scan.known_parts(FILMS, "").await.unwrap();
    let part = known[0].part;
    let old_stream = tracks_of(&metadata, key).await[2].id.get();
    track_choice::Entity::insert(track_choice::ActiveModel {
        viewer: ActiveValue::Set(7),
        media_part_id: ActiveValue::Set(part.get()),
        audio_stream_id: ActiveValue::Set(None),
        subtitle_stream_id: ActiveValue::Set(Some(old_stream)),
        subtitle_off: ActiveValue::Set(false),
    })
    .exec(&connection)
    .await
    .unwrap();
    subtitle_offset::Entity::insert(subtitle_offset::ActiveModel {
        media_stream_id: ActiveValue::Set(old_stream),
        viewer: ActiveValue::Set(7),
        offset_ms: ActiveValue::Set(500),
    })
    .exec(&connection)
    .await
    .unwrap();

    let mut bull = probe("bull_s05e01");
    bull.file.size = 42;
    let replaced = NewPart {
        path: known[0].path.clone(),
        size: 42,
        modified_at: 1_700_000_000,
        analysis: Some(bull),
        sidecars: Vec::new(),
    };
    scan.update_part(part, replaced, NOW + 60).await.unwrap();

    let after = scan.known_parts(FILMS, "").await.unwrap();
    assert_eq!(after[0].part, part);
    assert_eq!(after[0].media, known[0].media);
    assert_eq!((after[0].size, after[0].modified_at), (42, 1_700_000_000));
    let tracks = tracks_of(&metadata, key).await;
    assert_eq!(tracks.len(), 3);
    assert!(tracks.iter().all(|track| track.id.get() > old_stream));
    let version = &metadata.versions_for(&[key]).await.unwrap()[0];
    assert_eq!(version.audio_codec.as_deref(), Some("aac"));
    assert_eq!(version.width, Some(1280));
    assert_eq!(version.duration_ms, Some(2_467_160));
    let item = metadata.item(key).await.unwrap();
    assert_eq!(item.duration_ms, Some(2_467_160));
    assert_eq!(item.updated_at, NOW + 60);
    assert_eq!(
        track_choice::Entity::find()
            .count(&connection)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        subtitle_offset::Entity::find()
            .count(&connection)
            .await
            .unwrap(),
        0
    );

    // A file that no longer reads as media keeps its rows and loses its description.
    let unreadable = NewPart {
        path: known[0].path.clone(),
        size: 7,
        modified_at: 1_700_000_100,
        analysis: None,
        sidecars: Vec::new(),
    };
    scan.update_part(part, unreadable, NOW + 120).await.unwrap();
    assert!(tracks_of(&metadata, key).await.is_empty());
    let version = &metadata.versions_for(&[key]).await.unwrap()[0];
    assert_eq!(version.container, None);
    assert_eq!(version.duration_ms, None);
}

#[tokio::test]
async fn removing_the_last_files_removes_items_seasons_and_shows() {
    let (connection, scan, metadata) = seeded().await;
    let film = scan.add_item(FILMS, soul(), NOW).await.unwrap();
    let first = scan
        .add_item(SERIES, bull(1, Some(probe("bull_s05e01"))), NOW)
        .await
        .unwrap();
    let second = scan.add_item(SERIES, bull(2, None), NOW).await.unwrap();
    item_marker::Entity::insert(item_marker::ActiveModel {
        item_id: ActiveValue::Set(film.get()),
        owner: ActiveValue::Set(None),
        kind: ActiveValue::Set("credits".to_owned()),
        start_ms: ActiveValue::Set(1),
        end_ms: ActiveValue::Set(Some(2)),
        is_final: ActiveValue::Set(true),
        version: ActiveValue::Set(None),
        title: ActiveValue::Set(None),
        color: ActiveValue::Set(None),
        ..Default::default()
    })
    .exec(&connection)
    .await
    .unwrap();

    let film_parts: Vec<PartId> = scan
        .known_parts(FILMS, "")
        .await
        .unwrap()
        .iter()
        .map(|known| known.part)
        .collect();
    scan.remove_parts(&film_parts).await.unwrap();
    assert!(metadata.item(film).await.is_err());
    assert_eq!(media::Entity::find().count(&connection).await.unwrap(), 2);
    assert_eq!(
        media_stream::Entity::find()
            .filter(media_stream::Column::Codec.eq("eac3"))
            .count(&connection)
            .await
            .unwrap(),
        0
    );
    // The cascade the migrations declare is enforced on the connection.
    assert_eq!(
        item_title_word::Entity::find()
            .filter(item_title_word::Column::ItemId.eq(film.get()))
            .count(&connection)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        item_marker::Entity::find()
            .count(&connection)
            .await
            .unwrap(),
        0
    );

    let parts = scan.known_parts(SERIES, "").await.unwrap();
    let of = |item: RatingKey| {
        parts
            .iter()
            .filter(|known| known.item == item)
            .map(|known| known.part)
            .collect::<Vec<_>>()
    };
    scan.remove_parts(&of(first)).await.unwrap();
    assert!(metadata.item(first).await.is_err());
    assert_eq!(
        metadata_item::Entity::find()
            .count(&connection)
            .await
            .unwrap(),
        3
    );
    scan.remove_parts(&of(second)).await.unwrap();
    assert_eq!(
        metadata_item::Entity::find()
            .count(&connection)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn finishing_records_the_scan_and_counts_a_change() {
    let (connection, scan, _) = seeded().await;
    scan.finish(FILMS, NOW, false).await.unwrap();
    let row = library_section::Entity::find_by_id(FILMS.get())
        .one(&connection)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((row.scanned_at, row.content_changed_at), (Some(NOW), 41));
    scan.finish(FILMS, NOW + 1, true).await.unwrap();
    let row = library_section::Entity::find_by_id(FILMS.get())
        .one(&connection)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.scanned_at, row.content_changed_at),
        (Some(NOW + 1), 42)
    );
    assert!(scan.finish(SectionId::new(9), NOW, true).await.is_err());
}

#[tokio::test]
async fn new_items_are_numbered_past_the_highest() {
    let (_, scan, _) = seeded().await;
    let first = scan.add_item(FILMS, soul(), NOW).await.unwrap();
    let second = scan
        .add_item(
            FILMS,
            NewItem::Movie {
                title: "Luca".to_owned(),
                year: None,
                versions: Vec::new(),
            },
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(second.get(), first.get() + 1);
    assert!(scan.add_item(SectionId::new(9), soul(), NOW).await.is_err());
}

#[tokio::test]
async fn a_version_names_its_widest_audio_and_keeps_no_cover_as_a_track() {
    use open_plex_scanner::report::TrackKind;

    let (_, scan, metadata) = seeded().await;
    let mut analysis = probe("soul");
    // The default track in stereo, the other in 5.1: the reference names the 5.1 (Fringe).
    for track in &mut analysis.tracks {
        if let (TrackKind::Audio, Some(audio)) = (track.kind, track.audio.as_mut()) {
            audio.channels = Some(if track.disposition.default { 2 } else { 6 });
        }
    }
    // A cover stored as a video track (Kaleidoscope's MJPEG).
    let mut cover = analysis
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .unwrap()
        .clone();
    cover.index = 6;
    cover.codec = Some("mjpeg".to_owned());
    cover.disposition.attached_picture = true;
    analysis.tracks.push(cover);

    let key = scan
        .add_item(FILMS, soul_with(analysis), NOW)
        .await
        .unwrap();
    let versions = metadata.versions_for(&[key]).await.unwrap();
    assert_eq!(versions[0].audio_channels, Some(6));
    let tracks = tracks_of(&metadata, key).await;
    assert!(tracks.iter().all(|track| track.position != 6), "{tracks:?}");
    let files = metadata.files_for(&[versions[0].id]).await.unwrap();
    assert!(files[0].layout.has_thumbnail);
}
