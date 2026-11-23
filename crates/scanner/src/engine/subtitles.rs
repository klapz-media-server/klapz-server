//! An embedded subtitle track, extracted to a file of its own.

use std::path::Path;
use tokio::process::Command;

use crate::engine::{input, output_directory, probe, produced, run, ScanError, Tools};
use crate::report::{FileRole, Produced, TrackKind};

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
    let (extension, encoder) = match codec.as_str() {
        "subrip" => ("srt", "copy"),
        "ass" | "ssa" => ("ass", "copy"),
        "webvtt" => ("vtt", "copy"),
        "mov_text" => ("srt", "srt"),
        "hdmv_pgs_subtitle" => ("sup", "copy"),
        other => {
            return Err(ScanError::Unsupported(format!(
                "no file format holds a {other} subtitle track by itself"
            )))
        }
    };
    let name = format!("stream-{track}.{extension}");
    run(
        "ffmpeg",
        Command::new(&tools.ffmpeg)
            .args(["-hide_banner", "-nostats", "-loglevel", "error", "-n", "-i"])
            .arg(input(path))
            .args(["-map", &format!("0:{track}"), "-c:s", encoder])
            .arg(input(&directory.join(&name))),
    )
    .await?;
    Ok(Produced {
        files: vec![produced(
            &directory,
            name,
            FileRole::Subtitle { track, codec },
        )?],
    })
}
