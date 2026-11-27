//! Subtitles extracted whole, every track of a file a player can be given in one pass of the
//! transcoder: text in a text format, pictures as TTML.
//!
//! A player gets a whole track at once, on the media's timeline, whatever the stream it goes
//! with is doing: a seek or a change of quality restarts the stream's process but not this.
//! A stream's start extracts every text track of its file in the formats its player reads, and
//! what was extracted is kept on disk, in a directory per file below the transcoder's, so that
//! switching to another track finds it ready.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest as _, Sha256};
use tokio::process::Command;
use tokio::sync::{watch, Semaphore};
use tokio::task::AbortHandle;

use crate::plan::Track;
use crate::TranscodeError;

/// The most an extracted text subtitle may weigh. A film's run to some hundred kilobytes; ASS
/// karaoke a few megabytes.
const MAX_TEXT_BYTES: u64 = 32 * 1024 * 1024;

/// The most a TTML document of pictures may weigh: Daria's 74 subtitles in Base64 weigh 820
/// kilobytes, so a film's two thousand some 25 megabytes.
const MAX_TTML_BYTES: u64 = 128 * 1024 * 1024;

/// What every file's subtitles may weigh together: a film's tracks weigh some hundred
/// kilobytes each, a broadcast's teletext stream kept as it is some 60 megabytes.
const MAX_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// How often the subtitles are brought back within their budget.
const PRUNE_EVERY: Duration = Duration::from_secs(10 * 60);

/// How a file's directory of subtitles begins, below the transcoder's.
const SUBTITLE_DIRECTORY: &str = "subtitle-";

/// How what a pass is writing begins, and how old it is when no pass can still be writing it:
/// an older transcoder reads a whole file, which over a share takes minutes.
const PARTIAL: &str = ".partial-";
const ABANDONED_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

/// A format a player reads subtitles in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubtitleFormat {
    Ass,
    WebVtt,
    Srt,
    /// TTML holding pictures: IMSC1 Image's regions, each image in Base64 in the document, as
    /// ffmpeg-plex's `imsc_image` muxer writes them from v8.1.3-69.
    Ttml,
}

/// Whether a track of this codec is text the transcoder converts into any of the text formats.
#[must_use]
pub fn is_text(codec: &str) -> bool {
    matches!(
        codec,
        "srt" | "subrip" | "ass" | "ssa" | "mov_text" | "webvtt" | "vtt" | "text"
    )
}

/// Whether a track of this codec is a broadcast's teletext, whose subtitles libzvbi decodes
/// into text from one of its pages. Not text to every command: only extraction decodes it as
/// such, so it is neither burned in nor drawn with libass.
#[must_use]
pub fn is_teletext(codec: &str) -> bool {
    matches!(codec, "dvb_teletext")
}

/// Whether a track of this codec is pictures, from a Blu-ray, a DVD or a broadcast.
#[must_use]
pub fn is_image(codec: &str) -> bool {
    matches!(
        codec,
        "pgs"
            | "hdmv_pgs_subtitle"
            | "vobsub"
            | "dvd_subtitle"
            | "dvdsub"
            | "dvb_subtitle"
            | "dvbsub"
            | "xsub"
    )
}

impl SubtitleFormat {
    /// Whether a track of this codec converts into this format: text into text, pictures into
    /// TTML.
    #[must_use]
    pub fn holds(self, codec: &str) -> bool {
        match self {
            Self::Ttml => is_image(codec),
            Self::Ass | Self::WebVtt | Self::Srt => is_text(codec) || is_teletext(codec),
        }
    }

    /// The format a track already is, for those whose packets go into a file of it untouched.
    fn of_codec(codec: &str) -> Option<Self> {
        match codec {
            "ass" => Some(Self::Ass),
            "webvtt" | "vtt" => Some(Self::WebVtt),
            "srt" | "subrip" => Some(Self::Srt),
            _ => None,
        }
    }

    /// The name of ffmpeg's encoder for it.
    const fn encoder(self) -> &'static str {
        match self {
            Self::Ass => "ass",
            Self::WebVtt => "webvtt",
            Self::Srt => "subrip",
            Self::Ttml => "imsc_image",
        }
    }

    /// The name of ffmpeg's muxer for it.
    const fn muxer(self) -> &'static str {
        match self {
            Self::Ass => "ass",
            Self::WebVtt => "webvtt",
            Self::Srt => "srt",
            Self::Ttml => "imsc_image",
        }
    }

    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Ass => "ass",
            Self::WebVtt => "vtt",
            Self::Srt => "srt",
            Self::Ttml => "ttml",
        }
    }

    /// As players and DASH manifests name it.
    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Ass => "text/x-ssa",
            Self::WebVtt => "text/vtt",
            Self::Srt => "application/x-subrip",
            Self::Ttml => "application/ttml+xml",
        }
    }

    const fn max_bytes(self) -> u64 {
        match self {
            Self::Ttml => MAX_TTML_BYTES,
            Self::Ass | Self::WebVtt | Self::Srt => MAX_TEXT_BYTES,
        }
    }
}

