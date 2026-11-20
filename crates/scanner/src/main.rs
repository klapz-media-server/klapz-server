//! `klapz-scanner <command> [options] -- <file>`: one file, one JSON report on standard
//! output. Errors go to standard error, and the exit status says whose fault they are.

use std::path::PathBuf;
use std::process::ExitCode;

use klapz_scanner::report::{Output, Report, VERSION};
use klapz_scanner::{ScanError, Tools};

const USAGE: &str = "\
usage: klapz-scanner <command> [options] -- <file>

commands:
  probe [--deep]                       container, tracks and chapters; --deep measures every packet
  hashes                               size, modification time, OpenSubtitles hash
  loudness [--stream N]                EBU R 128 of an audio track, the first by default
  fingerprint [--stream N] [--length S] Chromaprint of the start of an audio track
  subtitles --stream N --out DIR       an embedded subtitle track as a file
  bif [--interval S] --out DIR         preview thumbnails every S seconds, 2 by default
  chapter-thumbs --out DIR             a frame per chapter
  artwork --out DIR                    frames for a poster and a backdrop, and embedded covers

The report is JSON on standard output. Exit status: 0 done, 2 bad usage, 3 the media is
unreadable or lacks what was asked, 1 anything else.";

/// The media is at fault: corrupt, or without the track asked for. The caller records it
/// against the file rather than retrying.
const EXIT_MEDIA: u8 = 3;
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let invocation = match Invocation::parse(&arguments) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("{message}\n\n{USAGE}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    match invocation.run(&Tools::from_env()) {
        Ok(output) => {
            let report = Report {
                version: VERSION,
                output,
            };
            match serde_json::to_string(&report) {
                Ok(json) => {
                    println!("{json}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("cannot write the report: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(error) => {
            eprintln!("{error}");
            if error.is_media_fault() {
                ExitCode::from(EXIT_MEDIA)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

#[derive(Debug, PartialEq)]
struct Invocation {
    command: String,
    file: PathBuf,
    deep: bool,
    stream: Option<u32>,
    length_s: Option<u64>,
    interval_s: u32,
    out: Option<PathBuf>,
}

impl Invocation {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let (command, rest) = arguments.split_first().ok_or("no command")?;
        let mut invocation = Self {
            command: command.clone(),
            file: PathBuf::new(),
            deep: false,
            stream: None,
            length_s: None,
            interval_s: 2,
            out: None,
        };
        let mut rest = rest.iter();
        let mut file = None;
        while let Some(argument) = rest.next() {
            let mut value = |name: &str| {
                rest.next()
                    .cloned()
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            match argument.as_str() {
                "--deep" => invocation.deep = true,
                "--stream" => invocation.stream = Some(number(&value("--stream")?)?),
                "--length" => invocation.length_s = Some(number(&value("--length")?)?),
                "--interval" => {
                    invocation.interval_s = number(&value("--interval")?)?;
                    if invocation.interval_s == 0 {
                        return Err("--interval must be at least 1".to_owned());
                    }
                }
                "--out" => invocation.out = Some(PathBuf::from(value("--out")?)),
                // Everything after `--` is the file, so a name starting with `-` is never an
                // option.
                "--" => {
                    file = rest.next().cloned();
                    if rest.next().is_some() {
                        return Err("one file at a time".to_owned());
                    }
                    break;
                }
                other => return Err(format!("unknown option {other}; the file goes after --")),
            }
        }
        invocation.file = PathBuf::from(file.ok_or("no file: give it after --")?);
        Ok(invocation)
    }

    fn out(&self) -> Result<&PathBuf, ScanError> {
        self.out
            .as_ref()
            .ok_or_else(|| ScanError::Unsupported(format!("{} needs --out", self.command)))
    }

    fn run(&self, tools: &Tools) -> Result<Output, ScanError> {
        let file = &self.file;
        Ok(match self.command.as_str() {
            "probe" => Output::Probe(klapz_scanner::probe(tools, file, self.deep)?),
            "hashes" => Output::Hashes(klapz_scanner::identify(file)?),
            "loudness" => Output::Loudness(klapz_scanner::loudness(tools, file, self.stream)?),
            "fingerprint" => Output::Fingerprint(klapz_scanner::fingerprint(
                tools,
                file,
                self.stream,
                self.length_s.map(|seconds| seconds.saturating_mul(1000)),
            )?),
            "subtitles" => {
                let track = self
                    .stream
                    .ok_or_else(|| ScanError::Unsupported("subtitles needs --stream".to_owned()))?;
                Output::Subtitles(klapz_scanner::extract_subtitle(
                    tools,
                    file,
                    track,
                    self.out()?,
                )?)
            }
            "bif" => Output::Bif(klapz_scanner::preview_index(
                tools,
                file,
                self.interval_s,
                self.out()?,
            )?),
            "chapter-thumbs" => {
                Output::ChapterThumbs(klapz_scanner::chapter_thumbs(tools, file, self.out()?)?)
            }
            "artwork" => Output::Artwork(klapz_scanner::artwork(tools, file, self.out()?)?),
            other => return Err(ScanError::Unsupported(format!("unknown command {other}"))),
        })
    }
}

fn number<T: std::str::FromStr>(value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{value} is not a whole number"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn parse(arguments: &[&str]) -> Result<Invocation, String> {
        let arguments: Vec<String> = arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect();
        Invocation::parse(&arguments)
    }

    #[test]
    fn the_file_comes_after_the_separator() {
        let invocation = parse(&[
            "bif",
            "--interval",
            "5",
            "--out",
            "tmp",
            "--",
            "-a film.mkv",
        ])
        .unwrap();
        assert_eq!(invocation.file, PathBuf::from("-a film.mkv"));
        assert_eq!(invocation.interval_s, 5);
        assert_eq!(invocation.out, Some(PathBuf::from("tmp")));
    }

    #[test]
    fn refuses_a_file_without_the_separator() {
        assert!(parse(&["probe", "film.mkv"]).is_err());
        assert!(parse(&["probe", "--"]).is_err());
        assert!(parse(&["probe", "--", "a.mkv", "b.mkv"]).is_err());
        assert!(parse(&["bif", "--interval", "0", "--", "a.mkv"]).is_err());
    }
}
