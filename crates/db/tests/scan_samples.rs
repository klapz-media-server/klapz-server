//! The scan's analysis against what the reference stored, on 38 sample files.
//!
//! Each sample under `crates/scanner/tests/fixtures/samples` holds ffmpeg-plex's ffprobe output
//! for a file of the reference's library, the file's identity, and the rows the reference
//! stored for that same file (`tools/capture_scan_samples.py`). Every sample is replayed
//! through the analysis and the scan's storage, then read back and written in the reference's vocabulary — its
//! columns and `ma:` keys, and its chapters — to be compared value for value. What may differ is listed below,
//! each with its reason; anything else differing fails.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::float_cmp)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use klapz_core::ids::SectionId;
use klapz_core::scan::{NewItem, NewMedia, NewPart, ScanRepository};
use klapz_db::entity::{
    item_chapter, library_section, media, media_part, media_stream, metadata_item,
    stream_dolby_vision,
};
use klapz_db::SqlScanRepository;
use klapz_scanner::analysis_from_ffprobe;
use klapz_scanner::report::FileIdentity;
use sea_orm::{ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;

const FILMS: SectionId = SectionId::new(1);

/// Values the reference's older libav read otherwise, by sample and field.
const KNOWN: &[(&str, &str, &str)] = &[
    // AAC in MPEG-TS: libavformat estimates the rate from the frames it parsed, and the
    // reference's estimated more (80 kb/s where ffprobe 8.1 declares 73). The video's rate is
    // what the container's leaves once the audio's is taken out, so it moves with them.
    (
        "peur-primale-1996-multi-vff-1080i-hdtv-full-avc-he-aac-2-0",
        "stream 1 bitrate",
        "TS",
    ),
    (
        "peur-primale-1996-multi-vff-1080i-hdtv-full-avc-he-aac-2-0",
        "stream 2 bitrate",
        "TS",
    ),
    (
        "peur-primale-1996-multi-vff-1080i-hdtv-full-avc-he-aac-2-0",
        "stream 3 bitrate",
        "TS",
    ),
    (
        "peur-primale-1996-multi-vff-1080i-hdtv-full-avc-he-aac-2-0",
        "stream 4 bitrate",
        "TS",
    ),
    (
        "peur-primale-1996-multi-vff-1080i-hdtv-full-avc-he-aac-2-0",
        "stream 5 bitrate",
        "TS",
    ),
    ("s01e41-la-cueillette", "stream 0 bitrate", "TS"),
    ("s01e41-la-cueillette", "stream 1 bitrate", "TS"),
    // An E-AC-3 Atmos track the reference stored at 320 kb/s, where both its header and its
    // tag say 768: what its libav read of the independent substream alone, presumably.
    (
        "the-bikeriders-2023-hybrid-multi-vff-2160p-10bit-4klight-dv",
        "stream 0 bitrate",
        "E-AC-3",
    ),
    (
        "the-bikeriders-2023-hybrid-multi-vff-2160p-10bit-4klight-dv",
        "stream 1 bitrate",
        "E-AC-3",
    ),
];

/// Fields whose stored value comes from a deep analysis, which a sample does not replay.
const MEASURED: &[&str] = &["media bitrate", "media duration", "part duration"];

fn samples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../scanner/tests/fixtures/samples")
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The sample as a scan would store it: ffprobe's output through the analysis.
fn new_item(sample: &Path) -> NewItem {
    let file = json(&sample.join("file.json"));
    let identity: FileIdentity = serde_json::from_value(file["file"].clone()).unwrap();
    let ffprobe = std::fs::read(sample.join("ffprobe.json")).unwrap();
    let name = json(&sample.join("ffprobe.json"))["format"]["filename"]
        .as_str()
        .unwrap()
        .to_owned();
    let analysis = analysis_from_ffprobe(&ffprobe, identity.clone()).unwrap();
    NewItem::Movie {
        title: name.clone(),
        year: None,
        versions: vec![NewMedia {
            parts: vec![NewPart {
                path: format!("D:\\Samples\\{name}"),
                size: identity.size,
                modified_at: identity.modified_at,
                analysis: Some(analysis),
                sidecars: Vec::new(),
            }],
        }],
    }
}

/// What was stored, in the reference's vocabulary: `media width`, `part ma:hasThumbnail`,
/// `stream 1 ma:profile`.
async fn stored(connection: &DatabaseConnection, item: i64) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    let mut put = |key: String, value: Option<String>| {
        if let Some(value) = value {
            values.insert(key, value);
        }
    };
    let version = media::Entity::find()
        .filter(media::Column::MetadataItemId.eq(item))
        .one(connection)
        .await
        .unwrap()
        .unwrap();
    put("media width".into(), text(version.width));
    put("media height".into(), text(version.height));
    put(
        "media display_aspect_ratio".into(),
        text(version.aspect_ratio),
    );
    put("media bitrate".into(), text(version.bitrate));
    put("media container".into(), version.container);
    put("media video_codec".into(), version.video_codec);
    put("media audio_codec".into(), version.audio_codec);
    put("media audio_channels".into(), text(version.audio_channels));
    put("media frames_per_second".into(), text(version.frame_rate));
    put("media duration".into(), text(version.duration_ms));

    let part = media_part::Entity::find()
        .filter(media_part::Column::MediaId.eq(version.id))
        .one(connection)
        .await
        .unwrap()
        .unwrap();
    put("part size".into(), text(part.size_bytes));
    put("part duration".into(), text(part.duration_ms));
    put(
        "part ma:optimizedForStreaming".into(),
        stated(part.optimized_for_streaming),
    );
    put(
        "part ma:has64bitOffsets".into(),
        stated(part.has_64bit_offsets),
    );
    put("part ma:hasThumbnail".into(), flag(part.has_thumbnail));
    put("part ma:packetLength".into(), text(part.packet_length));
    put(
        "part ma:hasChapterTextStream".into(),
        flag(part.has_chapter_text_stream),
    );

    let row = metadata_item::Entity::find_by_id(item)
        .one(connection)
        .await
        .unwrap()
        .unwrap();
    put("item chapterSource".into(), row.chapter_source);
    let chapters = item_chapter::Entity::find()
        .filter(item_chapter::Column::ItemId.eq(item))
        .all(connection)
        .await
        .unwrap();
    for chapter in chapters {
        let index = chapter.position;
        put(
            format!("chapter {index} start"),
            text(Some(chapter.start_ms)),
        );
        put(format!("chapter {index} end"), text(Some(chapter.end_ms)));
        put(format!("chapter {index} title"), chapter.title);
    }

    let tracks = media_stream::Entity::find()
        .filter(media_stream::Column::MediaPartId.eq(part.id))
        .all(connection)
        .await
        .unwrap();
    for track in tracks {
        let record = stream_dolby_vision::Entity::find_by_id(track.id)
            .one(connection)
            .await
            .unwrap();
        for (key, value) in track_values(&track, record) {
            put(format!("stream {} {key}", track.position), value);
        }
    }
    values
}

