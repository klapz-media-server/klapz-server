//! The command line's other jobs: creating a library and scanning one without serving.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use klapz_core::files::RelativePath;
use klapz_core::library::{
    checked_roots, is_language_tag, normalized_title, RootError, SectionKind,
};
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

/// `library add`: a library over directories of this machine, without serving.
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
            root => roots.push(root.to_owned()),
        }
    }
    let (Some(kind), Some(title)) = (kind, title) else {
        return Err(LIBRARY_USAGE.into());
    };
    let title = normalized_title(&title).ok_or("the title is empty or too long")?;
    if !is_language_tag(&language) {
        return Err(format!("{language} is not a language tag such as fr-FR").into());
    }
    // The rules the API applies to an owner's roots, so both ways in store the same paths.
    let roots = checked_roots(&LocalFiles, &roots)
        .await
        .map_err(|error| match error {
            RootError::NotADirectory(index) | RootError::Overlapping(index) => {
                format!("{}: {error}", roots[index])
            }
            other => other.to_string(),
        })?;
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
