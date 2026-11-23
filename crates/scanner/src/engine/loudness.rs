//! EBU R 128 loudness, through ffmpeg's `ebur128` filter.

use std::path::Path;
use tokio::process::Command;

use crate::engine::{input, output, probe, ScanError, Tools};
use crate::report::{Loudness, TrackKind};

/// Measures one audio track whole, decoding it: the first one when `track` is `None`.
///
/// # Errors
///
/// [`ScanError::NoSuchTrack`] when `track` is not an audio track, [`ScanError::Unreadable`]
/// when ffmpeg cannot decode it.
pub async fn loudness(
    tools: &Tools,
    path: &Path,
    track: Option<u32>,
) -> Result<Loudness, ScanError> {
    let track = audio_track(tools, path, track).await?;
    // The filter prints its summary at the info level, on standard error, and each frame's
    // measurement at the level `framelog` names, here one that is not shown.
    let output = output(
        "ffmpeg",
        Command::new(&tools.ffmpeg)
            .args(["-hide_banner", "-nostats", "-loglevel", "info", "-i"])
            .arg(input(path))
            .args([
                "-map",
                &format!("0:{track}"),
                "-af",
                "ebur128=peak=true:framelog=verbose",
                "-f",
                "null",
                "-",
            ]),
    )
    .await?;
    summary(&String::from_utf8_lossy(&output.stderr), track).ok_or_else(|| ScanError::Output {
        tool: "ffmpeg",
        message: "no ebur128 summary".to_owned(),
    })
}

/// The track asked for, provided it is audio, or the first audio track.
pub(crate) async fn audio_track(
    tools: &Tools,
    path: &Path,
    track: Option<u32>,
) -> Result<u32, ScanError> {
    let tracks = probe::tracks(tools, path).await?;
    let mut audio = tracks
        .iter()
        .filter(|candidate| candidate.kind == TrackKind::Audio);
    match track {
        Some(index) => audio
            .find(|candidate| candidate.index == index)
            .map(|candidate| candidate.index)
            .ok_or(ScanError::NoSuchTrack(index)),
        None => audio
            .next()
            .map(|candidate| candidate.index)
            .ok_or_else(|| ScanError::Unsupported("the file has no audio track".to_owned())),
    }
}

/// Reads the filter's closing summary:
///
/// ```text
///   Integrated loudness:
///     I:         -23.0 LUFS
///     Threshold: -33.4 LUFS
///   Loudness range:
///     LRA:         6.2 LU
///     Threshold: -43.5 LUFS
///     LRA low:   -27.3 LUFS
///     LRA high:  -21.1 LUFS
///   True peak:
///     Peak:       -1.2 dBFS
/// ```
fn summary(stderr: &str, track: u32) -> Option<Loudness> {
    let summary = &stderr[stderr.rfind("Summary:")?..];
    let mut section = "";
    let mut integrated = None;
    let mut threshold = None;
    let mut range = None;
    let mut range_low = None;
    let mut range_high = None;
    let mut true_peak = None;
    for line in summary.lines().map(str::trim) {
        if line.ends_with(':') {
            section = line;
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value
            .split_whitespace()
            .next()
            .and_then(|number| number.parse::<f64>().ok());
        match (section, name.trim()) {
            (_, "I") => integrated = value,
            ("Integrated loudness:", "Threshold") => threshold = value,
            (_, "LRA") => range = value,
            (_, "LRA low") => range_low = value,
            (_, "LRA high") => range_high = value,
            ("True peak:", "Peak") => true_peak = value,
            _ => {}
        }
    }
    Some(Loudness {
        track,
        integrated: integrated?,
        threshold: threshold?,
        range: range?,
        range_low: range_low?,
        range_high: range_high?,
        true_peak: true_peak?,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn reads_the_last_summary() {
        let stderr = "\
[Parsed_ebur128_0 @ 0000] Summary:

  Integrated loudness:
    I:         -23.0 LUFS
    Threshold: -33.4 LUFS

  Loudness range:
    LRA:         6.2 LU
    Threshold: -43.5 LUFS
    LRA low:   -27.3 LUFS
    LRA high:  -21.1 LUFS

  True peak:
    Peak:       -1.2 dBFS
";
        let loudness = summary(stderr, 1).unwrap();
        assert!((loudness.integrated - -23.0_f64).abs() < 1e-9);
        assert!((loudness.threshold - -33.4_f64).abs() < 1e-9);
        assert!((loudness.range - 6.2_f64).abs() < 1e-9);
        assert!((loudness.range_low - -27.3_f64).abs() < 1e-9);
        assert!((loudness.range_high - -21.1_f64).abs() < 1e-9);
        assert!((loudness.true_peak - -1.2_f64).abs() < 1e-9);
    }
}
