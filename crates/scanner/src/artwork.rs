//! Pictures for an item without artwork: frames of its video, and covers its file carries.

use std::path::Path;
use std::process::Command;

use crate::report::{FileRole, Produced, ProducedFile, TrackKind};
use crate::{input, output_directory, probe, produced, run, seconds, ScanError, Tools};

/// Chapters past this get no thumbnails: a file declaring thousands is broken or hostile,
/// and each costs a decode.
const MAX_CHAPTERS: usize = 200;

/// Where the reference draws a poster and a backdrop from, as fractions of the duration.
const ARTWORK_POSITIONS: [(u64, u64); 3] = [(1, 4), (1, 2), (3, 4)];

/// Into `out`: `thumb{1,2,3}.jpg` fitted inside 720×512 and `art{1,2,3}.jpg` inside
/// 1280×720, the same frame by number, at a quarter, half and three quarters of the video, as
/// the reference draws them; and `cover-{track}.{jpg,png}` for each picture the file carries.
/// Choosing among them is the caller's.
///
/// # Errors
///
/// [`ScanError::Unreadable`] when ffmpeg cannot decode the video.
pub fn artwork(tools: &Tools, path: &Path, out: &Path) -> Result<Produced, ScanError> {
    let directory = output_directory(out)?;
    let mut files = Vec::new();
    if let Some(duration) = probe::duration_ms(tools, path)?.filter(|duration| *duration > 0) {
        for (number, (numerator, denominator)) in (1..).zip(ARTWORK_POSITIONS) {
            let at_ms = duration * numerator / denominator;
            let thumb = format!("thumb{number}.jpg");
            let art = format!("art{number}.jpg");
            frame(tools, path, at_ms, (720, 512), &directory.join(&thumb))?;
            frame(tools, path, at_ms, (1280, 720), &directory.join(&art))?;
            files.push(produced(&directory, thumb, FileRole::Thumb { at_ms })?);
            files.push(produced(&directory, art, FileRole::Art { at_ms })?);
        }
    }
    files.extend(covers(tools, path, &directory)?);
    Ok(Produced { files })
}

/// Into `out`, `chapter{index}.jpg` for each chapter, fitted inside 720×512, a little after
/// its start, where a fade from the previous chapter has ended.
///
/// # Errors
///
/// [`ScanError::Unsupported`] past [`MAX_CHAPTERS`], [`ScanError::Unreadable`] when ffmpeg
/// cannot decode the video.
pub fn chapter_thumbs(tools: &Tools, path: &Path, out: &Path) -> Result<Produced, ScanError> {
    let directory = output_directory(out)?;
    let chapters = probe::chapters_of(tools, path)?;
    if chapters.len() > MAX_CHAPTERS {
        return Err(ScanError::Unsupported(format!(
            "{} chapters, past the {MAX_CHAPTERS} given thumbnails",
            chapters.len()
        )));
    }
    let mut files = Vec::with_capacity(chapters.len());
    for chapter in chapters {
        let length = chapter.end_ms.saturating_sub(chapter.start_ms);
        let at_ms = chapter.start_ms + (length / 10).min(5_000);
        let name = format!("chapter{}.jpg", chapter.index);
        frame(tools, path, at_ms, (720, 512), &directory.join(&name))?;
        files.push(produced(
            &directory,
            name,
            FileRole::ChapterThumb {
                chapter: chapter.index,
                at_ms,
            },
        )?);
    }
    Ok(Produced { files })
}

/// One frame of the main video at `at_ms`, fitted inside `box_size`. The seek lands on the key
/// frame before and decodes forward, so the frame is the exact one.
fn frame(
    tools: &Tools,
    path: &Path,
    at_ms: u64,
    (width, height): (u32, u32),
    target: &Path,
) -> Result<(), ScanError> {
    run(
        "ffmpeg",
        Command::new(&tools.ffmpeg)
            .args([
                "-hide_banner",
                "-nostats",
                "-loglevel",
                "error",
                "-n",
                "-ss",
                &seconds(at_ms),
                "-i",
            ])
            .arg(input(path))
            .args([
                "-map",
                "0:V:0",
                "-frames:v",
                "1",
                "-vf",
                // Fitted, never enlarged: a small source gives a small picture.
                &format!(
                    "scale=w='min(iw,{width})':h='min(ih,{height})':force_original_aspect_ratio=decrease"
                ),
                "-q:v",
                "3",
            ])
            .arg(input(target)),
    )?;
    Ok(())
}

/// Attached pictures (MP4 and Matroska covers stored as a video track) and Matroska image
/// attachments, copied as stored.
fn covers(tools: &Tools, path: &Path, directory: &Path) -> Result<Vec<ProducedFile>, ScanError> {
    let mut files = Vec::new();
    for track in probe::tracks(tools, path)? {
        let extension = match track.kind {
            TrackKind::Video if track.disposition.attached_picture => {
                match track.codec.as_deref() {
                    Some("mjpeg") => "jpg",
                    Some("png") => "png",
                    _ => continue,
                }
            }
            TrackKind::Attachment => {
                match track
                    .attachment
                    .as_ref()
                    .and_then(|attachment| attachment.mime_type.as_deref())
                {
                    Some("image/jpeg") => "jpg",
                    Some("image/png") => "png",
                    _ => continue,
                }
            }
            _ => continue,
        };
        let name = format!("cover-{}.{extension}", track.index);
        let target = directory.join(&name);
        let mut command = Command::new(&tools.ffmpeg);
        command.args(["-hide_banner", "-nostats", "-loglevel", "error", "-n"]);
        if track.kind == TrackKind::Attachment {
            // An attachment is no stream ffmpeg can map; it is written out while the input
            // opens, and the output that must follow takes nothing.
            command
                .arg(format!("-dump_attachment:{}", track.index))
                .arg(input(&target))
                .arg("-i")
                .arg(input(path))
                .args(["-frames:v", "0", "-map", "0:V:0?", "-f", "null", "-"]);
        } else {
            command
                .arg("-i")
                .arg(input(path))
                .args([
                    "-map",
                    &format!("0:{}", track.index),
                    "-c",
                    "copy",
                    "-frames:v",
                    "1",
                ])
                .arg(input(&target));
        }
        run("ffmpeg", &mut command)?;
        files.push(produced(
            directory,
            name,
            FileRole::Cover { track: track.index },
        )?);
    }
    Ok(files)
}
