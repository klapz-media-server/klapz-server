//! Which ffmpeg the server streams with and reads files with, beside the server's executable
//! in the packages.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Names tried beside the executable, in order: the patched build under the name the packages
/// give it, then under ffmpeg's own.
const NAMES: [&str; 2] = ["klapz-ffmpeg", "ffmpeg"];

/// `KLAPZ_TRANSCODER`; else `klapz-ffmpeg`, then `ffmpeg`, beside the running executable;
/// else the `ffmpeg` that `PATH` finds.
pub fn binary() -> PathBuf {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf));
    resolve(std::env::var_os("KLAPZ_TRANSCODER"), beside.as_deref())
}

fn resolve(configured: Option<OsString>, beside: Option<&Path>) -> PathBuf {
    if let Some(configured) = configured {
        return PathBuf::from(configured);
    }
    let executable = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
    beside
        .into_iter()
        .flat_map(|directory| NAMES.map(|name| directory.join(executable(name))))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(executable("ffmpeg")))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn directory(name: &str, programs: &[&str]) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("klapz-transcoder-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        for program in programs {
            let file = format!("{program}{}", std::env::consts::EXE_SUFFIX);
            std::fs::write(directory.join(file), b"").unwrap();
        }
        directory
    }

    fn named(directory: &Path, program: &str) -> PathBuf {
        directory.join(format!("{program}{}", std::env::consts::EXE_SUFFIX))
    }

    #[test]
    fn the_klapz_ffmpeg_beside_wins_over_ffmpeg() {
        let both = directory("both", &["klapz-ffmpeg", "ffmpeg"]);
        assert_eq!(resolve(None, Some(&both)), named(&both, "klapz-ffmpeg"));
        let ffmpeg = directory("ffmpeg", &["ffmpeg"]);
        assert_eq!(resolve(None, Some(&ffmpeg)), named(&ffmpeg, "ffmpeg"));
    }

    #[test]
    fn without_either_beside_it_is_the_ffmpeg_on_path() {
        let empty = directory("empty", &[]);
        let on_path = PathBuf::from(format!("ffmpeg{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(resolve(None, Some(&empty)), on_path);
        assert_eq!(resolve(None, None), on_path);
    }

    #[test]
    fn the_variable_wins_over_everything() {
        let both = directory("variable", &["klapz-ffmpeg", "ffmpeg"]);
        let configured = OsString::from("/opt/ffmpeg-plex/ffmpeg");
        assert_eq!(
            resolve(Some(configured.clone()), Some(&both)),
            PathBuf::from(configured)
        );
    }
}
