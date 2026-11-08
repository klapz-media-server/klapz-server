//! Reading and re-encoding an image through `ffprobe` and `ffmpeg`.
//!
//! The bytes come from another machine, so no image decoder runs in the server's address
//! space, as for video. They travel over pipes, and the input is pinned to the image pipe
//! demuxer with only the `pipe` protocol allowed: left to probe, ffmpeg would also accept a
//! playlist naming other files or URLs. Every argument below is a number or a constant.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::PhotoError;

const PROCESS_TIMEOUT: Duration = Duration::from_secs(10);

/// A small file can declare a huge canvas; refused before anything decodes it.
const MAX_SOURCE_PIXELS: u64 = 64_000_000;

/// ffmpeg's `-q:v` for JPEG, close to the file sizes the reference produces by default.
const DEFAULT_JPEG_SCALE: u8 = 5;

const PINNED_INPUT: [&str; 6] = [
    "-f",
    "image2pipe",
    "-protocol_whitelist",
    "pipe",
    "-i",
    "pipe:0",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Encoding {
    Jpeg(Option<u8>),
    Png,
    Ppm,
}

#[derive(Debug)]
pub(super) struct SourceImage {
    pub width: u32,
    pub height: u32,
    pub is_png: bool,
    pub has_alpha: bool,
}

pub(super) async fn probe(bytes: &[u8]) -> Result<SourceImage, PhotoError> {
    let mut args = vec!["-v", "error"];
    args.extend(PINNED_INPUT);
    args.extend([
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=codec_name,width,height,pix_fmt",
        "-of",
        "csv=p=0",
    ]);
    let output = run("ffprobe", &args, bytes).await?;
    let image = parse_probe(&String::from_utf8_lossy(&output))?;
    if u64::from(image.width) * u64::from(image.height) > MAX_SOURCE_PIXELS {
        return Err(PhotoError::SourceTooLarge);
    }
    Ok(image)
}

fn parse_probe(output: &str) -> Result<SourceImage, PhotoError> {
    let mut fields = output.trim().split(',');
    let mut next = || fields.next().ok_or(PhotoError::UnreadableProbe);
    let codec = next()?;
    let width = next()?.parse().map_err(|_| PhotoError::UnreadableProbe)?;
    let height = next()?.parse().map_err(|_| PhotoError::UnreadableProbe)?;
    let pixel_format = next()?;
    Ok(SourceImage {
        width,
        height,
        is_png: codec == "png",
        // `pal8` is how a PNG with a transparent palette reads, and the reference keeps it
        // as PNG when asked for JPEG.
        has_alpha: matches!(
            pixel_format,
            "rgba" | "rgba64be" | "ya8" | "ya16be" | "pal8" | "yuva420p"
        ),
    })
}

pub(super) async fn encode(
    bytes: &[u8],
    size: (u32, u32),
    encoding: Encoding,
    has_alpha: bool,
) -> Result<Vec<u8>, PhotoError> {
    run("ffmpeg", &encode_args(size, encoding, has_alpha), bytes).await
}

fn encode_args(size: (u32, u32), encoding: Encoding, has_alpha: bool) -> Vec<String> {
    let mut args: Vec<String> = ["-v", "error", "-nostdin"].map(String::from).to_vec();
    args.extend(PINNED_INPUT.map(String::from));
    args.extend(["-an", "-frames:v", "1", "-vf"].map(String::from));
    args.push(format!("scale={}:{}:flags=lanczos", size.0, size.1));

    let scale;
    let codec: &[&str] = match encoding {
        Encoding::Jpeg(quality) => {
            scale = quality.map_or(DEFAULT_JPEG_SCALE, jpeg_scale).to_string();
            // 4:4:4, as the reference encodes, whatever the source's subsampling.
            &["-c:v", "mjpeg", "-pix_fmt", "yuvj444p", "-q:v", &scale]
        }
        Encoding::Png if has_alpha => &["-c:v", "png", "-pix_fmt", "rgba"],
        Encoding::Png => &["-c:v", "png"],
        Encoding::Ppm => &["-c:v", "ppm", "-pix_fmt", "rgb24"],
    };
    args.extend(codec.iter().map(ToString::to_string));
    args.extend(["-f", "image2pipe", "pipe:1"].map(String::from));
    args
}

/// From the reference's 1–100 (anything above is 100) to ffmpeg's 31–2, where lower is better.
fn jpeg_scale(quality: u8) -> u8 {
    let steps = (u16::from(quality.clamp(1, 100)) - 1) * 29 / 99;
    31 - u8::try_from(steps).unwrap_or(29)
}

async fn run<S: AsRef<std::ffi::OsStr>>(
    program: &'static str,
    args: &[S],
    input: &[u8],
) -> Result<Vec<u8>, PhotoError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| PhotoError::Spawn(program, error))?;
    let stdin = child.stdin.take();

    // Written while the output is read: a child blocked on a full stdout stops reading its
    // stdin. A failed write only means the child quit early, which its status reports.
    let feed = async move {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(input).await;
        }
    };
    let finished = async { tokio::join!(child.wait_with_output(), feed) };
    let (output, ()) = tokio::time::timeout(PROCESS_TIMEOUT, finished)
        .await
        .map_err(|_| PhotoError::TimedOut(program))?;

    let output = output.map_err(|error| PhotoError::Spawn(program, error))?;
    if output.status.success() && !output.stdout.is_empty() {
        Ok(output.stdout)
    } else {
        Err(PhotoError::Failed(program))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn reads_what_ffprobe_prints() {
        // Real output, on the avatar and on Plex Web's own images.
        let avatar = parse_probe("png,256,256,gray\r\n").unwrap();
        assert!(avatar.is_png && !avatar.has_alpha);
        assert_eq!((avatar.width, avatar.height), (256, 256));

        let jpeg = parse_probe("mjpeg,1708,640,yuvj444p\n").unwrap();
        assert!(!jpeg.is_png && !jpeg.has_alpha);

        assert!(parse_probe("png,1521,730,pal8\n").unwrap().has_alpha);
        assert!(parse_probe("png,332,332,rgba\n").unwrap().has_alpha);
    }

    #[test]
    fn refuses_output_that_is_not_an_image_description() {
        assert!(parse_probe("").is_err());
        assert!(parse_probe("png,abc,256,gray").is_err());
    }

    #[test]
    fn jpeg_quality_runs_from_worst_to_best() {
        assert_eq!(jpeg_scale(1), 31);
        assert_eq!(jpeg_scale(100), 2);
        assert_eq!(jpeg_scale(127), 2);
        assert!(jpeg_scale(10) > jpeg_scale(90));
    }

    #[test]
    fn the_input_is_pinned_to_pipes() {
        let args = encode_args((60, 60), Encoding::Png, false).join(" ");
        assert!(args.contains("-f image2pipe -protocol_whitelist pipe -i pipe:0"));
        assert!(args.contains("scale=60:60"));
        assert!(args.ends_with("pipe:1"));
    }
}