/// One track's columns and `ma:` keys, as the reference writes them.
fn track_values(
    track: &media_stream::Model,
    record: Option<stream_dolby_vision::Model>,
) -> Vec<(&'static str, Option<String>)> {
    let kind = match track.kind.as_str() {
        "video" => "1",
        "audio" => "2",
        _ => "3",
    };
    let mut values = vec![
        ("stream_type_id", Some(kind.to_owned())),
        ("codec", track.codec.clone()),
        ("language", track.language.clone()),
        ("channels", text(track.channels)),
        ("bitrate", text(track.bitrate)),
        ("default", Some(u8::from(track.is_default).to_string())),
        ("forced", Some(u8::from(track.is_forced).to_string())),
        ("ma:title", track.title.clone()),
        ("ma:streamIdentifier", text(track.stream_identifier)),
        ("ma:profile", track.profile.clone()),
        ("ma:bitDepth", text(track.bit_depth)),
        ("ma:width", text(track.width)),
        ("ma:height", text(track.height)),
        ("ma:codedWidth", text(track.coded_width)),
        ("ma:codedHeight", text(track.coded_height)),
        (
            "ma:frameRate",
            track.frame_rate.map(|rate| format!("{rate:.3}")),
        ),
        ("ma:level", text(track.level)),
        ("ma:refFrames", text(track.ref_frames)),
        ("ma:scanType", track.scan_type.clone()),
        ("ma:chromaSubsampling", track.chroma_subsampling.clone()),
        ("ma:chromaLocation", track.chroma_location.clone()),
        ("ma:colorRange", track.color_range.clone()),
        ("ma:colorSpace", track.color_space.clone()),
        ("ma:colorPrimaries", track.color_primaries.clone()),
        ("ma:colorTrc", track.color_transfer.clone()),
        ("ma:hasScalingMatrix", stated(track.has_scaling_matrix)),
        ("ma:anamorphic", stated(track.is_anamorphic)),
        ("ma:pixelAspectRatio", track.pixel_aspect_ratio.clone()),
        ("ma:codecID", track.fourcc.clone()),
        ("ma:closedCaptions", flag(track.has_closed_captions)),
        ("ma:separateFields", flag(track.has_separate_fields)),
        ("ma:HDR10PlusPresent", flag(track.has_hdr10_plus)),
        ("ma:samplingRate", text(track.sampling_rate)),
        ("ma:audioChannelLayout", track.channel_layout.clone()),
        ("ma:format", track.subtitle_format.clone()),
        ("ma:embeddedInVideo", flag(track.is_embedded_in_video)),
        ("ma:original", flag(track.is_original)),
        ("ma:dub", flag(track.is_dub)),
        ("ma:comment", flag(track.is_commentary)),
        ("ma:hearingImpaired", flag(track.is_hearing_impaired)),
        ("ma:visualImpaired", flag(track.is_visual_impaired)),
        ("ma:headerCompression", flag(track.has_header_compression)),
    ];
    if let Some(record) = record {
        values.extend([
            ("ma:DOVIPresent", flag(true)),
            ("ma:DOVIVersion", Some(record.version)),
            ("ma:DOVIProfile", text(Some(record.profile))),
            ("ma:DOVILevel", text(Some(record.level))),
            ("ma:DOVIBLCompatID", text(record.base_layer_compatibility)),
            ("ma:DOVIBLPresent", flag(record.has_base_layer)),
            ("ma:DOVIELPresent", flag(record.has_enhancement_layer)),
            ("ma:DOVIRPUPresent", flag(record.has_rpu)),
        ]);
    }
    values
}

