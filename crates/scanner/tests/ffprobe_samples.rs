//! Reading ffprobe's output, on what it printed for 38 files of the reference's library
//! (`fixtures/samples`, from `tools/capture_scan_samples.py`).
//!
//! `crates/db/tests/scan_samples.rs` compares the whole chain with what the reference stored;
//! these pin down what the parser itself takes from ffprobe, where each sample is the one
//! that carries the case.

#![allow(clippy::unwrap_used, clippy::float_cmp)]

use std::path::{Path, PathBuf};

use open_plex_scanner::report::{Analysis, Cropping, FileIdentity, Track, TrackKind};
use open_plex_scanner::{analysis_from_ffprobe, sequence_header_from_trace};

fn sample(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/samples")
        .join(name)
}

fn identity(sample: &Path) -> FileIdentity {
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sample.join("file.json")).unwrap()).unwrap();
    serde_json::from_value(file["file"].clone()).unwrap()
}

fn analysis(name: &str) -> Analysis {
    let sample = sample(name);
    let output = std::fs::read(sample.join("ffprobe.json")).unwrap();
    analysis_from_ffprobe(&output, identity(&sample)).unwrap()
}

fn track(analysis: &Analysis, index: u32) -> &Track {
    analysis
        .tracks
        .iter()
        .find(|track| track.index == index)
        .unwrap()
}

#[test]
fn every_sample_parses_into_one_track_per_stream() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/samples");
    for entry in std::fs::read_dir(root).unwrap() {
        let sample = entry.unwrap().path();
        let output = std::fs::read(sample.join("ffprobe.json")).unwrap();
        let probed: serde_json::Value = serde_json::from_slice(&output).unwrap();
        let analysis = analysis_from_ffprobe(&output, identity(&sample)).unwrap();
        assert_eq!(
            analysis.tracks.len(),
            probed["streams"].as_array().unwrap().len(),
            "{}",
            sample.display()
        );
        assert!(analysis.duration_ms.is_some(), "{}", sample.display());
    }
}

#[test]
fn reads_the_container_its_duration_and_rate() {
    let fleabag = analysis("fleabag-s02e01-repack-1080p-hdtv-h264-plutonium");
    assert_eq!(fleabag.container, "matroska,webm");
    assert_eq!(fleabag.duration_ms, Some(1_525_376));
    assert_eq!(fleabag.bitrate, Some(2_504_336));
}

#[test]
fn takes_the_depth_from_the_pixel_format_when_ffprobe_states_none() {
    // HEVC 10-bit, AV1 and MPEG-2: no `bits_per_raw_sample`, a pixel format.
    let groot = analysis("i-am-groot-s01e02-4k-multi-2160p-hdr-web-h265-extreme");
    assert_eq!(track(&groot, 3).video.as_ref().unwrap().bit_depth, Some(10));
    let av1 = analysis("le-mal-aime-2025-french-webdl");
    assert_eq!(track(&av1, 0).video.as_ref().unwrap().bit_depth, Some(8));
    let mpeg2 = analysis("kaamelott-s00e302-bonus-s2-bande-annonce-livre-iii");
    assert_eq!(track(&mpeg2, 0).video.as_ref().unwrap().bit_depth, Some(8));
}

#[test]
fn takes_the_average_frame_rate_where_it_drifts_from_the_nominal_one() {
    let bloques = analysis("bloques-s01e10-comment-tu-sais-que-tu-l-aimes");
    let rate = track(&bloques, 0)
        .video
        .as_ref()
        .unwrap()
        .frame_rate
        .unwrap();
    assert_eq!(format!("{rate:.3}"), "25.011");
}

#[test]
fn reads_a_matroska_crop_and_the_ratio_after_it() {
    let fleabag = analysis("fleabag-s02e01-repack-1080p-hdtv-h264-plutonium");
    let video = track(&fleabag, 0).video.as_ref().unwrap();
    assert_eq!(video.sample_aspect_ratio.as_deref(), Some("1:1"));
    assert_eq!(
        video.cropping,
        Some(Cropping {
            top: 140,
            bottom: 140,
            left: 0,
            right: 0
        })
    );
}

#[test]
fn reads_the_containers_number_for_a_track() {
    // MPEG-TS PIDs and MP4 track IDs, which ffprobe prints in hex; none in Matroska.
    let ts = analysis("peur-primale-1996-multi-vff-1080i-hdtv-full-avc-he-aac-2-0");
    assert_eq!(track(&ts, 1).identifier, Some(0x44));
    let mp4 = analysis("s00e02");
    assert_eq!(track(&mp4, 1).identifier, Some(2));
    let mkv = analysis("fleabag-s02e01-repack-1080p-hdtv-h264-plutonium");
    assert_eq!(track(&mkv, 0).identifier, None);
}

#[test]
fn keeps_tags_and_names_as_ffprobe_prints_them() {
    // Teletext's language per page, left to the storage to judge.
    let ts = analysis("peur-primale-1996-multi-vff-1080i-hdtv-full-avc-he-aac-2-0");
    assert_eq!(track(&ts, 0).language.as_deref(), Some("fra,fra"));
    // DTS:X under DTS-HD MA, and mkvmerge's statistics.
    let jurassic = analysis("jurassic-world-fallen-kingdom-2018-multi-truefrench-1080p");
    let dtsx = track(&jurassic, 2);
    assert_eq!(dtsx.profile.as_deref(), Some("DTS-HD MA + DTS:X"));
    assert_eq!(dtsx.bitrate, None);
    assert_eq!(dtsx.tagged_bitrate, Some(4_230_911));
    // A video's own statistics tag, beside a second video.
    let steven = analysis("suf-steven-universe-s01e13-so-many-birthdays-vf-vostfr-multi");
    assert_eq!(track(&steven, 1).tagged_bitrate, Some(1_841_709));
    // A track written `und` reports no language.
    let doodlebug = analysis("doodlebug-1997-vo-h264");
    assert_eq!(track(&doodlebug, 0).language, None);
}

