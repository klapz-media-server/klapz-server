//! Creating, changing and deleting a library.
//!
//! The server's owners create and delete libraries and choose the directories they scan: a
//! root is a path, and naming the server's own directories is theirs alone. A library's admins
//! rename it and change its language, as they change what it holds.

use async_graphql::{Context, Enum, InputObject, Result, SimpleObject, ID};
use klapz_core::library::{
    checked_roots, create_library, delete_library, edit_library, is_language_tag, normalized_title,
    Deletion, Edited, LibraryEdit, NewLibrary, RootError, SectionKind,
};
use klapz_core::{now_epoch, SectionId};

use crate::library::{Library, LibraryKind, LIBRARY_TYPE};
use crate::node::GlobalId;
use crate::schema::ApiContext;

/// Why a library mutation did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum LibraryErrorCode {
    /// No library the caller may change carries this id.
    LibraryNotFound,
    /// A title is 1 to 255 characters once trimmed, with no control character.
    InvalidTitle,
    /// Not shaped as a BCP 47 language tag, such as `fr-FR`.
    InvalidLanguage,
    /// Music and photo libraries are not held by this server.
    UnsupportedKind,
    /// A library needs a directory to scan.
    NoRoot,
    /// A library scans at most 32 directories.
    TooManyRoots,
    /// Not an absolute path to a directory the server reaches.
    RootNotFound,
    /// Lies inside another of the roots given, or holds one: its files would be found twice.
    OverlappingRoots,
    /// Only the server's owners choose the directories a library scans.
    OwnerOnly,
    /// The library is being scanned: its roots stay as they are and it stays until the scan
    /// ends or is cancelled.
    Scanning,
}

/// A request a library mutation refused, and where in the input the reason lies.
#[derive(Debug, Clone, SimpleObject)]
pub struct LibraryUserError {
    /// The path to the input field at fault; for a root, its position in `roots` too.
    pub field: Vec<String>,
    /// What went wrong, for a person to read.
    pub message: String,
    /// What went wrong, for a program to branch on.
    pub code: LibraryErrorCode,
}

impl LibraryUserError {
    fn new(field: &str, code: LibraryErrorCode) -> Self {
        let message = match code {
            LibraryErrorCode::LibraryNotFound => "no such library",
            LibraryErrorCode::InvalidTitle => "a title is 1 to 255 characters, none a control",
            LibraryErrorCode::InvalidLanguage => "not a language tag such as fr-FR",
            LibraryErrorCode::UnsupportedKind => "no library here holds this kind",
            LibraryErrorCode::NoRoot => "a library needs a directory to scan",
            LibraryErrorCode::TooManyRoots => "a library scans at most 32 directories",
            LibraryErrorCode::RootNotFound => "not a directory this server reaches",
            LibraryErrorCode::OverlappingRoots => {
                "this directory lies inside another, or holds one"
            }
            LibraryErrorCode::OwnerOnly => "only an owner chooses a library's directories",
            LibraryErrorCode::Scanning => "this library is being scanned",
        };
        Self {
            field: vec!["input".to_owned(), field.to_owned()],
            message: message.to_owned(),
            code,
        }
    }

    fn at_root(index: usize, code: LibraryErrorCode) -> Self {
        let mut error = Self::new("roots", code);
        error.field.push(index.to_string());
        error
    }
}

type Refusal<T> = std::result::Result<T, LibraryUserError>;

fn api<'c>(context: &Context<'c>) -> Result<&'c ApiContext> {
    context.data::<ApiContext>()
}

fn section_id(id: &ID) -> Option<SectionId> {
    GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == LIBRARY_TYPE)
        .map(|decoded| SectionId::new(decoded.key))
}

fn title(title: &str) -> Refusal<String> {
    normalized_title(title)
        .ok_or_else(|| LibraryUserError::new("title", LibraryErrorCode::InvalidTitle))
}