/// One track to extract and the file it goes to.
#[derive(Debug, Clone)]
struct Output {
    track: Track,
    format: SubtitleFormat,
    path: PathBuf,
    /// For a teletext track, where its stream is kept as it is, which it is decoded from.
    copy: Option<PathBuf>,
}

/// A teletext stream copied as it is from the source, which its pages are then decoded from.
#[derive(Debug, Clone)]
struct Copied {
    index: i32,
    /// Where the pass writes it, to be moved to its place once whole.
    path: PathBuf,
}

/// The options every pass starts with.
fn common_arguments() -> Vec<String> {
    [
        "-nostdin",
        "-loglevel",
        "error",
        "-analyzeduration",
        "20000000",
        "-probesize",
        "20000000",
    ]
    .iter()
    .map(ToString::to_string)
    .collect()
}

/// How `output` is written once its track is mapped: copied when the track already is the
/// format asked, converted otherwise.
fn output_arguments(arguments: &mut Vec<String>, output: &Output) {
    let codec = if SubtitleFormat::of_codec(&output.track.codec) == Some(output.format) {
        "copy"
    } else {
        output.format.encoder()
    };
    arguments.extend([
        "-codec:0".to_owned(),
        codec.to_owned(),
        "-f".to_owned(),
        output.format.muxer().to_owned(),
    ]);
    if output.format == SubtitleFormat::Ttml {
        // One file, as every other format is kept and served, rather than one per picture.
        arguments.extend(["-embed_images".to_owned(), "1".to_owned()]);
    }
    arguments.push(format!("file:{}", output.path.display()));
}

/// The command reading `source` once: every one of `outputs` into its own file, and each of
/// `copied` as it is. Every file is named `file:…`, so that no name is read as an option or a
/// protocol.
fn extraction_arguments(source: &Path, outputs: &[Output], copied: &[Copied]) -> Vec<String> {
    let mut arguments = common_arguments();
    arguments.extend(["-i".to_owned(), format!("file:{}", source.display())]);
    for output in outputs {
        arguments.extend(["-map".to_owned(), format!("0:{}", output.track.index)]);
        output_arguments(&mut arguments, output);
    }
    for copy in copied {
        // NUT keeps the timestamps as the source's reader shifted them, which the pages'
        // pass then reads as they are (`-copyts`): its cues fall where a direct decode's do.
        arguments.extend([
            "-map".to_owned(),
            format!("0:{}", copy.index),
            "-codec:0".to_owned(),
            "copy".to_owned(),
            "-f".to_owned(),
            "nut".to_owned(),
            format!("file:{}", copy.path.display()),
        ]);
    }
    arguments
}

/// The command decoding each teletext output from its stream's copy: an input per output,
/// since libzvbi's options (text rather than the pictures of the page, the page the subtitles
/// are on) belong to a decoder, and a decoder to an input's stream. A page shows until it
/// changes, so its subtitles come with no end: each is ended where the next begins, the page
/// cleared included.
fn teletext_arguments(outputs: &[(&Output, &Path)]) -> Vec<String> {
    let mut arguments = common_arguments();
    arguments.push("-copyts".to_owned());
    for (output, copy) in outputs {
        // A track scanned before its pages were known: every subtitle page.
        let page = output
            .track
            .teletext_page
            .map_or_else(|| "subtitle".to_owned(), |page| page.to_string());
        arguments.extend([
            "-txt_format:0".to_owned(),
            "text".to_owned(),
            "-txt_page:0".to_owned(),
            page,
            "-fix_sub_duration:0".to_owned(),
            "-i".to_owned(),
            format!("file:{}", copy.display()),
        ]);
    }
    for (input, (output, _)) in outputs.iter().enumerate() {
        arguments.extend(["-map".to_owned(), format!("{input}:0")]);
        output_arguments(&mut arguments, output);
    }
    arguments
}

/// Where an extraction stands.
#[derive(Debug, Clone)]
enum Extracted {
    Pending,
    /// On disk, at its place.
    Done(PathBuf),
    Failed,
}

/// The pass that extracts several tracks. Stopped when no extraction of it is left.
#[derive(Debug)]
struct Pass(AbortHandle);

