//! Which audio and subtitle track a viewer plays a file with.
//!
//! A viewer may choose either, per file, and what it did not choose is chosen for it. The
//! automatic rule is the reference's as far as measured: the file's default audio track, else
//! its first; and the forced subtitle in that audio's language, which the reference selected
//! with no choice made — French forced subtitles under French audio.

use crate::ids::StreamId;
use crate::metadata::{MediaStream, StreamKind};

/// What a viewer asked for on one file. `None` is "choose for me".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrackChoice {
    pub audio: Option<StreamId>,
    pub subtitle: Option<SubtitleChoice>,
}

impl TrackChoice {
    /// Nothing asked for: the row can go.
    pub const fn is_empty(&self) -> bool {
        self.audio.is_none() && self.subtitle.is_none()
    }
}

/// A subtitle choice, which unlike audio can be "none".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleChoice {
    Off,
    Track(StreamId),
}

/// The tracks a file plays with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectedTracks {
    pub audio: Option<StreamId>,
    pub subtitle: Option<StreamId>,
}

/// Applies a choice to a file's tracks. A chosen track that is not one of the file's, or not
/// of the kind, is ignored rather than trusted: the file may have been rescanned since.
pub fn selected_tracks(streams: &[MediaStream], choice: TrackChoice) -> SelectedTracks {
    let of_kind = |kind: StreamKind| streams.iter().filter(move |stream| stream.kind == kind);
    let chosen = |kind: StreamKind, id: StreamId| of_kind(kind).find(|stream| stream.id == id);

    let audio = choice
        .audio
        .and_then(|id| chosen(StreamKind::Audio, id))
        .or_else(|| of_kind(StreamKind::Audio).find(|stream| stream.is_default))
        .or_else(|| of_kind(StreamKind::Audio).next());
    let subtitle = match choice.subtitle {
        Some(SubtitleChoice::Off) => None,
        Some(SubtitleChoice::Track(id)) if chosen(StreamKind::Subtitle, id).is_some() => Some(id),
        _ => audio.and_then(|audio| {
            of_kind(StreamKind::Subtitle)
                .find(|stream| stream.is_forced && stream.language == audio.language)
                .map(|stream| stream.id)
        }),
    };
    SelectedTracks {
        audio: audio.map(|stream| stream.id),
        subtitle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::PartId;
    use crate::metadata::TrackTraits;

    fn stream(
        id: i64,
        kind: StreamKind,
        language: &str,
        default: bool,
        forced: bool,
    ) -> MediaStream {
        MediaStream {
            id: StreamId::new(id),
            file_id: PartId::new(1),
            kind,
            position: 0,
            codec: None,
            language: Some(language.to_owned()),
            channels: None,
            bitrate: None,
            is_default: default,
            is_forced: forced,
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
            stated: crate::metadata::StatedTrack::default(),
        }
    }

    /// Soul on the reference: French audio by default, English beside it, and three French
    /// subtitles, the forced one first.
    fn soul() -> Vec<MediaStream> {
        vec![
            stream(78, StreamKind::Video, "en", true, false),
            stream(73, StreamKind::Audio, "fr", true, false),
            stream(74, StreamKind::Audio, "en", false, false),
            stream(75, StreamKind::Subtitle, "fr", true, true),
            stream(76, StreamKind::Subtitle, "fr", false, false),
            stream(77, StreamKind::Subtitle, "fr", false, false),
        ]
    }

    fn picked(choice: TrackChoice) -> (Option<i64>, Option<i64>) {
        let selected = selected_tracks(&soul(), choice);
        (
            selected.audio.map(StreamId::get),
            selected.subtitle.map(StreamId::get),
        )
    }

    #[test]
    fn with_no_choice_the_forced_subtitle_of_the_audios_language_plays() {
        assert_eq!(picked(TrackChoice::default()), (Some(73), Some(75)));
    }

    #[test]
    fn other_audio_drops_forced_subtitles_of_another_language() {
        let english = TrackChoice {
            audio: Some(StreamId::new(74)),
            subtitle: None,
        };
        assert_eq!(picked(english), (Some(74), None));
    }

    #[test]
    fn a_choice_wins_and_off_is_a_choice() {
        let chosen = TrackChoice {
            audio: None,
            subtitle: Some(SubtitleChoice::Track(StreamId::new(77))),
        };
        assert_eq!(picked(chosen), (Some(73), Some(77)));
        let off = TrackChoice {
            audio: None,
            subtitle: Some(SubtitleChoice::Off),
        };
        assert_eq!(picked(off), (Some(73), None));
    }

    #[test]
    fn a_track_the_file_no_longer_has_is_ignored() {
        let stale = TrackChoice {
            audio: Some(StreamId::new(999)),
            subtitle: Some(SubtitleChoice::Track(StreamId::new(73))),
        };
        // 73 is audio, not a subtitle: the automatic choice stands for both.
        assert_eq!(picked(stale), (Some(73), Some(75)));
    }
}