#[test]
fn tells_covers_and_chapter_tracks_apart_from_media() {
    let cover = analysis("kaleidoscope-s00e01-noir");
    let picture = track(&cover, 6);
    assert_eq!(picture.kind, TrackKind::Video);
    assert!(picture.disposition.attached_picture);
    let soda = analysis("s03e13-titre-non-communique");
    let chapters = track(&soda, 2);
    assert_eq!(chapters.kind, TrackKind::Data);
    assert_eq!(chapters.codec_tag.as_deref(), Some("text"));
}

#[test]
fn reads_the_dolby_vision_configuration_record() {
    let ohio = analysis("devil-in-ohio-s01e07-par-le-sang-1080p-dv-hdr-x265-amen");
    let record = track(&ohio, 0)
        .video
        .as_ref()
        .unwrap()
        .dolby_vision
        .unwrap();
    assert_eq!(record.profile, 8);
    assert!(record.rpu_present && record.bl_present && !record.el_present);
}

#[test]
fn reads_an_h264_sequence_header_from_its_trace() {
    let trace = std::fs::read_to_string(
        sample("fleabag-s02e01-repack-1080p-hdtv-h264-plutonium").join("trace-0.txt"),
    )
    .unwrap();
    let header = sequence_header_from_trace(&trace).unwrap();
    assert_eq!((header.coded_width, header.coded_height), (1920, 1088));
    assert_eq!(header.ref_frames, 4);
    assert!(header.has_scaling_matrix);
}

#[test]
fn keeps_what_ffprobe_states_as_written() {
    let bloques = analysis("bloques-s01e10-comment-tu-sais-que-tu-l-aimes");
    assert_eq!(bloques.duration_us, Some(90_372_063));
    let video = track(&bloques, 0);
    assert_eq!(video.time_base.as_deref(), Some("1/90000"));
    assert_eq!(video.raw_codec_tag.as_deref(), Some("avc1"));
    assert_eq!(video.handler_name.as_deref(), Some("VideoHandler"));
    // An MP4 states `und`, which is a statement, unlike Matroska's silence.
    assert_eq!(video.declared_language.as_deref(), Some("und"));
    assert_eq!(video.language, None);
    let picture = video.video.as_ref().unwrap();
    assert_eq!(
        picture.average_frame_rate.as_deref(),
        Some("203220000/8125199")
    );
    assert_eq!(picture.real_frame_rate.as_deref(), Some("25/1"));
    assert_eq!(picture.is_avc, Some(true));
    assert_eq!(picture.nal_length_size, Some(4));
}

#[test]
fn tells_annex_b_framing_from_length_prefixes() {
    let peur = analysis("peur-primale-1996-multi-vff-1080i-hdtv-full-avc-he-aac-2-0");
    let picture = track(&peur, 1).video.as_ref().unwrap();
    assert_eq!(picture.is_avc, Some(false));
    assert_eq!(picture.nal_length_size, Some(0));
    // Teletext lists a language per page, kept as written.
    assert_eq!(
        track(&peur, 0).declared_language.as_deref(),
        Some("fra,fra")
    );
}

#[test]
fn reads_a_muxers_statistics_under_either_name() {
    let kaamelott = analysis("kaamelott-s00e602-bonus-s5-bande-annonce-livre-vi");
    let statistics = track(&kaamelott, 1).statistics.unwrap();
    assert_eq!(statistics.bitrate, Some(1_668_865));
    assert_eq!(statistics.bytes, Some(10_440_420));
    assert_eq!(statistics.duration_us, Some(50_048_000));
    // `BPS-eng` alone: the rate Plex's rule reads stays unset.
    assert_eq!(track(&kaamelott, 1).tagged_bitrate, None);
}

#[test]
fn a_picture_subtitle_has_a_canvas_and_a_text_one_none() {
    let fringe = analysis("fringe-s03e19-multi-1080p-bluray-h-265-dts-hdma-5-1-celestis");
    let canvas = track(&fringe, 5).subtitle.unwrap();
    assert_eq!((canvas.width, canvas.height), (1920, 1080));
    assert_eq!(track(&fringe, 4).subtitle, None);
}

#[test]
fn a_matroska_cover_keeps_its_name_and_type() {
    let devil = analysis("devil-in-ohio-s01e07-par-le-sang-1080p-dv-hdr-x265-amen");
    let cover = track(&devil, 6);
    assert!(cover.disposition.attached_picture);
    let attachment = cover.attachment.as_ref().unwrap();
    assert_eq!(attachment.filename.as_deref(), Some("cover.png"));
    assert_eq!(attachment.mime_type.as_deref(), Some("image/png"));
    // An MP4's cover is a track, which names nothing.
    let mp4 = analysis("90210-beverly-hills-ng-s01e21-vf-sd-tvhdrip-mp4-avc-aac");
    assert_eq!(track(&mp4, 2).attachment, None);
}