impl Drop for Pass {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// One track's subtitle, found on disk or extracted by a task of its own: reading a file
/// over a network share can take longer than a player waits for a request, and a request given
/// up must not take the work done so far with it. Every request waits for the same result.
#[derive(Debug)]
pub(crate) struct Extraction {
    result: watch::Receiver<Extracted>,
    /// None for one found on disk.
    pass: Option<Arc<Pass>>,
}

impl Extraction {
    fn cached(path: PathBuf) -> Self {
        let (_, result) = watch::channel(Extracted::Done(path));
        Self { result, pass: None }
    }

    /// The subtitle, read from disk once extracted.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::SubtitleExtraction`] when the extraction failed or was stopped, or its
    /// file has since been pruned.
    pub(crate) async fn bytes(&self) -> Result<Vec<u8>, TranscodeError> {
        let mut result = self.result.clone();
        let outcome = result
            .wait_for(|extracted| !matches!(extracted, Extracted::Pending))
            .await
            .map_err(|_| TranscodeError::SubtitleExtraction)?
            .clone();
        let Extracted::Done(path) = outcome else {
            return Err(TranscodeError::SubtitleExtraction);
        };
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|_| TranscodeError::SubtitleExtraction)?;
        // What is read is what pruning keeps longest.
        let _ = tokio::task::spawn_blocking(move || touch(&path)).await;
        Ok(bytes)
    }

    fn running(&self) -> bool {
        matches!(&*self.result.borrow(), Extracted::Pending)
            && self.pass.as_ref().is_some_and(|pass| !pass.0.is_finished())
    }
}

fn touch(path: &Path) -> std::io::Result<()> {
    std::fs::File::options()
        .write(true)
        .open(path)?
        .set_modified(SystemTime::now())
}

/// The directory a file's subtitles are kept in, named after a digest of the file's path.
fn file_directory(root: &Path, source: &Path) -> PathBuf {
    let digest = Sha256::digest(source.as_os_str().as_encoded_bytes());
    let hex = digest[..16]
        .iter()
        .fold(String::with_capacity(32), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    root.join(format!("{SUBTITLE_DIRECTORY}{hex}"))
}

/// The file as it stood, size and modification time: a file changed is a new name, and the
/// old one ages out.
fn file_stamp(size: u64, modified: Option<SystemTime>) -> String {
    let modified = modified
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    format!("{size}-{modified}")
}

/// Where a track's subtitle is kept: in its file's directory, under a name that tells the
/// track (and the teletext page decoded), the format, and the file as it stood.
fn cache_path(
    root: &Path,
    source: &Path,
    size: u64,
    modified: Option<SystemTime>,
    track: &Track,
    format: SubtitleFormat,
) -> PathBuf {
    let page = track
        .teletext_page
        .map_or_else(String::new, |page| format!("p{page}-"));
    file_directory(root, source).join(format!(
        "{}-{page}{}.{}",
        track.index,
        file_stamp(size, modified),
        format.extension()
    ))
}

/// Where a teletext stream is kept as it is, beside its subtitles: every page in every format
/// is decoded from it without reading the file again, a page or a format asked later included.
fn copy_path(
    root: &Path,
    source: &Path,
    size: u64,
    modified: Option<SystemTime>,
    index: i32,
) -> PathBuf {
    file_directory(root, source).join(format!("{index}-raw-{}.nut", file_stamp(size, modified)))
}

/// Subtitles extracted, kept on disk across streams, users and restarts: a player changes track
/// by starting a new stream, and finds the one it switches to there. What is being extracted is
/// known in memory by the file it goes to, so that streams of the same file, whoever plays them,
/// wait for one pass rather than start another.
#[derive(Debug)]
pub(crate) struct Subtitles {
    slots: Arc<Semaphore>,
    root: PathBuf,
    running: Mutex<HashMap<PathBuf, Arc<Extraction>>>,
    pruned: Mutex<Option<Instant>>,
}

impl Subtitles {
    pub(crate) fn new(concurrent: usize, root: PathBuf) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(concurrent)),
            root,
            running: Mutex::default(),
            pruned: Mutex::default(),
        }
    }

    /// The extraction of each of `wanted`, in its order: those on disk read from it, those
    /// being extracted shared, the others started together in one pass over `source`.
    pub(crate) async fn prepare(
        &self,
        config: &crate::Config,
        source: &Path,
        wanted: &[(Track, SubtitleFormat)],
    ) -> Vec<Arc<Extraction>> {
        let (size, modified) = match tokio::fs::metadata(source).await {
            Ok(metadata) => (metadata.len(), metadata.modified().ok()),
            Err(_) => (0, None),
        };
        let mut located = Vec::with_capacity(wanted.len());
        for (track, format) in wanted {
            let path = cache_path(&self.root, source, size, modified, track, *format);
            let kept = tokio::fs::metadata(&path).await.is_ok();
            located.push((track, *format, path, kept));
        }

        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        running.retain(|_, extraction| extraction.running());
        let mut outputs: Vec<Output> = Vec::new();
        for (track, format, path, kept) in &located {
            let known = *kept
                || running.contains_key(path)
                || outputs.iter().any(|output| output.path == *path);
            if !known {
                outputs.push(Output {
                    track: (*track).clone(),
                    format: *format,
                    path: path.clone(),
                    copy: is_teletext(&track.codec)
                        .then(|| copy_path(&self.root, source, size, modified, track.index)),
                });
            }
        }
        if !outputs.is_empty() {
            let (senders, receivers): (Vec<_>, Vec<_>) = outputs
                .iter()
                .map(|_| watch::channel(Extracted::Pending))
                .unzip();
            let paths: Vec<PathBuf> = outputs.iter().map(|output| output.path.clone()).collect();
            let task = tokio::spawn(run_pass(
                config.clone(),
                Arc::clone(&self.slots),
                source.to_path_buf(),
                outputs,
                senders,
            ));
            let pass = Arc::new(Pass(task.abort_handle()));
            for (path, result) in paths.into_iter().zip(receivers) {
                let extraction = Extraction {
                    result,
                    pass: Some(Arc::clone(&pass)),
                };
                running.insert(path, Arc::new(extraction));
            }
        }
        located
            .into_iter()
            .map(|(_, _, path, kept)| match running.get(&path) {
                Some(extraction) if !kept => Arc::clone(extraction),
                _ => Arc::new(Extraction::cached(path)),
            })
            .collect()
    }

    /// Keeps the subtitles within their budget, at most every [`PRUNE_EVERY`].
    pub(crate) async fn prune(&self) {
        {
            let mut pruned = self.pruned.lock().unwrap_or_else(PoisonError::into_inner);
            if pruned.is_some_and(|at| at.elapsed() < PRUNE_EVERY) {
                return;
            }
            *pruned = Some(Instant::now());
        }
        let root = self.root.clone();
        let pruning =
            tokio::task::spawn_blocking(move || prune(&root, MAX_CACHE_BYTES, ABANDONED_AFTER));
        if let Err(error) = pruning.await {
            tracing::warn!(%error, "could not prune the subtitles");
        }
    }
}