fn text<T: ToString>(value: Option<T>) -> Option<String> {
    value.map(|value| value.to_string())
}

/// A flag the reference writes only when set.
fn flag(value: bool) -> Option<String> {
    value.then(|| "1".to_owned())
}

/// A flag the reference writes either way once it is known.
fn stated(value: Option<bool>) -> Option<String> {
    value.map(|value| u8::from(value).to_string())
}

/// The reference's rows, in the same keys.
fn reference(sample: &Path) -> (BTreeMap<String, String>, bool) {
    let stored = json(&sample.join("reference.json"));
    let mut values = BTreeMap::new();
    let scalar = |value: &Value| match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    for (key, value) in stored["media"].as_object().unwrap() {
        values.insert(format!("media {key}"), scalar(value));
    }
    for key in ["size", "duration"] {
        values.insert(format!("part {key}"), scalar(&stored["part"][key]));
    }
    if let Some(source) = stored["chapterSource"].as_str() {
        values.insert("item chapterSource".into(), source.to_owned());
    }
    for chapter in stored["chapters"].as_array().unwrap() {
        let index = &chapter["index"];
        values.insert(format!("chapter {index} start"), scalar(&chapter["start"]));
        values.insert(format!("chapter {index} end"), scalar(&chapter["end"]));
        if let Some(title) = chapter["title"].as_str().filter(|title| !title.is_empty()) {
            values.insert(format!("chapter {index} title"), title.to_owned());
        }
    }
    for (key, value) in stored["part"]["analysis"].as_object().unwrap() {
        values.insert(format!("part {key}"), scalar(value));
    }
    for stream in stored["streams"].as_array().unwrap() {
        let index = &stream["index"];
        for (key, value) in stream.as_object().unwrap() {
            if key == "analysis" {
                for (key, value) in value.as_object().unwrap() {
                    values.insert(format!("stream {index} {key}"), scalar(value));
                }
            } else if key != "index" {
                values.insert(format!("stream {index} {key}"), scalar(value));
            }
        }
    }
    (values, stored["rates"] == "measured")
}

/// Whether two values say the same. A float the deep analysis rewrote is kept to 15 digits
/// (`1.77777779102325` for the 32-bit `1.7777777910232544` a normal one stores), and a flag's
/// absence is written `0` in places.
fn same(key: &str, ours: Option<&String>, theirs: Option<&String>) -> bool {
    match (ours, theirs) {
        (Some(ours), Some(theirs)) if ours == theirs => true,
        (Some(ours), Some(theirs)) => matches!(
            (ours.parse::<f64>(), theirs.parse::<f64>()),
            (Ok(ours), Ok(theirs)) if (ours - theirs).abs() <= theirs.abs() * 1e-13
        ),
        (None, Some(theirs)) => theirs == "0" && !key.ends_with("bitrate"),
        (None, None) => true,
        (Some(_), None) => false,
    }
}

