//! The server's settings, and the values libraries hold in place of some, for its owners.
//!
//! A setting is no `Node`: it is a value of the server, named by its key, and lives on
//! `Server.settings` as the transcoder's capabilities live on `Server.transcoder`; a library's
//! own value lives on `Library.settings`.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::{Context, Enum, InputObject, Object, OutputType, Result, SimpleObject, ID};
use klapz_core::page::{Cursor, MAX_ITEMS};
use klapz_core::settings::{
    self, LibrarySetting, Rejected, Setting, SettingError, SettingKind, SettingUpdate, MAX_UPDATES,
};
use klapz_core::{now_epoch, SectionId};

use crate::access::UserObject;
use crate::cursor::OpaqueCursor;
use crate::schema::ApiContext;
use crate::{DateTime, Int64};

/// What values a setting takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ServerSettingKind {
    /// One of the words in `choices`.
    Choice,
    /// A whole number between `minimum` and `maximum`, both included.
    Integer,
    /// `true` or `false`.
    Boolean,
    /// An address clients can be sent to: `http` or `https`, a host, perhaps a port, nothing
    /// after; in lower case. Empty for none.
    Address,
}

/// A setting of the server, with the value in force.
#[derive(Debug, Clone)]
pub struct ServerSettingObject(Setting);

#[Object(name = "ServerSetting")]
impl ServerSettingObject {
    /// Dotted lower-case words naming it, the machinery it drives first, such as
    /// `transcoder.h264.preset`. What `updateServerSettings` takes.
    async fn key(&self) -> &str {
        self.0.definition.key
    }

    /// What it changes, in English.
    async fn description(&self) -> &str {
        self.0.definition.description
    }

    /// What values it takes.
    async fn kind(&self) -> ServerSettingKind {
        kind(self.0.definition.kind)
    }

    /// Whether a library may hold a value of its own, which it follows instead of this one:
    /// see `Library.settings`.
    async fn per_library(&self) -> bool {
        self.0.definition.per_library
    }

    /// The value in force, in its canonical form: a choice as listed, a number in decimal,
    /// `true` or `false`.
    async fn value(&self) -> String {
        self.0.value()
    }

    /// The value in force while no owner chose one.
    async fn default_value(&self) -> &str {
        self.0.definition.default
    }

    /// Whether no owner chose a value, the default being in force. A value an owner chose is
    /// kept even when it equals the default.
    async fn is_default(&self) -> bool {
        self.0.stored.is_none()
    }

    /// The words a `CHOICE` takes, in the order the server lists them. Null for another kind.
    async fn choices(&self) -> Option<Vec<&str>> {
        match self.0.definition.kind {
            SettingKind::Choice(choices) => Some(choices.to_vec()),
            _ => None,
        }
    }

    /// The least value an `INTEGER` takes. Null for another kind.
    async fn minimum(&self) -> Option<Int64> {
        match self.0.definition.kind {
            SettingKind::Integer { minimum, .. } => Some(Int64(minimum)),
            _ => None,
        }
    }

    /// The greatest value an `INTEGER` takes. Null for another kind.
    async fn maximum(&self) -> Option<Int64> {
        match self.0.definition.kind {
            SettingKind::Integer { maximum, .. } => Some(Int64(maximum)),
            _ => None,
        }
    }

    /// When an owner last chose its value. Null while the default is in force.
    async fn updated_at(&self) -> Option<DateTime> {
        self.0
            .stored
            .as_ref()
            .map(|stored| stored.updated_at.into())
    }

    /// The owner who last chose its value. Null while the default is in force, and when that
    /// account is gone.
    async fn updated_by(&self, context: &Context<'_>) -> Result<Option<UserObject>> {
        let Some(user) = self.0.stored.as_ref().and_then(|stored| stored.updated_by) else {
            return Ok(None);
        };
        crate::access::user_node(context, user.get()).await
    }
}

fn kind(kind: SettingKind) -> ServerSettingKind {
    match kind {
        SettingKind::Choice(_) => ServerSettingKind::Choice,
        SettingKind::Integer { .. } => ServerSettingKind::Integer,
        SettingKind::Boolean => ServerSettingKind::Boolean,
        SettingKind::Address => ServerSettingKind::Address,
    }
}

