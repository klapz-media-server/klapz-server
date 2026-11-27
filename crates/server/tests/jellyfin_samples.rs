//! The whole chain behind a Jellyfin lookup, against what the reference Jellyfin published, on the
//! 35 sample files it holds.
//!
//! Each sample under `crates/scanner/tests/fixtures/samples` keeps ffmpeg-plex's ffprobe output
//! for the file (`ffprobe.json`, `tools/capture_scan_samples.py`) and the reference's answer
//! about it (`jellyfin.json`, `tools/capture_jellyfin_samples.py`). The analysis read from the
//! former goes through the scan's storage, the official API and the Jellyfin binding, as a
//! scanned file does; the item's file-derived fields and its
//! media source, every track field included, must then be the reference's. What may differ is
//! listed below, each with its reason (`docs/jellyfin/spec-gaps.md`); anything else fails.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use http_body_util::BodyExt;
use klapz_api::{build_schema, ApiContext};
use klapz_core::access::{AccessRepository, NewUser, ServerRole};
use klapz_core::scan::{NewItem, NewMedia, NewPart, ScanRepository};
use klapz_core::testing::read_queued;
use klapz_core::{SectionKind, ServerCapabilities, ServerIdentity};
use klapz_db::repository::{create_library, NewLibrary};
use klapz_db::{SqlLibraryRepository, SqlMetadataRepository, SqlScanRepository};
use klapz_scanner::analysis_from_ffprobe;
use klapz_scanner::report::{Analysis, FileIdentity};
use serde_json::Value;
use tower::ServiceExt;

/// Fields the reference reads in ways this server does not, on every file.
const EVERYWHERE: &[(&str, &str)] = &[
    // The reference's prober states one reference frame for every video; ours, H.264's
    // sequence header.
    ("RefFrames", "ref-frames"),
    // Attachments are not delivered here.
    ("DeliveryUrl", "delivery"),
];

/// Fields the reference's prober read by decoding the start of the file, and ffprobe-plex did
/// not, by sample; and the one file whose chapter images the reference drew.
const DECODED: &[(&str, &str)] = &[
    (
        "peur-primale-1996-multi-vff-1080i-hdtv-full-avc-he-aac-2-0",
        "BitRate",
    ),
    ("s01e41-la-cueillette", "BitRate"),
    ("clone-wars-s01e06", "Width"),
    ("clone-wars-s01e06", "Height"),
    ("daria-s00e01-multi-dvdrip-x264-linkjul", "Width"),
    ("daria-s00e01-multi-dvdrip-x264-linkjul", "Height"),
    ("s1957e06-bedevilled-rabbit", "BitDepth"),
    // The reference drew this film's chapter images, and keeps when; none is drawn here.
    (
        "jurassic-world-fallen-kingdom-2018-multi-truefrench-1080p",
        "ImageDateModified",
    ),
    (
        "star-trek-discovery-s03e10-multi-hdr-dv-1080p-web-h265-fw",
        "Hdr10PlusPresentFlag",
    ),
    (
        "star-trek-discovery-s03e10-multi-hdr-dv-1080p-web-h265-fw",
        "VideoRangeType",
    ),
];

fn samples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../scanner/tests/fixtures/samples")
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The sample as a scan finds it, and the headers' analysis, which the analysis reads first.
fn new_item(sample: &Path) -> (NewItem, Analysis) {
    let identity: FileIdentity =
        serde_json::from_value(json(&sample.join("file.json"))["file"].clone()).unwrap();
    let ffprobe = std::fs::read(sample.join("ffprobe.json")).unwrap();
    let analysis = analysis_from_ffprobe(&ffprobe, identity).unwrap();
    let name = json(&sample.join("ffprobe.json"))["format"]["filename"]
        .as_str()
        .unwrap()
        .trim_start_matches("file:")
        .rsplit(['/', '\\'])
        .next()
        .unwrap()
        .to_owned();
    let item = NewItem::Movie {
        title: name.clone(),
        year: None,
        versions: vec![NewMedia {
            parts: vec![NewPart {
                path: format!("/media/{name}"),
                size: analysis.file.size,
                modified_at: analysis.file.modified_at,
                sidecars: Vec::new(),
            }],
        }],
    };
    (item, analysis)
}

/// Every scalar of an answer by its path, tracks by their index in the file.
fn leaves(value: &Value, at: &str, found: &mut BTreeMap<String, Value>) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                if key == "MediaStreams" {
                    for track in child.as_array().unwrap() {
                        leaves(track, &format!("{at}{key}[{}].", track["Index"]), found);
                    }
                } else {
                    leaves(child, &format!("{at}{key}."), found);
                }
            }
        }
        Value::Array(items) if items.is_empty() => {
            found.insert(
                at.trim_end_matches('.').to_owned(),
                Value::Array(Vec::new()),
            );
        }
        Value::Array(items) => {
            for (position, item) in items.iter().enumerate() {
                leaves(item, &format!("{at}[{position}]."), found);
            }
        }
        scalar => {
            found.insert(at.trim_end_matches('.').to_owned(), scalar.clone());
        }
    }
}

fn same(ours: Option<&Value>, theirs: Option<&Value>) -> bool {
    match (ours, theirs) {
        (Some(Value::Number(a)), Some(Value::Number(b))) => a.as_f64() == b.as_f64(),
        (ours, theirs) => ours == theirs,
    }
}

/// Lets every request through as the server's owner.
async fn as_owner(
    axum::extract::State(principal): axum::extract::State<klapz_core::Principal>,
    mut request: Request,
    next: Next,
) -> Response {
    request.extensions_mut().insert(principal);
    next.run(request).await
}