fn language(language: &str) -> Refusal<String> {
    let language = language.trim();
    if is_language_tag(language) {
        Ok(language.to_owned())
    } else {
        Err(LibraryUserError::new(
            "language",
            LibraryErrorCode::InvalidLanguage,
        ))
    }
}

async fn roots(context: &Context<'_>, paths: &[String]) -> Result<Refusal<Vec<String>>> {
    let refusal = match checked_roots(api(context)?.scans.files(), paths).await {
        Ok(roots) => return Ok(Ok(roots)),
        Err(RootError::NoRoot) => LibraryUserError::new("roots", LibraryErrorCode::NoRoot),
        Err(RootError::TooMany) => LibraryUserError::new("roots", LibraryErrorCode::TooManyRoots),
        Err(RootError::NotADirectory(index)) => {
            LibraryUserError::at_root(index, LibraryErrorCode::RootNotFound)
        }
        Err(RootError::Overlapping(index)) => {
            LibraryUserError::at_root(index, LibraryErrorCode::OverlappingRoots)
        }
        Err(error @ RootError::Files(_)) => return Err(error.into()),
    };
    Ok(Err(refusal))
}

/// A library after a change.
#[derive(Debug, SimpleObject)]
pub struct LibraryPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The library as it now stands. Null when the request was refused.
    pub library: Option<Library>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<LibraryUserError>,
}

impl LibraryPayload {
    fn of(client_mutation_id: Option<String>, outcome: Refusal<Library>) -> Self {
        match outcome {
            Ok(library) => Self {
                client_mutation_id,
                library: Some(library),
                user_errors: Vec::new(),
            },
            Err(error) => Self {
                client_mutation_id,
                library: None,
                user_errors: vec![error],
            },
        }
    }
}

/// A library to create.
#[derive(Debug, InputObject)]
pub struct CreateLibraryInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// Its name: 1 to 255 characters, trimmed.
    pub title: String,
    /// What it holds: films or series.
    pub kind: LibraryKind,
    /// The language its metadata is in, as a BCP 47 tag such as `fr-FR`.
    pub language: String,
    /// The directories it scans: absolute paths on the server, 1 to 32 of them, none inside
    /// another. Each is stored resolved, links followed; a repeat is kept once.
    pub roots: Vec<String>,
}

pub(crate) async fn create(
    context: &Context<'_>,
    input: CreateLibraryInput,
) -> Result<LibraryPayload> {
    crate::access::require_owner(context)?;
    let outcome = async {
        let kind = SectionKind::from(input.kind);
        if !matches!(kind, SectionKind::Movie | SectionKind::Show) {
            return Ok(Err(LibraryUserError::new(
                "kind",
                LibraryErrorCode::UnsupportedKind,
            )));
        }
        let (title, language) = match (title(&input.title), language(&input.language)) {
            (Ok(title), Ok(language)) => (title, language),
            (Err(error), _) | (_, Err(error)) => return Ok(Err(error)),
        };
        let roots = match roots(context, &input.roots).await? {
            Ok(roots) => roots,
            Err(error) => return Ok(Err(error)),
        };
        let api = api(context)?;
        let created = create_library(
            api.library.as_ref(),
            api.scans.as_ref(),
            NewLibrary {
                kind,
                title,
                language,
                roots,
            },
            now_epoch(),
        )
        .await?;
        Ok::<_, async_graphql::Error>(Ok(Library::new(created)))
    }
    .await?;
    Ok(LibraryPayload::of(input.client_mutation_id, outcome))
}

/// Changes to a library. An absent field is left as it is.
#[derive(Debug, InputObject)]
pub struct UpdateLibraryInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The library, as returned in `Library.id`. The caller must be its admin or an owner.
    pub id: ID,
    /// Its new name: 1 to 255 characters, trimmed. Clears `titleSort`.
    pub title: Option<String>,
    /// The language its metadata is in, as a BCP 47 tag such as `fr-FR`.
    pub language: Option<String>,
    /// Replaces the directories it scans, as `createLibrary` takes them; for owners only. A
    /// root kept keeps its id, and a scan starts: the files of a root taken away leave the
    /// library with it. Refused while the library is being scanned.
    pub roots: Option<Vec<String>>,
}