/// A setting as one library sees it: its own value, or the server's while it holds none.
#[derive(Debug, Clone)]
pub struct LibrarySettingObject(LibrarySetting);

#[Object(name = "LibrarySetting")]
impl LibrarySettingObject {
    /// The setting's key, as `ServerSetting.key` names it, where the values it takes are
    /// described. What `updateLibrarySettings` takes.
    async fn key(&self) -> &str {
        self.0.server.definition.key
    }

    /// What it changes, in English.
    async fn description(&self) -> &str {
        self.0.server.definition.description
    }

    /// What values it takes.
    async fn kind(&self) -> ServerSettingKind {
        kind(self.0.server.definition.kind)
    }

    /// The value in force for the library, in its canonical form: its own, else the server's.
    async fn value(&self) -> String {
        self.0.value()
    }

    /// Whether the library holds no value of its own and follows the server's, which
    /// `serverValue` gives.
    async fn is_inherited(&self) -> bool {
        self.0.stored.is_none()
    }

    /// The server's value, the one in force while the library holds none of its own.
    async fn server_value(&self) -> String {
        self.0.server.value()
    }

    /// When an owner last chose the library's own value. Null while it follows the server.
    async fn updated_at(&self) -> Option<DateTime> {
        self.0
            .stored
            .as_ref()
            .map(|stored| stored.updated_at.into())
    }

    /// The owner who last chose the library's own value. Null while it follows the server,
    /// and when that account is gone.
    async fn updated_by(&self, context: &Context<'_>) -> Result<Option<UserObject>> {
        let Some(user) = self.0.stored.as_ref().and_then(|stored| stored.updated_by) else {
            return Ok(None);
        };
        crate::access::user_node(context, user.get()).await
    }
}

fn repository<'c>(context: &Context<'c>) -> Result<&'c dyn settings::SettingsRepository> {
    Ok(context.data::<ApiContext>()?.settings.as_ref())
}

fn is_owner(context: &Context<'_>) -> bool {
    crate::access::caller(context).is_some_and(klapz_core::Principal::is_owner)
}

/// What the analysis reads is read before its every round, but a round may be waiting for a
/// file to change: it is told.
fn settings_changed(context: &Context<'_>) -> Result<()> {
    context.data::<ApiContext>()?.scans.settings_changed();
    Ok(())
}

/// `Library.settings`: empty for anyone but an owner, as `Server.settings` is.
pub(crate) async fn library_settings(
    context: &Context<'_>,
    section: SectionId,
    namespace: Option<String>,
    after: Option<String>,
    first: Option<i32>,
) -> Result<Connection<OpaqueCursor, LibrarySettingObject, EmptyFields, EmptyFields>> {
    let listed = if is_owner(context) {
        settings::library_settings(
            repository(context)?,
            section,
            namespace.as_deref().map(str::trim),
        )
        .await?
    } else {
        Vec::new()
    };
    by_position(
        listed.into_iter().map(LibrarySettingObject).collect(),
        after,
        first,
    )
    .await
}

/// `Server.settings`: empty for anyone but an owner, as if the server had none.
pub(crate) async fn server_settings(
    context: &Context<'_>,
    namespace: Option<String>,
    after: Option<String>,
    first: Option<i32>,
) -> Result<Connection<OpaqueCursor, ServerSettingObject, EmptyFields, EmptyFields>> {
    let listed = if is_owner(context) {
        settings::settings(repository(context)?, namespace.as_deref().map(str::trim)).await?
    } else {
        Vec::new()
    };
    by_position(
        listed.into_iter().map(ServerSettingObject).collect(),
        after,
        first,
    )
    .await
}