/// Whether a directory below the transcoder's root holds a file's subtitles.
pub(crate) fn is_subtitle_directory(name: &str) -> bool {
    name.starts_with(SUBTITLE_DIRECTORY)
}

/// Across the files' directories: removes what passes left behind and the directories they
/// emptied, then the least recently read subtitles until the rest weigh three quarters of
/// `budget`, once past it. Nothing younger than `abandoned_after` but a subtitle is touched: a
/// pass may still be writing it.
fn prune(root: &Path, budget: u64, abandoned_after: Duration) {
    let Ok(directories) = std::fs::read_dir(root) else {
        return;
    };
    let old = |modified: SystemTime| modified.elapsed().is_ok_and(|age| age >= abandoned_after);
    let mut kept = Vec::new();
    for directory in directories.flatten() {
        if !is_subtitle_directory(&directory.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(directory.path()) else {
            continue;
        };
        let mut left = 0;
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let partial = entry.file_name().to_string_lossy().starts_with(PARTIAL);
            if partial && old(modified) && std::fs::remove_file(entry.path()).is_ok() {
                continue;
            }
            left += 1;
            if !partial {
                kept.push((modified, metadata.len(), entry.path()));
            }
        }
        let emptied = directory
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(old);
        if left == 0 && emptied {
            let _ = std::fs::remove_dir(directory.path());
        }
    }
    let mut weight: u64 = kept.iter().map(|(_, length, _)| length).sum();
    if weight <= budget {
        return;
    }
    kept.sort_unstable_by_key(|(modified, _, _)| *modified);
    for (_, length, path) in kept {
        if weight <= budget / 4 * 3 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            weight -= length;
        }
    }
}