pub(crate) async fn update(
    context: &Context<'_>,
    input: UpdateLibraryInput,
) -> Result<LibraryPayload> {
    let UpdateLibraryInput {
        client_mutation_id,
        id,
        title: new_title,
        language: new_language,
        roots: new_roots,
    } = input;
    let outcome = async {
        let Some(section) =
            section_id(&id).filter(|section| crate::access::can_edit(context, *section))
        else {
            return Ok(Err(LibraryUserError::new(
                "id",
                LibraryErrorCode::LibraryNotFound,
            )));
        };
        let owner = crate::access::caller(context).is_some_and(klapz_core::Principal::is_owner);
        if new_roots.is_some() && !owner {
            return Ok(Err(LibraryUserError::new(
                "roots",
                LibraryErrorCode::OwnerOnly,
            )));
        }
        let mut edit = LibraryEdit::default();
        if let Some(new_title) = &new_title {
            match title(new_title) {
                Ok(title) => edit.title = Some(title),
                Err(error) => return Ok(Err(error)),
            }
        }
        if let Some(new_language) = &new_language {
            match language(new_language) {
                Ok(language) => edit.language = Some(language),
                Err(error) => return Ok(Err(error)),
            }
        }
        if let Some(new_roots) = &new_roots {
            match roots(context, new_roots).await? {
                Ok(roots) => edit.roots = Some(roots),
                Err(error) => return Ok(Err(error)),
            }
        }
        let api = api(context)?;
        let outcome = edit_library(
            api.library.as_ref(),
            api.scans.as_ref(),
            section,
            &edit,
            now_epoch(),
        )
        .await?;
        Ok::<_, async_graphql::Error>(match outcome {
            Edited::Edited(section) => Ok(Library::new(section)),
            Edited::NotFound => Err(LibraryUserError::new(
                "id",
                LibraryErrorCode::LibraryNotFound,
            )),
            Edited::Scanning => Err(LibraryUserError::new("roots", LibraryErrorCode::Scanning)),
        })
    }
    .await?;
    Ok(LibraryPayload::of(client_mutation_id, outcome))
}

/// A library to delete.
#[derive(Debug, InputObject)]
pub struct DeleteLibraryInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The library, as returned in `Library.id`.
    pub id: ID,
}

/// What a deletion removed.
#[derive(Debug, SimpleObject)]
pub struct DeleteLibraryPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The library deleted, as it was returned in `Library.id`. Null when refused.
    pub deleted_library_id: Option<ID>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<LibraryUserError>,
}

pub(crate) async fn delete(
    context: &Context<'_>,
    input: DeleteLibraryInput,
) -> Result<DeleteLibraryPayload> {
    crate::access::require_owner(context)?;
    let outcome = match section_id(&input.id) {
        None => Deletion::NotFound,
        Some(section) => {
            let api = api(context)?;
            delete_library(
                api.library.as_ref(),
                &api.lifecycle(),
                api.scans.as_ref(),
                section,
            )
            .await?
        }
    };
    let (deleted_library_id, user_errors) = match outcome {
        Deletion::Deleted => (Some(input.id), Vec::new()),
        Deletion::NotFound => (
            None,
            vec![LibraryUserError::new(
                "id",
                LibraryErrorCode::LibraryNotFound,
            )],
        ),
        Deletion::Scanning => (
            None,
            vec![LibraryUserError::new("id", LibraryErrorCode::Scanning)],
        ),
    };
    Ok(DeleteLibraryPayload {
        client_mutation_id: input.client_mutation_id,
        deleted_library_id,
        user_errors,
    })
}
