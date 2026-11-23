//! The server's directories, for an owner choosing the ones a library scans without typing
//! their paths.
//!
//! The second place a path comes in, by the repo owner's decision, beside a library's roots:
//! owners only, and it names directories and nothing else — no file, no content.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::{Context, Enum, Object, Result, SimpleObject};
use klapz_core::files::{Directory, FilesError, MAX_LISTED_DIRECTORIES};
use klapz_core::page::{Cursor, MAX_ITEMS};

use crate::cursor::OpaqueCursor;
use crate::schema::ApiContext;

const DEFAULT_PAGE_SIZE: usize = 200;

/// Why a directory was not listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum DirectoryErrorCode {
    /// Nothing is at this path on the server.
    NotFound,
    /// Not an absolute path, or not one to a directory.
    NotADirectory,
    /// The server is not allowed to read this directory.
    Unreadable,
}

/// A directory of the server.
#[derive(Debug, Clone, SimpleObject)]
pub struct DirectoryEntry {
    /// Its own name; for a drive, its letter and colon, such as `D:`.
    pub name: String,
    /// Its full path, in the form a library stores a root in: what `createLibrary` takes.
    pub path: String,
}

/// The directories inside one, or where browsing starts.
#[derive(Debug)]
pub struct DirectoryListing {
    path: Option<String>,
    parent: Option<String>,
    /// By name, ignoring case.
    directories: Vec<Directory>,
    truncated: bool,
}

/// The directories inside one, or where browsing starts.
#[Object]
impl DirectoryListing {
    /// The directory listed, resolved as a library root is: absolute, links followed, in
    /// canonical form. Null where browsing starts.
    async fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// The directory holding it. Null at the top of a drive or of the filesystem, whose parent
    /// is where browsing starts, and where browsing starts.
    async fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    /// Whether the directory holds more directories than the server reads in one listing
    /// (10,000): the others are not in `directories`. Typing the path of one still reaches it.
    async fn truncated(&self) -> bool {
        self.truncated
    }

    /// The directories inside, by name ignoring case. Files and hidden directories are left
    /// out. Pages forward only, by position.
    async fn directories(
        &self,
        #[graphql(desc = "Return directories after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return at most this many directories; 200 when absent, 500 at most.")]
        first: Option<i32>,
    ) -> Result<Connection<OpaqueCursor, DirectoryEntry, EmptyFields, EmptyFields>> {
        query(
            after,
            None,
            first,
            None,
            |after: Option<OpaqueCursor>, _: Option<OpaqueCursor>, first, _| async move {
                let start = after.map_or(0, |cursor| {
                    usize::try_from(cursor.0.id).map_or(0, |position| position + 1)
                });
                let count = first.unwrap_or(DEFAULT_PAGE_SIZE);
                if count > MAX_ITEMS as usize {
                    return Err(async_graphql::Error::new(format!(
                        "a page of more than {MAX_ITEMS} is not served"
                    )));
                }
                let mut connection =
                    Connection::new(start > 0, self.directories.len() > start + count);
                connection.edges.extend(
                    self.directories
                        .iter()
                        .enumerate()
                        .skip(start)
                        .take(count)
                        .map(|(position, directory)| {
                            Edge::new(
                                OpaqueCursor(Cursor::by_id(
                                    i64::try_from(position).unwrap_or(i64::MAX),
                                )),
                                DirectoryEntry {
                                    name: directory.name.clone(),
                                    path: directory.path.clone(),
                                },
                            )
                        }),
                );
                Ok::<_, async_graphql::Error>(connection)
            },
        )
        .await
    }
}

/// A directory listed, or why it was not.
#[derive(Debug, SimpleObject)]
pub struct ServerDirectories {
    /// The listing. Null when `error` says why there is none.
    pub listing: Option<DirectoryListing>,
    /// Why the directory was not listed. Null on success.
    pub error: Option<DirectoryErrorCode>,
}

pub(crate) async fn server_directories(
    context: &Context<'_>,
    path: Option<String>,
) -> Result<ServerDirectories> {
    crate::access::require_owner(context)?;
    let files = context.data::<ApiContext>()?.scans.files();
    let Some(path) = path else {
        let starts = files.starting_points().await?;
        return Ok(ServerDirectories {
            listing: Some(DirectoryListing {
                path: None,
                parent: None,
                directories: starts,
                truncated: false,
            }),
            error: None,
        });
    };
    let error = match files
        .list_directories(path.trim(), MAX_LISTED_DIRECTORIES)
        .await
    {
        Ok(listing) => {
            return Ok(ServerDirectories {
                listing: Some(DirectoryListing {
                    path: Some(listing.path),
                    parent: listing.parent,
                    directories: listing.directories,
                    truncated: listing.truncated,
                }),
                error: None,
            })
        }
        Err(FilesError::NotFound | FilesError::Unavailable) => DirectoryErrorCode::NotFound,
        Err(FilesError::Refused) => DirectoryErrorCode::NotADirectory,
        Err(FilesError::Unreadable) => DirectoryErrorCode::Unreadable,
        Err(error) => return Err(error.into()),
    };
    Ok(ServerDirectories {
        listing: None,
        error: Some(error),
    })
}
