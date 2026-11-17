//! Chromaprint fingerprints, what intro detection compares across a season's episodes.

use std::io::Read;
use std::path::Path;
use std::process::Command;

use rusty_chromaprint::{Configuration, Fingerprinter};

use crate::report::Fingerprint;
use crate::{input, loudness, probe, seconds, ScanError, Streaming, Tools};

/// Chromaprint works on mono audio at this rate; ffmpeg resamples to it, as `fpcalc` does.
const SAMPLE_RATE: u32 = 11_025;

/// The least the reference fingerprints: whole episodes under ten minutes, ten minutes of the
/// others' first half.
const MIN_LENGTH_MS: u64 = 600_000;

/// Fingerprints the start of an audio track — the first one when `track` is `None` — over
/// `length_ms`, or by the reference's measured rule when `None`: half the duration, at least
/// ten minutes, at most all of it.
///
/// # Errors
///
/// [`ScanError::NoSuchTrack`] when `track` is not an audio track, [`ScanError::Unreadable`]
/// when ffmpeg cannot decode it.
pub fn fingerprint(
    tools: &Tools,
    path: &Path,
    track: Option<u32>,
    length_ms: Option<u64>,
) -> Result<Fingerprint, ScanError> {
    let track = loudness::audio_track(tools, path, track)?;
    let length_ms = match length_ms {
        Some(length) => length,
        None => default_length(probe::duration_ms(tools, path)?),
    };
    let mut child = Streaming::spawn(
        "ffmpeg",
        Command::new(&tools.ffmpeg)
            .args(["-hide_banner", "-nostats", "-loglevel", "error", "-i"])
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
                "s16le",
                "-acodec",
                "pcm_s16le",
                "-",
            ]),
    )?;
    let configuration = Configuration::preset_test2();
    let mut printer = Fingerprinter::new(&configuration);
    printer
        .start(SAMPLE_RATE, 1)
        .map_err(|error| ScanError::Output {
            tool: "chromaprint",
            message: format!("{error:?}"),
        })?;
    let mut stdout = child.stdout()?;
    let mut pending: Vec<u8> = Vec::with_capacity(64 * 1024 + 1);
    let mut buffer = vec![0; 64 * 1024];
    let mut samples: Vec<i16> = Vec::with_capacity(buffer.len() / 2 + 1);
    let mut sample_count: u64 = 0;
    loop {
        let read = stdout
            .read(&mut buffer)
            .map_err(|error| ScanError::Output {
                tool: "ffmpeg",
                message: error.to_string(),
            })?;
        if read == 0 {
            break;
        }
        // A read can end between the two bytes of a sample; the odd byte waits for the next.
        pending.extend_from_slice(&buffer[..read]);
        let whole = pending.len() - pending.len() % 2;
        samples.clear();
        samples.extend(
            pending[..whole]
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]])),
        );
        pending.drain(..whole);
        sample_count += samples.len() as u64;
        printer.consume(&samples);
    }
    child.finish()?;
    printer.finish();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "about 123,810 microseconds, positive by construction"
    )]
    let item_duration_us =
        (f64::from(configuration.item_duration_in_seconds()) * 1e6).round() as u64;
    Ok(Fingerprint {
        track,
        algorithm: 2,
        duration_ms: sample_count * 1000 / u64::from(SAMPLE_RATE),
        item_duration_us,
        values: printer.fingerprint().to_vec(),
    })
}

fn default_length(duration_ms: Option<u64>) -> u64 {
    match duration_ms {
        Some(duration) => (duration / 2).max(MIN_LENGTH_MS).min(duration),
        None => MIN_LENGTH_MS,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn covers_half_an_episode_and_ten_minutes_at_least() {
        assert_eq!(default_length(Some(2_467_160)), 1_233_580);
        assert_eq!(default_length(Some(840_000)), 600_000);
        assert_eq!(default_length(Some(420_000)), 420_000);
        assert_eq!(default_length(None), 600_000);
    }
}