/// Extracts `outputs` in one pass, each first into a file of its own beside its place, then
/// moves each to its place and tells who waits for it.
async fn run_pass(
    config: crate::Config,
    slots: Arc<Semaphore>,
    source: PathBuf,
    outputs: Vec<Output>,
    senders: Vec<watch::Sender<Extracted>>,
) {
    let pass = uuid::Uuid::new_v4().simple().to_string();
    let written: Vec<Output> = outputs
        .iter()
        .enumerate()
        .map(|(number, output)| Output {
            path: output.path.with_file_name(format!(
                "{PARTIAL}{pass}-{number}.{}",
                output.format.extension()
            )),
            ..output.clone()
        })
        .collect();
    let extracted = match slots.acquire_owned().await {
        Ok(_slot) => extract(&config, &source, &written).await,
        Err(_) => Err(TranscodeError::SubtitleExtraction),
    };
    if let Err(error) = &extracted {
        tracing::warn!(%error, tracks = outputs.len(), "could not extract subtitles");
    }
    for ((output, written), sender) in outputs.iter().zip(&written).zip(senders) {
        let outcome = match &extracted {
            Ok(()) if keep(written, &output.path).await => Extracted::Done(output.path.clone()),
            _ => Extracted::Failed,
        };
        let _ = tokio::fs::remove_file(&written.path).await;
        // Nobody left to read one is no failure.
        let _ = sender.send(outcome);
    }
}

/// Moves what a pass wrote to `path`, cleaned, unless it is missing or weighs more than a
/// subtitle of its format does. A rename, so that nobody reads half a file.
async fn keep(output: &Output, path: &Path) -> bool {
    let Ok(metadata) = tokio::fs::metadata(&output.path).await else {
        return false;
    };
    if metadata.len() > output.format.max_bytes() {
        return false;
    }
    if output.format == SubtitleFormat::WebVtt {
        let Ok(bytes) = tokio::fs::read(&output.path).await else {
            return false;
        };
        let cleaned = without_timestamp_map(bytes);
        if tokio::fs::write(&output.path, cleaned).await.is_err() {
            return false;
        }
    }
    tokio::fs::rename(&output.path, path).await.is_ok()
}

/// Runs the transcoder over the file once. A teletext track's stream is copied as it is in
/// that pass and kept, and its pages are decoded from the copy after it, all at once: a
/// broadcast's stream is interleaved with its video, so reading the file is what takes time,
/// and a stream carrying several pages, asked in several formats, is read once for all of
/// them. With its copy kept, the file is not read again.
///
/// # Errors
///
/// [`TranscodeError::Io`] when a directory cannot be made or a copy moved to its place,
/// [`TranscodeError::Spawn`] when the process cannot start,
/// [`TranscodeError::SubtitleExtraction`] when it fails.
async fn extract(
    config: &crate::Config,
    source: &Path,
    outputs: &[Output],
) -> Result<(), TranscodeError> {
    for output in outputs {
        if let Some(directory) = output.path.parent() {
            tokio::fs::create_dir_all(directory)
                .await
                .map_err(TranscodeError::Io)?;
        }
    }
    let (teletext, direct): (Vec<&Output>, Vec<&Output>) =
        outputs.iter().partition(|output| output.copy.is_some());
    // The copies missing, each written under a partial name until it is whole.
    let mut copied: Vec<(Copied, &Path)> = Vec::new();
    for output in &teletext {
        let Some(kept) = output.copy.as_deref() else {
            continue;
        };
        if copied.iter().any(|(_, place)| *place == kept) {
            continue;
        }
        // Read now, so that pruning, which removes the least recently read first, leaves it.
        let owned = kept.to_path_buf();
        let found = tokio::task::spawn_blocking(move || touch(&owned))
            .await
            .is_ok_and(|touched| touched.is_ok());
        if found {
            continue;
        }
        let pass = uuid::Uuid::new_v4().simple();
        let copy = Copied {
            index: output.track.index,
            path: kept.with_file_name(format!("{PARTIAL}{pass}.nut")),
        };
        copied.push((copy, kept));
    }
    let direct: Vec<Output> = direct.into_iter().cloned().collect();
    let mut ran = Ok(());
    if !direct.is_empty() || !copied.is_empty() {
        let written: Vec<Copied> = copied.iter().map(|(copy, _)| copy.clone()).collect();
        ran = run_transcoder(config, extraction_arguments(source, &direct, &written)).await;
    }
    for (copy, kept) in &copied {
        if ran.is_ok() {
            ran = tokio::fs::rename(&copy.path, kept)
                .await
                .map_err(TranscodeError::Io);
        }
        let _ = tokio::fs::remove_file(&copy.path).await;
    }
    if ran.is_ok() && !teletext.is_empty() {
        let decoded: Vec<(&Output, &Path)> = teletext
            .iter()
            .filter_map(|output| Some((*output, output.copy.as_deref()?)))
            .collect();
        ran = run_transcoder(config, teletext_arguments(&decoded)).await;
    }
    ran
}

