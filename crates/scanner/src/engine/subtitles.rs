//! Embedded subtitle tracks, extracted to files of their own.

use std::path::Path;
use tokio::process::Command;

use crate::engine::{input, output_directory, probe, produced, run, ScanError, Tools};
use crate::report::{FileRole, Produced, Track, TrackKind};

/// Writes subtitle `track` into `out` as `stream-{track}.{extension}`. Text formats are copied
/// as they are muxed, except MP4's `mov_text`, which no file format holds and becomes `SubRip`;
/// PGS is copied as a `.sup`. DVD and DVB bitmaps have no single-file form ffmpeg writes, and
/// are refused.
///
/// # Errors
///
/// [`ScanError::NoSuchTrack`] when `track` is not a subtitle track,
/// [`ScanError::Unsupported`] for a format with no file of its own.
pub async fn extract_subtitle(
    tools: &Tools,
    path: &Path,
    track: u32,
    out: &Path,
) -> Result<Produced, ScanError> {
    let directory = output_directory(out)?;
    let codec = probe::tracks(tools, path)
        .await?
        .into_iter()
        .find(|candidate| candidate.index == track && candidate.kind == TrackKind::Subtitle)
        .ok_or(ScanError::NoSuchTrack(track))?
        .codec
        .unwrap_or_default();
    let Some(format) = file_format(&codec) else {
        return Err(ScanError::Unsupported(format!(
            "no file format holds a {codec} subtitle track by itself"
        )));
    };
    extract(tools, path, &directory, &[(track, codec, format)]).await
}

/// Writes every subtitle track a file format holds by itself into `out`, named and converted
/// as [`extract_subtitle`] does, in one run that reads the file once. A track with no file of
/// its own — DVD and DVB bitmaps, teletext — is left out; a file without any writes nothing.
///
/// # Errors
///
/// [`ScanError::Unreadable`] when ffmpeg cannot read the file.
pub async fn extract_subtitles(
    tools: &Tools,
    path: &Path,
    out: &Path,
) -> Result<Produced, ScanError> {
    let directory = output_directory(out)?;
    let tracks: Vec<(u32, String, Format)> = probe::tracks(tools, path)
        .await?
        .into_iter()
        .filter(|track| track.kind == TrackKind::Subtitle)
        .filter_map(|Track { index, codec, .. }| {
            let codec = codec.unwrap_or_default();
            file_format(&codec).map(|format| (index, codec, format))
        })
        .collect();
    if tracks.is_empty() {
        return Ok(Produced { files: Vec::new() });
    }
    extract(tools, path, &directory, &tracks).await
}

/// A file's extension, and the encoder that writes it from the track.
type Format = (&'static str, &'static str);

fn file_format(codec: &str) -> Option<Format> {
    Some(match codec {
        "subrip" => ("srt", "copy"),
        "ass" | "ssa" => ("ass", "copy"),
        "webvtt" => ("vtt", "copy"),
        "mov_text" => ("srt", "srt"),
        "hdmv_pgs_subtitle" => ("sup", "copy"),
        _ => return None,
    })
}

/// One output per track, each mapped and encoded on its own.
async fn extract(
    tools: &Tools,
    path: &Path,
    directory: &Path,
    tracks: &[(u32, String, Format)],
) -> Result<Produced, ScanError> {
    let mut command = Command::new(&tools.ffmpeg);
    command
        .args(["-hide_banner", "-nostats", "-loglevel", "error", "-n", "-i"])
        .arg(input(path));
    let names: Vec<String> = tracks
        .iter()
        .map(|(track, _, (extension, _))| format!("stream-{track}.{extension}"))
        .collect();
    for ((track, _, (_, encoder)), name) in tracks.iter().zip(&names) {
        command
            .args(["-map", &format!("0:{track}"), "-c:s", encoder])
            .arg(input(&directory.join(name)));
    }
    run("ffmpeg", &mut command).await?;
    let mut files = Vec::with_capacity(tracks.len());
    for ((track, codec, _), name) in tracks.iter().zip(names) {
        files.push(produced(
            directory,
            name,
            FileRole::Subtitle {
                track: *track,
                codec: codec.clone(),
            },
        )?);
    }
    Ok(Produced { files })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_track_is_extracted_only_to_a_format_that_holds_it_alone() {
        assert_eq!(file_format("subrip"), Some(("srt", "copy")));
        assert_eq!(file_format("mov_text"), Some(("srt", "srt")));
        assert_eq!(file_format("hdmv_pgs_subtitle"), Some(("sup", "copy")));
        for bitmap in ["dvd_subtitle", "dvb_subtitle", "dvb_teletext", ""] {
            assert_eq!(file_format(bitmap), None, "{bitmap}");
        }
    }
}