/// The Jellyfin surface over the official API over storage, every request the owner's.
async fn jellyfin_app(connection: &klapz_db::DatabaseConnection) -> axum::Router {
    let access = Arc::new(klapz_core::testing::InMemoryAccess::default());
    let owner = access
        .create_user(
            NewUser {
                username: "owner",
                email: None,
                role: ServerRole::Owner,
                active: true,
                password_hash: None,
            },
            0,
        )
        .await
        .unwrap();
    let principal = klapz_core::access::user_principal(owner, &[]);
    let identity = ServerIdentity::new("0000000000000000000000000000000000000000", "0.1.0");
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity.clone(), 0),
        library: Arc::new(SqlLibraryRepository::new(connection.clone())),
        metadata: Arc::new(SqlMetadataRepository::new(connection.clone())),
        playback: Arc::new(klapz_core::testing::InMemoryPlayback::default()),
        play_queues: Arc::new(klapz_core::testing::InMemoryPlayQueues::default()),
        playlists: Arc::new(klapz_core::testing::InMemoryPlaylists::default()),
        collections: Arc::new(klapz_core::testing::InMemoryCollections::default()),
        access: Arc::clone(&access) as Arc<dyn AccessRepository>,
        directory: Arc::new(klapz_core::testing::StubDirectory::default()),
        watch: Arc::new(klapz_core::testing::InMemoryWatch::default()),
        streams: Arc::new(klapz_core::testing::StubStreams::default()),
        session_locks: Arc::default(),
        settings: std::sync::Arc::new(klapz_core::testing::InMemorySettings::default()),
        scans: Arc::new(klapz_core::testing::RecordedScans::default()),
        matching: None,
    });
    let sessions = Arc::new(klapz_api::Sessions::new(
        Arc::new(klapz_core::account::SessionKey::generate()),
        true,
    ));
    klapz_api_jellyfin::router(klapz_api_jellyfin::JellyfinState::new(
        schema, sessions, &identity,
    ))
    .layer(axum::middleware::from_fn_with_state(principal, as_owner))
}

/// What differs between our lookup of a sample and the reference's answer, past the reasons above.
fn differences_of(name: &str, answer: &Value, reference: &Value) -> Vec<String> {
    let mut differences = Vec::new();
    let mut ours = BTreeMap::new();
    let mut theirs = BTreeMap::new();
    let mut source = answer["MediaSources"][0].clone();
    for key in ["Id", "Path", "ETag"] {
        source.as_object_mut().unwrap().remove(key);
    }
    leaves(&source, "MediaSource.", &mut ours);
    leaves(&reference["MediaSource"], "MediaSource.", &mut theirs);
    for (key, value) in reference["Item"].as_object().unwrap() {
        leaves(value, &format!("Item.{key}."), &mut theirs);
        if let Some(mine) = answer.get(key) {
            leaves(mine, &format!("Item.{key}."), &mut ours);
        }
    }
    let keys: std::collections::BTreeSet<&String> = ours.keys().chain(theirs.keys()).collect();
    for key in keys {
        let field = key.rsplit('.').next().unwrap_or(key);
        let explained = EVERYWHERE.iter().any(|(known, _)| *known == field)
            || DECODED
                .iter()
                .any(|(sample, known)| *sample == name && *known == field);
        if explained || same(ours.get(key), theirs.get(key)) {
            continue;
        }
        differences.push(format!(
            "{name}: {key}: ours {:?}, reference {:?}",
            ours.get(key),
            theirs.get(key)
        ));
    }
    differences
}

#[tokio::test]
async fn every_sample_is_published_as_the_reference_publishes_it() {
    let connection = klapz_db::connect("sqlite::memory:").await.unwrap();
    let mut sections = BTreeMap::new();
    // The reference's films are in a French library, its series in one of no language, which
    // its server reads as English: unnamed chapters are named in it.
    for (library, language) in [("movies", "fr-FR"), ("tvshows", "en-US")] {
        let section = create_library(
            &connection,
            NewLibrary {
                kind: SectionKind::Movie,
                title: library.to_owned(),
                language: language.to_owned(),
                roots: vec![format!("/media/{library}")],
            },
        )
        .await
        .unwrap();
        sections.insert(library, section);
    }
    let scan = SqlScanRepository::new(connection.clone());
    let mut items = Vec::new();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(samples())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|sample| sample.join("jellyfin.json").is_file())
        .collect();
    paths.sort();
    assert_eq!(paths.len(), 35, "every file the reference holds");
    for sample in &paths {
        let reference = json(&sample.join("jellyfin.json"));
        let section = sections[reference["Library"].as_str().unwrap()];
        let (item, analysis) = new_item(sample);
        let item = scan.add_item(section, item, 1_763_400_000).await.unwrap();
        read_queued(&scan, section, 1_763_400_000, |_| Some(analysis.clone()))
            .await
            .unwrap();
        items.push((sample, item, reference));
    }

    let app = jellyfin_app(&connection).await;

    let mut differences = Vec::new();
    for (sample, item, reference) in items {
        let name = sample.file_name().unwrap().to_str().unwrap();
        let id = klapz_api_jellyfin::id::of("Item", item.get()).unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/jellyfin/Items/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let answer: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        differences.extend(differences_of(name, &answer, &reference));
    }
    assert!(differences.is_empty(), "\n{}", differences.join("\n"));
}

#[test]
fn every_decoded_difference_names_a_sample() {
    for (sample, field) in DECODED {
        assert!(
            samples().join(sample).join("jellyfin.json").is_file(),
            "{sample} ({field})"
        );
    }
}
