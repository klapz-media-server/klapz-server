//! A track's bitrate as its file states it, or as estimated when it states none.
//!
//! `MediaStream.bitrate` is the scan's measurement over every packet. Players that budget
//! bandwidth before reading anything use a figure they can have at once instead: what the
//! track's header declares, else what its muxer measured and tagged, else a typical rate for
//! its codec — and a file's one video takes what its container's declared rate leaves once
//! the others are counted. This is the reference Jellyfin's rule, reproduced on every track of
//! the 35 sample files its prober read alike.

use crate::metadata::{MediaStream, StreamKind};

/// What a track states of its rate, as `estimate` reads it: its header's, else its tags'
/// for sound and picture. A subtitle's tags are not read: Jellyfin publishes none.
fn stated(track: &MediaStream) -> Option<i64> {
    let header = track.stated.bitrate.filter(|rate| *rate > 0);
    match track.kind {
        StreamKind::Subtitle => header,
        _ => header.or(track.stated.tagged_bitrate.filter(|rate| *rate > 0)),
    }
}

/// A rate typical of an audio codec and its channels, for audio that states none.
fn typical_audio(track: &MediaStream) -> Option<i64> {
    let channels = i64::from(track.channels.filter(|channels| *channels >= 1)?);
    let surround = channels > 2;
    let pick = |stereo: i64, multichannel: i64| if surround { multichannel } else { stereo };
    Some(match track.codec.as_deref()? {
        "aac" | "mp3" | "mp2" => pick(192_000, 320_000),
        "ac3" | "eac3" => pick(192_000, 640_000),
        // DTS-HD Master Audio is lossless, whose rate follows the channels.
        "dca" | "dts"
            if track
                .profile
                .as_deref()
                .is_some_and(|profile| profile.starts_with("ma")) =>
        {
            channels * 700_000
        }
        "dca" | "dts" => pick(768_000, 1_509_000),
        "opus" => pick(128_000, 256_000),
        "vorbis" => pick(160_000, 320_000),
        "wmav1" | "wmav2" | "wmapro" => pick(192_000, 384_000),
        "flac" | "alac" => channels * 480_000,
        "truehd" | "mlp" => channels * 700_000,
        _ => return None,
    })
}

/// The rate of each of a file's tracks, in the order given: stated, else estimated. `file_rate`
/// is the rate the file's container declares.
pub fn estimate(tracks: &[MediaStream], file_rate: Option<i64>) -> Vec<Option<i64>> {
    let mut rates: Vec<Option<i64>> = tracks
        .iter()
        .map(|track| {
            stated(track).or_else(|| match track.kind {
                StreamKind::Audio => typical_audio(track),
                _ => None,
            })
        })
        .collect();
    let videos: Vec<usize> = tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| track.kind == StreamKind::Video)
        .map(|(index, _)| index)
        .collect();
    let others = || {
        tracks
            .iter()
            .zip(&rates)
            .filter(|(track, _)| track.kind != StreamKind::Video && track.external_path.is_none())
    };
    if let ([video], Some(total)) = (videos.as_slice(), file_rate) {
        let audio_known = others()
            .filter(|(track, _)| track.kind == StreamKind::Audio)
            .all(|(_, rate)| rate.is_some());
        if rates[*video].is_none() && audio_known {
            let left = total - others().filter_map(|(_, rate)| *rate).sum::<i64>();
            rates[*video] = (left > 0).then_some(left);
        }
    }
    rates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{PartId, StreamId};
    use crate::metadata::{StatedTrack, TrackTraits};

    fn track(kind: StreamKind, codec: &str, channels: Option<i32>) -> MediaStream {
        MediaStream {
            id: StreamId::new(1),
            file_id: PartId::new(1),
            kind,
            position: 0,
            codec: Some(codec.to_owned()),
            language: None,
            channels,
            bitrate: Some(1),
            is_default: false,
            is_forced: false,
            external_path: None,
            title: None,
            stream_identifier: None,
            profile: None,
            bit_depth: None,
            traits: TrackTraits::default(),
            video: None,
            audio: None,
            subtitle: None,
            required_bandwidths: Vec::new(),
            stated: StatedTrack::default(),
        }
    }

    fn stating(mut track: MediaStream, header: Option<i64>, tagged: Option<i64>) -> MediaStream {
        track.stated.bitrate = header;
        track.stated.tagged_bitrate = tagged;
        track
    }

    #[test]
    fn the_header_comes_before_the_tags_and_the_tags_before_a_typical_rate() {
        // Ghost in the Shell's DTS: 1536000 declared, 1509000 tagged.
        let declared = stating(
            track(StreamKind::Audio, "dca", Some(6)),
            Some(1_536_000),
            Some(1_509_000),
        );
        let tagged = stating(
            track(StreamKind::Audio, "eac3", Some(6)),
            None,
            Some(640_000),
        );
        let silent = track(StreamKind::Audio, "opus", Some(2));
        assert_eq!(
            estimate(&[declared, tagged, silent], None),
            [Some(1_536_000), Some(640_000), Some(128_000)]
        );
    }

    #[test]
    fn a_subtitle_keeps_its_header_alone() {
        let subtitle = stating(track(StreamKind::Subtitle, "pgs", None), None, Some(14_166));
        assert_eq!(estimate(&[subtitle], None), [None]);
    }

    #[test]
    fn the_one_video_takes_what_the_container_leaves() {
        // doodlebug: 667480 for the file, an Opus track stating nothing.
        let video = track(StreamKind::Video, "h264", None);
        let audio = track(StreamKind::Audio, "opus", Some(2));
        assert_eq!(
            estimate(&[video, audio], Some(667_480)),
            [Some(539_480), Some(128_000)]
        );
    }

    #[test]
    fn a_video_stays_unknown_while_an_audio_rate_is() {
        let video = track(StreamKind::Video, "h264", None);
        let unknown = track(StreamKind::Audio, "cook", Some(2));
        assert_eq!(
            estimate(&[video.clone(), unknown], Some(1_000_000))[0],
            None
        );
        let second = track(StreamKind::Video, "h264", None);
        assert_eq!(estimate(&[video, second], Some(1_000_000)), [None, None]);
    }

    #[test]
    fn lossless_audio_counts_its_channels() {
        let mut master = track(StreamKind::Audio, "dca", Some(6));
        master.profile = Some("ma".to_owned());
        let truehd = track(StreamKind::Audio, "truehd", Some(8));
        let flac = track(StreamKind::Audio, "flac", Some(2));
        assert_eq!(
            estimate(&[master, truehd, flac], None),
            [Some(4_200_000), Some(5_600_000), Some(960_000)]
        );
    }
}