/// Runs the transcoder with `arguments` to its end.
async fn run_transcoder(
    config: &crate::Config,
    arguments: Vec<String>,
) -> Result<(), TranscodeError> {
    let mut command = Command::new(&config.binary);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // A pass stopped stops the read of the file with it.
        .kill_on_drop(true);
    if let Some(codecs) = &config.external_libs {
        command.env(
            "FFMPEG_EXTERNAL_LIBS",
            crate::session::external_libs_value(codecs),
        );
    }
    let status = command
        .spawn()
        .map_err(TranscodeError::Spawn)?
        .wait()
        .await
        .map_err(TranscodeError::Io)?;
    if status.success() {
        Ok(())
    } else {
        Err(TranscodeError::SubtitleExtraction)
    }
}

/// The file without the `X-TIMESTAMP-MAP` header Plex's build writes, which ties cues to the
/// clock of HLS's MPEG-TS segments: a player honoring it beside a whole file shifts every cue by
/// the 10 seconds it states.
fn without_timestamp_map(bytes: Vec<u8>) -> Vec<u8> {
    const HEADER: &[u8] = b"\nX-TIMESTAMP-MAP=";
    let Some(start) = bytes
        .windows(HEADER.len())
        .position(|window| window == HEADER)
    else {
        return bytes;
    };
    // Only within the header block, which the first blank line ends.
    if bytes[..start].windows(2).any(|pair| pair == b"\n\n") {
        return bytes;
    }
    let end = bytes[start + 1..]
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or(bytes.len(), |at| start + 1 + at);
    [&bytes[..start], &bytes[end..]].concat()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn track(codec: &str) -> Track {
        Track {
            teletext_page: None,
            index: 3,
            codec: codec.to_owned(),
            language: Some("fr".to_owned()),
        }
    }

    fn output(track: Track, format: SubtitleFormat, path: &str) -> Output {
        Output {
            track,
            format,
            path: PathBuf::from(path),
            copy: None,
        }
    }

    fn failing() -> crate::Config {
        crate::Config {
            binary: PathBuf::from("no-such-transcoder-anywhere"),
            ..crate::Config::default()
        }
    }

    fn scratch() -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("klapz-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn set_age(path: &Path, seconds: u64) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn pictures_convert_into_ttml_alone_and_text_into_the_text_formats() {
        assert!(SubtitleFormat::Ttml.holds("hdmv_pgs_subtitle"));
        assert!(SubtitleFormat::Ttml.holds("dvd_subtitle"));
        assert!(!SubtitleFormat::Ttml.holds("subrip"));
        assert!(!SubtitleFormat::WebVtt.holds("pgs"));
        assert!(SubtitleFormat::Ass.holds("mov_text"));
        // Teletext's subtitles decode into text, never into pictures.
        assert!(SubtitleFormat::WebVtt.holds("dvb_teletext"));
        assert!(SubtitleFormat::Srt.holds("dvb_teletext"));
        assert!(!SubtitleFormat::Ttml.holds("dvb_teletext"));
        assert!(!is_text("dvb_teletext"));
    }

    #[test]
    fn copies_a_teletext_stream_once_and_decodes_each_page_from_the_copy() {
        let mut french = track("dvb_teletext");
        french.index = 5;
        french.teletext_page = Some(889);
        let mut german = french.clone();
        german.teletext_page = Some(150);
        let copy = Copied {
            index: 5,
            path: PathBuf::from("c.nut"),
        };
        let read = extraction_arguments(
            Path::new("f.ts"),
            &[output(track("subrip"), SubtitleFormat::WebVtt, "b")],
            std::slice::from_ref(&copy),
        )
        .join(" ");
        assert!(
            read.ends_with(concat!(
                "-i file:f.ts -map 0:3 -codec:0 webvtt -f webvtt file:b ",
                "-map 0:5 -codec:0 copy -f nut file:c.nut"
            )),
            "{read}"
        );
        assert!(!read.contains("-txt_"), "{read}");

        let (a, b) = (
            output(french, SubtitleFormat::Srt, "a"),
            output(german, SubtitleFormat::WebVtt, "g"),
        );
        let decoded =
            teletext_arguments(&[(&a, copy.path.as_path()), (&b, copy.path.as_path())]).join(" ");
        // Decoder options before the input they apply to: an input per page.
        assert!(
            decoded.ends_with(concat!(
                "-copyts ",
                "-txt_format:0 text -txt_page:0 889 -fix_sub_duration:0 -i file:c.nut ",
                "-txt_format:0 text -txt_page:0 150 -fix_sub_duration:0 -i file:c.nut ",
                "-map 0:0 -codec:0 subrip -f srt file:a -map 1:0 -codec:0 webvtt -f webvtt file:g"
            )),
            "{decoded}"
        );
        // A track scanned before its pages were known: every subtitle page.
        let mut any = a.clone();
        any.track.teletext_page = None;
        let decoded = teletext_arguments(&[(&any, copy.path.as_path())]).join(" ");
        assert!(
            decoded.contains("-txt_page:0 subtitle -fix_sub_duration:0 -i"),
            "{decoded}"
        );
    }

    #[test]
    fn a_teletext_page_has_a_subtitle_of_its_own() {
        let mut teletext = track("dvb_teletext");
        let place = |track: &Track| {
            cache_path(
                Path::new("/t"),
                Path::new("/m/a.ts"),
                10,
                None,
                track,
                SubtitleFormat::Srt,
            )
        };
        let any = place(&teletext);
        teletext.teletext_page = Some(888);
        let page = place(&teletext);
        assert_eq!(page.file_name().unwrap(), "3-p888-10-0.srt");
        assert_ne!(page, any);
    }

    #[tokio::test]
    async fn every_reader_gets_the_one_outcome_of_an_extraction() {
        let subtitles = Subtitles::new(1, scratch());
        let wanted = [(track("subrip"), SubtitleFormat::WebVtt)];
        let prepared = subtitles
            .prepare(&failing(), Path::new("film.mkv"), &wanted)
            .await;
        assert_eq!(prepared.len(), 1);
        let extraction = &prepared[0];
        let (first, second) = tokio::join!(extraction.bytes(), extraction.bytes());
        assert!(matches!(first, Err(TranscodeError::SubtitleExtraction)));
        assert!(matches!(second, Err(TranscodeError::SubtitleExtraction)));
        // Read again once over, it answers at once.
        assert!(extraction.bytes().await.is_err());
    }

    #[tokio::test]
    async fn one_pass_extracts_every_missing_track_and_every_stream_shares_it() {
        // No slot: the pass waits, still running, for the whole test.
        let subtitles = Subtitles::new(0, scratch());
        let mut other = track("ass");
        other.index = 4;
        let wanted = [
            (track("subrip"), SubtitleFormat::WebVtt),
            (other.clone(), SubtitleFormat::Ass),
            // Asked twice, extracted once.
            (track("subrip"), SubtitleFormat::WebVtt),
        ];
        let source = Path::new("film.mkv");
        let config = crate::Config::default();
        let first = subtitles.prepare(&config, source, &wanted).await;
        assert_eq!(first.len(), 3);
        assert!(Arc::ptr_eq(&first[0], &first[2]));
        assert!(first[0].pass.is_some());
        // Another user's stream of the same file, on the other track, waits for the same pass.
        let again = subtitles
            .prepare(&config, source, &[(other, SubtitleFormat::Ass)])
            .await;
        assert!(Arc::ptr_eq(&again[0], &first[1]));
    }

    #[tokio::test]
    async fn a_subtitle_on_disk_is_read_from_it_without_a_pass() {
        let root = scratch();
        let subtitles = Subtitles::new(0, root.clone());
        let source = Path::new("film.mkv");
        let path = cache_path(
            &root,
            source,
            0,
            None,
            &track("subrip"),
            SubtitleFormat::Srt,
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"1\n00:00:01,000 --> 00:00:02,000\nX\n").unwrap();
        let prepared = subtitles
            .prepare(
                &failing(),
                source,
                &[(track("subrip"), SubtitleFormat::Srt)],
            )
            .await;
        assert!(prepared[0].pass.is_none());
        assert!(prepared[0].bytes().await.unwrap().ends_with(b"X\n"));
    }

    #[test]
    fn a_file_keeps_its_subtitles_in_a_directory_of_its_own() {
        let place = |file: &str, size: u64, index: i32, format: SubtitleFormat| {
            let mut track = track("subrip");
            track.index = index;
            cache_path(Path::new("/t"), Path::new(file), size, None, &track, format)
        };
        let base = place("/m/a.mkv", 10, 3, SubtitleFormat::WebVtt);
        let directory = base.parent().unwrap();
        assert!(is_subtitle_directory(
            &directory.file_name().unwrap().to_string_lossy()
        ));
        assert_eq!(base.file_name().unwrap(), "3-10-0.vtt");
        assert_eq!(base, place("/m/a.mkv", 10, 3, SubtitleFormat::WebVtt));
        // The same file's other tracks and versions beside it; another file elsewhere.
        assert_eq!(
            place("/m/a.mkv", 11, 4, SubtitleFormat::Srt).parent(),
            Some(directory)
        );
        assert_ne!(place("/m/a.mkv", 11, 3, SubtitleFormat::WebVtt), base);
        assert_ne!(
            place("/m/b.mkv", 10, 3, SubtitleFormat::WebVtt).parent(),
            Some(directory)
        );
        // A teletext stream kept as it is, beside the subtitles decoded from it.
        let copy = copy_path(Path::new("/t"), Path::new("/m/a.mkv"), 10, None, 0);
        assert_eq!(copy.parent(), Some(directory));
        assert_eq!(copy.file_name().unwrap(), "0-raw-10-0.nut");
    }

    #[test]
    fn pruning_removes_abandoned_passes_and_the_least_recently_read_past_the_budget() {
        let root = scratch();
        let film = root.join(format!("{SUBTITLE_DIRECTORY}aa"));
        std::fs::create_dir(&film).unwrap();
        for (name, age) in [("3-1-1.vtt", 300), ("4-1-1.vtt", 200), ("5-1-1.vtt", 100)] {
            std::fs::write(film.join(name), [0u8; 100]).unwrap();
            set_age(&film.join(name), age);
        }
        let gone = film.join(format!("{PARTIAL}x-0.vtt"));
        std::fs::write(&gone, b"half").unwrap();
        set_age(&gone, 100);
        // A session's directory is none of pruning's business.
        let session = root.join("transcode-abc");
        std::fs::create_dir(&session).unwrap();
        std::fs::write(session.join("chunk"), [0u8; 1000]).unwrap();

        // 300 bytes for a budget of 250: down to 187, the oldest two go.
        prune(&root, 250, Duration::from_secs(50));
        let mut left: Vec<String> = std::fs::read_dir(&film)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["5-1-1.vtt"]);
        assert!(session.join("chunk").exists());

        // Within the budget, a pass still writing is left alone.
        let running = film.join(format!("{PARTIAL}y-0.vtt"));
        std::fs::write(&running, b"half").unwrap();
        prune(&root, 250, Duration::from_secs(50));
        assert!(running.exists());
    }

    #[test]
    fn drops_the_hls_timestamp_map_from_a_whole_webvtt_file() {
        // As Plex's build writes I Am Groot's French track.
        let written =
            b"WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:900000\n\n\n00:37.416 --> 00:38.625\nX\n";
        assert_eq!(
            without_timestamp_map(written.to_vec()),
            b"WEBVTT\n\n\n00:37.416 --> 00:38.625\nX\n"
        );
        // A cue's text is no header.
        let cue = b"WEBVTT\n\n00:01.000 --> 00:02.000\nX-TIMESTAMP-MAP=\n";
        assert_eq!(without_timestamp_map(cue.to_vec()), cue);
    }

    #[test]
    fn copies_a_track_already_in_the_format_asked() {
        let joined = extraction_arguments(
            Path::new("/m/film.mkv"),
            &[output(track("subrip"), SubtitleFormat::Srt, "/t/3.srt")],
            &[],
        )
        .join(" ");
        assert!(
            joined.ends_with("-i file:/m/film.mkv -map 0:3 -codec:0 copy -f srt file:/t/3.srt"),
            "{joined}"
        );
        let ass = extraction_arguments(
            Path::new("f"),
            &[output(track("ass"), SubtitleFormat::Ass, "a")],
            &[],
        )
        .join(" ");
        assert!(ass.contains("-codec:0 copy -f ass file:a"), "{ass}");
    }

    #[test]
    fn converts_tracks_into_other_formats_in_one_pass() {
        let mut ssa = track("ssa");
        ssa.index = 4;
        let joined = extraction_arguments(
            Path::new("f"),
            &[
                output(track("subrip"), SubtitleFormat::WebVtt, "a"),
                // SSA is not ASS to the muxer: it is converted.
                output(ssa, SubtitleFormat::Ass, "b"),
            ],
            &[],
        )
        .join(" ");
        assert!(
            joined.ends_with(
                "-i file:f -map 0:3 -codec:0 webvtt -f webvtt file:a -map 0:4 -codec:0 ass -f ass file:b"
            ),
            "{joined}"
        );
        let pgs = extraction_arguments(
            Path::new("f"),
            &[output(
                track("hdmv_pgs_subtitle"),
                SubtitleFormat::Ttml,
                "d",
            )],
            &[],
        )
        .join(" ");
        assert!(
            pgs.ends_with("-map 0:3 -codec:0 imsc_image -f imsc_image -embed_images 1 file:d"),
            "{pgs}"
        );
        let mov = extraction_arguments(
            Path::new("f"),
            &[output(track("mov_text"), SubtitleFormat::Srt, "c")],
            &[],
        )
        .join(" ");
        assert!(mov.contains("-codec:0 subrip -f srt file:c"), "{mov}");
    }
}
