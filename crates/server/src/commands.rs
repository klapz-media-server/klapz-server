//! The command line's other jobs: creating a library, scanning one and matching it against its
//! metadata provider, without serving.

use std::path::Path;
use std::sync::Arc;

use klapz_core::files::LibraryFiles;
use klapz_core::files::RelativePath;
use klapz_core::language::LanguageTag;
use klapz_core::library::{checked_roots, normalized_title, RootError, SectionKind};
use klapz_core::scan::{
    scan_section, AnalysisTools, BackgroundAnalysis, ScanRepository, ScanRequest, Scanner,
};
use klapz_core::source_metadata::{match_library, MetadataSources, SourceMetadataError};
use klapz_core::{LibraryRepository, SectionId};
use klapz_db::repository::{create_library, NewLibrary};
use klapz_db::{
    DatabaseConnection, SqlLibraryRepository, SqlScanRepository, SqlSettingsRepository,
    SqlSourceMetadataRepository,
};

use crate::analyzer::MediaReader;
use crate::extracts::Extracts;
use crate::files::LocalFiles;

const LIBRARY_USAGE: &str =
    "usage: klapz-server library add --kind movie|show --title TITLE [--language fr-FR] ROOT...";
const SCAN_USAGE: &str =
    "usage: klapz-server scan LIBRARY_ID [--force] [--root ROOT --below DIRECTORY]";
const MATCH_USAGE: &str = "usage: klapz-server match LIBRARY_ID [--force]";

/// TMDB for films and TVDB for series, each when it has a key.
pub fn metadata_sources(
    credentials: klapz_agents::ProviderCredentials,
) -> Result<MetadataSources, klapz_core::source_metadata::AgentError> {
    Ok(MetadataSources {
        films: match credentials.tmdb {
            Some(tmdb) => Some(Arc::new(klapz_agents::Tmdb::new(tmdb)?)),
            None => None,
        },
        series: match credentials.tvdb {
            Some(tvdb) => Some(Arc::new(klapz_agents::Tvdb::new(tvdb)?)),
            None => None,
        },
    })
}

/// The artwork store over `images`, as matching keeps images through it.
pub fn artwork_fetcher(images: &Path) -> crate::artwork::StoreFetcher {
    crate::artwork::StoreFetcher(Arc::new(klapz_transcoder::artwork::ArtworkStore::new(
        images.to_path_buf(),
    )))
}

/// Series' themes found in this machine's libraries and kept in the artwork store.
fn themes(fetcher: &crate::artwork::StoreFetcher) -> klapz_core::source_metadata::theme::Themes {
    klapz_core::source_metadata::theme::Themes {
        store: Arc::new(fetcher.clone()),
        files: Arc::new(LocalFiles),
    }
}

/// `match`: a library's films or series matched against their provider, in this process.
/// Without `--force`, only the items no record describes yet.
pub async fn match_command(
    connection: &DatabaseConnection,
    arguments: &[String],
    images: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let (section, force) = match arguments {
        [id] => (id, false),
        [id, flag] if flag == "--force" => (id, true),
        _ => return Err(MATCH_USAGE.into()),
    };
    let section = SectionId::new(section.parse().map_err(|_| MATCH_USAGE)?);
    let sources = metadata_sources(klapz_agents::ProviderCredentials::from_env())?;
    let repository = SqlSourceMetadataRepository::new(connection.clone());
    let fetcher = artwork_fetcher(images);
    let themes = themes(&fetcher);
    let report = match_library(
        &repository,
        &sources,
        Some(&fetcher),
        Some(&themes),
        section,
        force,
        klapz_core::now_epoch(),
    )
    .await?;
    log_report(&report);
    Ok(())
}

/// What scans and the analysis of what they find need, for the server and the command line
/// alike. One store for both, so that their writes take turns; what the analysis extracts is
/// kept below `extracts`, whose unfinished work is removed first.
pub async fn scanning(
    connection: &DatabaseConnection,
    library: Arc<dyn LibraryRepository>,
    extracts: &Path,
) -> (Scanner, BackgroundAnalysis) {
    let store: Arc<dyn ScanRepository> = Arc::new(SqlScanRepository::new(connection.clone()));
    let files: Arc<dyn LibraryFiles> = Arc::new(LocalFiles);
    let scanner = Scanner {
        library: Arc::clone(&library),
        files: Arc::clone(&files),
        store: Arc::clone(&store),
    };
    let extractor = Extracts::from_env(extracts.to_path_buf());
    extractor.remove_leftovers().await;
    let analysis = BackgroundAnalysis::new(
        store,
        library,
        files,
        AnalysisTools {
            analyzer: Arc::new(MediaReader::from_env()),
            extractor: Arc::new(extractor),
            settings: Arc::new(SqlSettingsRepository::new(connection.clone())),
        },
    );
    (scanner, analysis)
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
    let language = LanguageTag::parse(&language)
        .ok_or_else(|| format!("{language} is not a language tag such as fr-FR"))?;
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
    images: &Path,
    extracts: &Path,
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
    let (scanner, analysis) = scanning(connection, library, extracts).await;
    let request = ScanRequest {
        section,
        below,
        force,
    };
    let outcome = scan_section(&scanner, &request).await?;
    println!(
        "added {}, updated {}, removed {}, unavailable roots {}, queued {}",
        outcome.added, outcome.updated, outcome.removed, outcome.unavailable_roots, outcome.queued
    );
    // A server reads what a scan queued in the background; the command waits for it, so that
    // the library it leaves is read, what an earlier run left queued included.
    analysis.analyze_library(section).await?;
    let read = analysis.progress(section).await?;
    println!(
        "files {}, read {}, complete {}",
        read.files, read.read, read.complete
    );
    // As the server's scans do: what was added is matched before the command ends.
    let sources = metadata_sources(klapz_agents::ProviderCredentials::from_env())?;
    let repository = SqlSourceMetadataRepository::new(connection.clone());
    let fetcher = artwork_fetcher(images);
    let themes = themes(&fetcher);
    match match_library(
        &repository,
        &sources,
        Some(&fetcher),
        Some(&themes),
        section,
        false,
        klapz_core::now_epoch(),
    )
    .await
    {
        Ok(report) => log_report(&report),
        // A library of music or photos, or one no provider has a key for: nothing to match.
        Err(SourceMetadataError::UnsupportedKind | SourceMetadataError::NoSource) => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn log_report(report: &klapz_core::source_metadata::MatchReport) {
    for matched in &report.matched {
        tracing::info!(
            item = matched.item.get(),
            searched = matched.searched,
            title = matched.title.as_deref().unwrap_or_default(),
            provider_id = matched.provider_id,
            score = ?matched.score,
            children = matched.children,
            "matched"
        );
    }
    for unmatched in &report.unmatched {
        let (best, score) = unmatched
            .best
            .as_ref()
            .map_or(("", 0), |(name, score)| (name.as_str(), *score));
        tracing::warn!(
            item = unmatched.item.get(),
            searched = unmatched.searched,
            best,
            score,
            "no result scored high enough"
        );
    }
    for (item, reason) in &report.failed {
        tracing::error!(item = item.get(), %reason, "matching failed");
    }
    tracing::info!(
        matched = report.matched.len(),
        unmatched = report.unmatched.len(),
        failed = report.failed.len(),
        "matching complete"
    );
}
