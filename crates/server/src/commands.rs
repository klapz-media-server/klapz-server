//! The command line's other jobs: creating a library and scanning one without serving.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use klapz_core::files::RelativePath;
use klapz_core::library::SectionKind;
use klapz_core::scan::{scan_section, ScanRequest, Scanner};
use klapz_core::{LibraryRepository, SectionId};
use klapz_db::repository::{create_library, NewLibrary};
use klapz_db::{DatabaseConnection, SqlLibraryRepository, SqlScanRepository};

use crate::analyzer::ScannerProcess;
use crate::files::LocalFiles;

const LIBRARY_USAGE: &str =
    "usage: klapz-server library add --kind movie|show --title TITLE [--language fr-FR] ROOT...";
const SCAN_USAGE: &str =
    "usage: klapz-server scan LIBRARY_ID [--force] [--root ROOT --below DIRECTORY]";

/// What a scan reads and writes with, for the server and the command line alike.
pub fn scanner(connection: &DatabaseConnection, library: Arc<dyn LibraryRepository>) -> Scanner {
    Scanner {
        library,
        files: Arc::new(LocalFiles),
        analyzer: Arc::new(ScannerProcess::from_env()),
        store: Arc::new(SqlScanRepository::new(connection.clone())),
    }
}

/// `library add`: the one way a library's roots are set, since no endpoint takes a path.
pub async fn library(
    connection: &DatabaseConnection,
    arguments: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let Some((action, rest)) = arguments.split_first() else {
        return Err(LIBRARY_USAGE.into());
    };
    if action != "add" {
        return Err(LIBRARY_USAGE.into());
    }
    let mut kind = None;
    let mut title = None;
    let mut language = "fr-FR".to_owned();
    let mut roots = Vec::new();
    let mut rest = rest.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--kind" => {
                kind = match rest.next().map(String::as_str) {
                    Some("movie") => Some(SectionKind::Movie),
                    Some("show") => Some(SectionKind::Show),
                    _ => return Err(LIBRARY_USAGE.into()),
                }
            }
            "--title" => title = rest.next().cloned(),
            "--language" => language = rest.next().cloned().ok_or(LIBRARY_USAGE)?,
            root => {
                // Stored canonical, as the scan and the media engine compare paths with it.
                let canonical =
                    std::fs::canonicalize(root).map_err(|error| format!("{root}: {error}"))?;
                if !canonical.is_dir() {
                    return Err(format!("{root} is not a directory").into());
                }
                roots.push(displayed(&canonical));
            }
        }
    }
    let (Some(kind), Some(title)) = (kind, title) else {
        return Err(LIBRARY_USAGE.into());
    };
    if roots.is_empty() {
        return Err(LIBRARY_USAGE.into());
    }
    let section = create_library(
        connection,
        NewLibrary {
            kind,
            title,
            language,
            roots,
        },
    )
    .await?;
    println!("library {section} created");
    Ok(())
}

/// `scan`: one scan to its end, in this process.
pub async fn scan(
    connection: &DatabaseConnection,
    arguments: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = arguments.iter();
    let section = arguments
        .next()
        .and_then(|id| id.parse::<i64>().ok())
        .map(SectionId::new)
        .ok_or(SCAN_USAGE)?;
    let mut force = false;
    let mut root = None;
    let mut below = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--force" => force = true,
            "--root" => root = Some(arguments.next().cloned().ok_or(SCAN_USAGE)?),
            "--below" => {
                below = Some(RelativePath::parse(arguments.next().ok_or(SCAN_USAGE)?)?);
            }
            _ => return Err(SCAN_USAGE.into()),
        }
    }
    let below = match (root, below) {
        (Some(root), Some(directory)) => Some((root, directory)),
        (None, None) => None,
        _ => return Err(SCAN_USAGE.into()),
    };
    let library: Arc<dyn LibraryRepository> =
        Arc::new(SqlLibraryRepository::new(connection.clone()));
    let scanner = scanner(connection, library);
    let request = ScanRequest {
        section,
        below,
        force,
    };
    let outcome = scan_section(&scanner, &request, &AtomicBool::new(false)).await?;
    println!(
        "added {}, updated {}, removed {}, unavailable roots {}",
        outcome.added, outcome.updated, outcome.removed, outcome.unavailable_roots
    );
    Ok(())
}

/// A canonical path as a person writes it. Windows canonicalizes `C:\Films` to the verbatim
/// `\\?\C:\Films`, and a mapped drive to its share, `\\?\UNC\nas\films`, which every API would
/// then publish as the files' location.
fn displayed(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{share}");
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.to_owned(),
        _ => text.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_the_verbatim_prefix_of_a_drive_or_a_share() {
        assert_eq!(
            displayed(std::path::Path::new(r"\\?\C:\Films")),
            r"C:\Films"
        );
        assert_eq!(
            displayed(std::path::Path::new(r"\\?\UNC\nas\films")),
            r"\\nas\films"
        );
        assert_eq!(
            displayed(std::path::Path::new(r"\\?\GLOBALROOT\Device\Films")),
            r"\\?\GLOBALROOT\Device\Films"
        );
        assert_eq!(displayed(std::path::Path::new("/srv/films")), "/srv/films");
    }
}
