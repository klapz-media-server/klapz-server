//! An H.264 track's sequence parameter set, read with ffmpeg's `trace_headers`.
//!
//! The reference reads its coded size, reference frames and scaling matrix from there, where
//! ffprobe reports the cropped size (1080 rather than 1088) and a reference count libavcodec
//! never fills in. Copying the first packet through the bitstream filter prints the header
//! field by field: the decoder is never involved.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::engine::{input, Tools};
use crate::report::SequenceHeader;

/// `None` when the track's first packet carries no parameter set, or ffmpeg cannot copy it:
/// these figures complete an analysis and are never worth failing one.
pub(super) fn read(tools: &Tools, path: &Path, stream: u32) -> Option<SequenceHeader> {
    let output = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-v", "info", "-i"])
        .arg(input(path))
        .args(["-map", &format!("0:{stream}")])
        .args(["-c", "copy", "-bsf:v", "trace_headers"])
        .args(["-frames:v", "1", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| parse(&String::from_utf8_lossy(&output.stderr)))
        .flatten()
}

/// The first sequence parameter set of ffmpeg's `trace_headers` output. Each field is a line
/// `[trace_headers @ …] <bit position> <name> <bits> = <value>`.
pub fn parse(trace: &str) -> Option<SequenceHeader> {
    let mut fields: Vec<(&str, u32)> = Vec::new();
    let mut inside = false;
    for line in trace.lines() {
        let Some((_, traced)) = line.split_once("[trace_headers @ ") else {
            continue;
        };
        let Some((_, traced)) = traced.split_once("] ") else {
            continue;
        };
        if traced == "Sequence Parameter Set" {
            if inside {
                break;
            }
            inside = true;
        } else if inside && !traced.starts_with(|first: char| first.is_ascii_digit()) {
            // The next unit's title: the parameter set is over.
            break;
        } else if inside {
            let mut words = traced.split_whitespace();
            let (Some(_position), Some(name)) = (words.next(), words.next()) else {
                continue;
            };
            let value = traced
                .rsplit_once("= ")
                .and_then(|(_, value)| value.parse().ok());
            if let Some(value) = value {
                fields.push((name, value));
            }
        }
    }
    let field = |name: &str| {
        fields
            .iter()
            .find(|(field, _)| *field == name)
            .map(|(_, value)| *value)
    };
    let width_in_macroblocks = field("pic_width_in_mbs_minus1")? + 1;
    let height_in_map_units = field("pic_height_in_map_units_minus1")? + 1;
    // A map unit is a macroblock pair when fields may be coded apart.
    let units_per_column = 2 - field("frame_mbs_only_flag")?.min(1);
    Some(SequenceHeader {
        coded_width: width_in_macroblocks * 16,
        coded_height: height_in_map_units * units_per_column * 16,
        ref_frames: field("max_num_ref_frames")?,
        // Absent below the High profiles, which cannot carry one.
        has_scaling_matrix: field("seq_scaling_matrix_present_flag").is_some_and(|flag| flag != 0),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Soul's video track, as ffmpeg-plex 8.1.3 traces it (fields between those kept elided).
    const SOUL: &str = "\
[trace_headers @ 00000119c3b98480] Extradata
[trace_headers @ 00000119c3b98480] Sequence Parameter Set
[trace_headers @ 00000119c3b98480] 8           profile_idc                                          01100100 = 100
[trace_headers @ 00000119c3b98480] 39          seq_scaling_matrix_present_flag                             0 = 0
[trace_headers @ 00000119c3b98480] 45          max_num_ref_frames                                      00101 = 4
[trace_headers @ 00000119c3b98480] 51          pic_width_in_mbs_minus1                         0000001111000 = 119
[trace_headers @ 00000119c3b98480] 64          pic_height_in_map_units_minus1                  0000001000100 = 67
[trace_headers @ 00000119c3b98480] 77          frame_mbs_only_flag                                         1 = 1
[trace_headers @ 00000119c3b98480] 79          frame_cropping_flag                                         1 = 1
[trace_headers @ 00000119c3b98480] 83          frame_crop_bottom_offset                                00101 = 4
[trace_headers @ 00000119c3b98480] Picture Parameter Set
[trace_headers @ 00000119c3b98480] 24          seq_parameter_set_id                                        1 = 0
";

    #[test]
    fn reads_the_uncropped_size_and_reference_frames_the_reference_stores() {
        assert_eq!(
            parse(SOUL),
            Some(SequenceHeader {
                coded_width: 1920,
                coded_height: 1088,
                ref_frames: 4,
                has_scaling_matrix: false,
            })
        );
    }

    #[test]
    fn counts_macroblock_pairs_when_fields_may_be_coded_apart() {
        let interlaced = SOUL.replace(
            "frame_mbs_only_flag                                         1 = 1",
            "frame_mbs_only_flag                                         0 = 0",
        );
        let header = parse(&interlaced.replace("= 67", "= 33")).unwrap();
        assert_eq!(header.coded_height, 1088);
    }

    #[test]
    fn a_trace_without_a_parameter_set_says_nothing() {
        assert_eq!(
            parse(&SOUL.replace("Sequence Parameter Set", "Slice")),
            None
        );
        assert_eq!(parse("Stream mapping:\n  Stream #0:0 -> #0:0 (copy)"), None);
    }
}