#[tokio::test]
async fn a_scanned_sample_stores_what_the_reference_stored() {
    let connection = films_library().await;
    let scan = SqlScanRepository::new(connection.clone());

    let mut samples: Vec<PathBuf> = std::fs::read_dir(samples())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    samples.sort();
    assert_eq!(samples.len(), 38, "every sample captured");

    let mut differences = Vec::new();
    for sample in &samples {
        let name = sample.file_name().unwrap().to_str().unwrap();
        let item = scan
            .add_item(FILMS, new_item(sample), 1_763_400_000)
            .await
            .unwrap();
        let ours = stored(&connection, item.get()).await;
        let (theirs, measured) = reference(sample);
        let keys: std::collections::BTreeSet<&String> = ours.keys().chain(theirs.keys()).collect();
        for key in keys {
            let deep = measured && (MEASURED.contains(&key.as_str()) || key.ends_with(" bitrate"));
            let known = KNOWN
                .iter()
                .any(|(sample, field, _)| *sample == name && *field == key);
            if deep || known || same(key, ours.get(key), theirs.get(key)) {
                continue;
            }
            differences.push(format!(
                "{name}: {key}: ours {:?}, reference {:?}",
                ours.get(key),
                theirs.get(key)
            ));
        }
    }
    assert!(differences.is_empty(), "\n{}", differences.join("\n"));
}

async fn films_library() -> DatabaseConnection {
    let connection = klapz_db::connect("sqlite::memory:").await.unwrap();
    library_section::Entity::insert(library_section::ActiveModel {
        id: ActiveValue::Set(FILMS.get()),
        kind: ActiveValue::Set("movie".to_owned()),
        title: ActiveValue::Set("Samples".to_owned()),
        title_sort: ActiveValue::Set(None),
        language: ActiveValue::Set("fr-FR".to_owned()),
        uuid: ActiveValue::Set("uuid-samples".to_owned()),
        created_at: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(0),
        scanned_at: ActiveValue::Set(None),
        content_changed_at: ActiveValue::Set(0),
        hidden: ActiveValue::Set(false),
    })
    .exec(&connection)
    .await
    .unwrap();
    connection
}

#[tokio::test]
async fn a_scan_keeps_what_ffprobe_states_and_what_does_not_play() {
    use klapz_core::metadata::{MetadataRepository, SideStreamKind, StreamKind};
    let connection = films_library().await;
    let scan = SqlScanRepository::new(connection.clone());
    let metadata = klapz_db::SqlMetadataRepository::new(connection.clone());
    let sample = samples().join("90210-beverly-hills-ng-s01e21-vf-sd-tvhdrip-mp4-avc-aac");
    let item = scan
        .add_item(FILMS, new_item(&sample), 1_763_400_000)
        .await
        .unwrap();
    let versions = metadata.versions_for(&[item]).await.unwrap();
    let files = metadata.files_for(&[versions[0].id]).await.unwrap();
    let file = &files[0];
    assert_eq!(
        file.stated.format_name.as_deref(),
        Some("mov,mp4,m4a,3gp,3g2,mj2")
    );
    assert_eq!(file.stated.duration_us, Some(287_754_134));
    assert_eq!(file.stated.bitrate, Some(1_986_929));

    let tracks = metadata.streams_for(&[file.id]).await.unwrap();
    assert_eq!(tracks.len(), 2, "the cover is no track");
    let video = tracks
        .iter()
        .find(|track| track.kind == StreamKind::Video)
        .unwrap();
    assert_eq!(video.stated.time_base.as_deref(), Some("1/30000"));
    assert_eq!(video.stated.codec_tag.as_deref(), Some("avc1"));
    assert_eq!(video.stated.language.as_deref(), Some("und"));
    assert_eq!(video.stated.bitrate, Some(1_510_916));
    let analysis = video.video.as_ref().unwrap();
    assert_eq!(analysis.average_frame_rate.as_deref(), Some("30000/1001"));
    assert_eq!(analysis.nal_length_size, Some(4));
    assert_eq!(analysis.is_avc, Some(true));
    assert_eq!(analysis.pixel_format.as_deref(), Some("yuv420p"));

    let side = metadata.side_streams_for(&[file.id]).await.unwrap();
    assert_eq!(side.len(), 1);
    assert_eq!(side[0].kind, SideStreamKind::Cover);
    assert_eq!(side[0].position, 2);
    assert_eq!(side[0].codec.as_deref(), Some("bmp"));
    assert_eq!(side[0].codec_tag.as_deref(), Some("[0][0][0][0]"));
    let picture = side[0].picture.as_ref().unwrap();
    assert_eq!((picture.width, picture.height), (Some(640), Some(360)));
    assert_eq!(picture.real_frame_rate.as_deref(), Some("90000/1"));
}

#[test]
fn every_known_difference_names_a_sample() {
    for (sample, field, _) in KNOWN {
        assert!(samples().join(sample).is_dir(), "{sample} ({field})");
    }
}