/// A short list held whole, paged forward by position.
async fn by_position<T: OutputType>(
    listed: Vec<T>,
    after: Option<String>,
    first: Option<i32>,
) -> Result<Connection<OpaqueCursor, T, EmptyFields, EmptyFields>> {
    query(
        after,
        None,
        first,
        None,
        |after: Option<OpaqueCursor>, _: Option<OpaqueCursor>, first, _| async move {
            let start = after.map_or(0, |cursor| {
                usize::try_from(cursor.0.id).map_or(0, |position| position + 1)
            });
            let count = first.unwrap_or(MAX_ITEMS as usize);
            if count > MAX_ITEMS as usize {
                return Err(async_graphql::Error::new(format!(
                    "a page of more than {MAX_ITEMS} is not served"
                )));
            }
            let mut connection = Connection::new(start > 0, listed.len() > start + count);
            connection
                .edges
                .extend(listed.into_iter().enumerate().skip(start).take(count).map(
                    |(position, node)| {
                        Edge::new(
                            OpaqueCursor(Cursor::by_id(
                                i64::try_from(position).unwrap_or(i64::MAX),
                            )),
                            node,
                        )
                    },
                ));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// Why a setting was not changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ServerSettingErrorCode {
    /// No setting has this key.
    UnknownSetting,
    /// The value is not one this setting takes.
    InvalidValue,
    /// The same key came twice in one change.
    RepeatedSetting,
    /// More than 100 settings came in one change.
    TooManySettings,
    /// A library was given a value of a setting that is the server's alone:
    /// `ServerSetting.perLibrary` is false.
    ServerOnly,
    /// No library has this id.
    LibraryNotFound,
}

/// A problem with what was asked, as opposed to a failure of the server.
#[derive(Debug, Clone, SimpleObject)]
pub struct ServerSettingUserError {
    /// The input field at fault, as a path from the input's root: `["settings", "0", "value"]`.
    pub field: Vec<String>,
    /// What went wrong, for a person.
    pub message: String,
    /// What went wrong, for a program.
    pub code: ServerSettingErrorCode,
}

impl ServerSettingUserError {
    fn of(rejected: Rejected, updates: &[SettingUpdate]) -> Self {
        let key = &updates[rejected.index].key;
        let (field, code, message) = match rejected.error {
            SettingError::UnknownSetting => (
                "key",
                ServerSettingErrorCode::UnknownSetting,
                format!("no setting is named `{key}`"),
            ),
            SettingError::InvalidValue => (
                "value",
                ServerSettingErrorCode::InvalidValue,
                format!("`{key}` does not take this value"),
            ),
            SettingError::RepeatedSetting => (
                "key",
                ServerSettingErrorCode::RepeatedSetting,
                format!("`{key}` is named more than once"),
            ),
            SettingError::ServerOnly => (
                "key",
                ServerSettingErrorCode::ServerOnly,
                format!("`{key}` is the server's alone: no library holds a value of it"),
            ),
        };
        Self {
            field: vec![
                "settings".to_owned(),
                rejected.index.to_string(),
                field.to_owned(),
            ],
            message,
            code,
        }
    }
}

/// One setting to change.
#[derive(Debug, InputObject)]
pub struct ServerSettingInput {
    /// The setting, as `ServerSetting.key` names it.
    pub key: String,
    /// Its new value, of its kind; surrounding blanks are ignored. Null to forget the value
    /// chosen, back to the default.
    pub value: Option<String>,
}

/// Settings to change together.
#[derive(Debug, InputObject)]
pub struct UpdateServerSettingsInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// Each setting at most once, 100 at most.
    pub settings: Vec<ServerSettingInput>,
}

/// What `updateServerSettings` did.
#[derive(Debug, SimpleObject)]
pub struct UpdateServerSettingsPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// The settings named, as they now are, in the order asked. Empty when nothing changed.
    pub settings: Vec<ServerSettingObject>,
    /// Why nothing changed: one setting refused, none is changed. Empty when all were.
    pub user_errors: Vec<ServerSettingUserError>,
}

/// What an owner asked, or why none of it is taken before anything is checked.
fn updates(
    settings: &[ServerSettingInput],
) -> std::result::Result<Vec<SettingUpdate>, ServerSettingUserError> {
    if settings.len() > MAX_UPDATES {
        return Err(ServerSettingUserError {
            field: vec!["settings".to_owned()],
            message: format!("a change names {MAX_UPDATES} settings at most"),
            code: ServerSettingErrorCode::TooManySettings,
        });
    }
    Ok(settings
        .iter()
        .map(|setting| SettingUpdate {
            key: setting.key.trim().to_owned(),
            value: setting.value.clone(),
        })
        .collect())
}

