//! Chromaprint fingerprints, what intro detection compares across a season's episodes.

use std::path::Path;

use tokio::process::Command;

use crate::engine::{input, loudness, output, probe, seconds, ScanError, Tools};
use crate::report::Fingerprint;

/// Chromaprint works on mono audio at this rate; ffmpeg resamples to it, as `fpcalc` does.
const SAMPLE_RATE: u32 = 11_025;

/// One value every 4096 − 2731 samples at 11,025 Hz: Chromaprint's frame less its overlap.
const ITEM_DURATION_US: u64 = 123_810;

/// The least the reference fingerprints: whole episodes under ten minutes, ten minutes of the
/// others' first half.
const MIN_LENGTH_MS: u64 = 600_000;

/// Fingerprints the start of an audio track — the first one when `track` is `None` — over
/// `length_ms`, or by the reference's measured rule when `None`: half the duration, at least
/// ten minutes, at most all of it.
///
/// ffmpeg-plex computes it, built with Chromaprint and its bundled FFT on every platform, so
/// that a file has one fingerprint wherever it was read: the values `fpcalc -raw` prints, what
/// Plex's Fingerprinter stores.
///
/// # Errors
///
/// [`ScanError::NoSuchTrack`] when `track` is not an audio track, [`ScanError::Unreadable`]
/// when ffmpeg cannot decode it or has no Chromaprint.
pub async fn fingerprint(
    tools: &Tools,
    path: &Path,
    track: Option<u32>,
    length_ms: Option<u64>,
) -> Result<Fingerprint, ScanError> {
    let track = loudness::audio_track(tools, path, track).await?;
    let length_ms = match length_ms {
        Some(length) => length,
        None => default_length(probe::duration_ms(tools, path).await?),
    };
    let output = output(
        "ffmpeg",
        Command::new(&tools.ffmpeg)
            .args(["-hide_banner", "-nostats", "-loglevel", "error"])
            // The progress report's last `out_time_us` is how much audio reached the muxer.
            .args(["-progress", "pipe:2", "-i"])
            .arg(input(path))
            .args([
                "-map",
                &format!("0:{track}"),
                "-t",
                &seconds(length_ms),
                "-ac",
                "1",
                "-ar",
                &SAMPLE_RATE.to_string(),
                "-f",
                "chromaprint",
                // Chromaprint's default algorithm, `fpcalc`'s 2.
                "-algorithm",
                "1",
                "-fp_format",
                "raw",
                "-",
            ]),
    )
    .await?;
    Ok(Fingerprint {
        track,
        algorithm: 2,
        duration_ms: fingerprinted_us(&output.stderr).map_or(0, |us| us / 1000),
        item_duration_us: ITEM_DURATION_US,
        values: values(&output.stdout),
    })
}

/// The raw fingerprint: 32-bit values in the byte order of the machine that computed it, which
/// is little-endian on every platform the server is built for.
fn values(raw: &[u8]) -> Vec<u32> {
    raw.chunks_exact(4)
        .map(|value| u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
        .collect()
}

/// The last `out_time_us=` of ffmpeg's progress report.
fn fingerprinted_us(progress: &[u8]) -> Option<u64> {
    String::from_utf8_lossy(progress)
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("out_time_us=")?.trim().parse().ok())
}

fn default_length(duration_ms: Option<u64>) -> u64 {
    match duration_ms {
        Some(duration) => (duration / 2).max(MIN_LENGTH_MS).min(duration),
        None => MIN_LENGTH_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_half_an_episode_and_ten_minutes_at_least() {
        assert_eq!(default_length(Some(2_467_160)), 1_233_580);
        assert_eq!(default_length(Some(840_000)), 600_000);
        assert_eq!(default_length(Some(420_000)), 420_000);
        assert_eq!(default_length(None), 600_000);
    }

    #[test]
    fn reads_the_raw_values_and_how_much_audio_they_cover() {
        assert_eq!(
            values(&[0x78, 0x56, 0x34, 0x12, 1, 0, 0, 0, 9]),
            [0x1234_5678, 1]
        );
        let progress = b"out_time_us=123000\nprogress=continue\nout_time_us=N/A\n\
            out_time_us=600000000\nprogress=end\n";
        assert_eq!(fingerprinted_us(progress), Some(600_000_000));
        assert_eq!(fingerprinted_us(b"progress=end\n"), None);
    }
}