fn refused(rejected: Vec<Rejected>, updates: &[SettingUpdate]) -> Vec<ServerSettingUserError> {
    rejected
        .into_iter()
        .map(|rejected| ServerSettingUserError::of(rejected, updates))
        .collect()
}

pub(crate) async fn update_server_settings(
    context: &Context<'_>,
    input: UpdateServerSettingsInput,
) -> Result<UpdateServerSettingsPayload> {
    let owner = crate::access::require_owner(context)?;
    let refuse = |user_errors| UpdateServerSettingsPayload {
        client_mutation_id: input.client_mutation_id.clone(),
        settings: Vec::new(),
        user_errors,
    };
    let updates = match updates(&input.settings) {
        Ok(updates) => updates,
        Err(error) => return Ok(refuse(vec![error])),
    };
    match settings::update_settings(repository(context)?, &updates, owner.user, now_epoch()).await?
    {
        Ok(changed) => {
            settings_changed(context)?;
            Ok(UpdateServerSettingsPayload {
                client_mutation_id: input.client_mutation_id.clone(),
                settings: changed.into_iter().map(ServerSettingObject).collect(),
                user_errors: Vec::new(),
            })
        }
        Err(rejected) => Ok(refuse(refused(rejected, &updates))),
    }
}

/// A library's own values to change together.
#[derive(Debug, InputObject)]
pub struct UpdateLibrarySettingsInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The library, as returned in `Library.id`.
    pub library_id: ID,
    /// Each setting at most once, 100 at most, each one whose `ServerSetting.perLibrary` is
    /// true. A null value makes the library follow the server's again.
    pub settings: Vec<ServerSettingInput>,
}

/// What `updateLibrarySettings` did.
#[derive(Debug, SimpleObject)]
pub struct UpdateLibrarySettingsPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// The library, as it now is. Null when nothing changed.
    pub library: Option<crate::library::Library>,
    /// The settings named, as the library now sees them, in the order asked. Empty when
    /// nothing changed.
    pub settings: Vec<LibrarySettingObject>,
    /// Why nothing changed: one setting refused, none is changed. Empty when all were.
    pub user_errors: Vec<ServerSettingUserError>,
}

pub(crate) async fn update_library_settings(
    context: &Context<'_>,
    input: UpdateLibrarySettingsInput,
) -> Result<UpdateLibrarySettingsPayload> {
    let owner = crate::access::require_owner(context)?;
    let refuse = |user_errors| UpdateLibrarySettingsPayload {
        client_mutation_id: input.client_mutation_id.clone(),
        library: None,
        settings: Vec::new(),
        user_errors,
    };
    let library = match crate::library_management::section_id(&input.library_id) {
        Some(section) => match context.data::<ApiContext>()?.library.section(section).await {
            Ok(section) => Some(section),
            Err(klapz_core::LibraryError::SectionNotFound(_)) => None,
            Err(error) => return Err(error.into()),
        },
        None => None,
    };
    let Some(library) = library else {
        return Ok(refuse(vec![ServerSettingUserError {
            field: vec!["libraryId".to_owned()],
            message: "no such library".to_owned(),
            code: ServerSettingErrorCode::LibraryNotFound,
        }]));
    };
    let updates = match updates(&input.settings) {
        Ok(updates) => updates,
        Err(error) => return Ok(refuse(vec![error])),
    };
    match settings::update_library_settings(
        repository(context)?,
        library.id,
        &updates,
        owner.user,
        now_epoch(),
    )
    .await?
    {
        Ok(changed) => {
            settings_changed(context)?;
            Ok(UpdateLibrarySettingsPayload {
                client_mutation_id: input.client_mutation_id.clone(),
                library: Some(crate::library::Library::new(library)),
                settings: changed.into_iter().map(LibrarySettingObject).collect(),
                user_errors: Vec::new(),
            })
        }
        Err(rejected) => Ok(refuse(refused(rejected, &updates))),
    }
}
