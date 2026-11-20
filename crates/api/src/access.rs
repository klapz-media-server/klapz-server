//! Who is asking, and who may be given what.
//!
//! Every path out of the schema to a library's content goes through the checks here. A caller
//! attached by the transport is the only source of rights: with none, nothing is visible.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::{Context, Enum, InputObject, Object, Result, SimpleObject, ID};
use klapz_core::access::{
    is_connection_uri, new_external_id, new_token, publish_link, token_digest, AccessError,
    AccessRepository, AccountLink, Extension, LibraryPermission, LibraryRole, NewUser, ServerRole,
    SignedInDevice, User, UserExtension,
};
use klapz_core::account::{normalized_email, normalized_username};
use klapz_core::page::Cursor;
use klapz_core::{now_epoch, Principal, SectionId, UserId};

use crate::account::{AccountErrorCode, AccountUserError};

use crate::cursor::OpaqueCursor;
use crate::library::Library;
use crate::node::GlobalId;
use crate::schema::ApiContext;
use crate::DateTime;

pub const USER_TYPE: &str = "User";
const DEFAULT_PAGE_SIZE: u32 = 50;

/// The caller the transport attached to this request.
pub(crate) fn caller<'c>(context: &Context<'c>) -> Option<&'c Principal> {
    context.data_opt::<Principal>()
}

/// Worded the same whatever the reason: telling a caller why would confirm what exists.
pub(crate) fn denied() -> async_graphql::Error {
    async_graphql::Error::new("not found")
}

pub(crate) fn require_caller<'c>(context: &Context<'c>) -> Result<&'c Principal> {
    caller(context).ok_or_else(denied)
}

/// Whose watch state, history, playlists and bookmarks the caller reads and writes: its own.
/// `None` for a request with no caller, which has none.
pub(crate) fn viewer_id(context: &Context<'_>) -> Option<UserId> {
    caller(context).map(|caller| caller.user)
}

pub(crate) fn can_read(context: &Context<'_>, section: SectionId) -> bool {
    role(context, section).is_some()
}

pub(crate) fn can_edit(context: &Context<'_>, section: SectionId) -> bool {
    caller(context).is_some_and(|caller| caller.libraries.may_edit(section))
}

fn role(context: &Context<'_>, section: SectionId) -> Option<LibraryRole> {
    caller(context)?.libraries.role(section)
}

/// What a listing across libraries is narrowed to. `None` is no restriction.
pub(crate) fn visible_sections(context: &Context<'_>) -> Option<Vec<SectionId>> {
    caller(context).map_or_else(|| Some(Vec::new()), |caller| caller.libraries.sections())
}

pub(crate) fn require_owner<'c>(context: &Context<'c>) -> Result<&'c Principal> {
    caller(context)
        .filter(|caller| caller.is_owner())
        .ok_or_else(denied)
}

/// An owner, or a user acting on itself: how a user pairs its own server.
fn require_owner_or_self(context: &Context<'_>, user: UserId) -> Result<()> {
    if require_caller(context)?.owns(user) {
        Ok(())
    } else {
        Err(denied())
    }
}

fn repository<'c>(context: &Context<'c>) -> Result<&'c dyn AccessRepository> {
    Ok(context.data::<ApiContext>()?.access.as_ref())
}

fn user_id(id: &ID) -> Result<UserId> {
    Ok(UserId::new(GlobalId::decode(id)?.key))
}

/// What a user may do in a library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum LibraryRoleType {
    /// Browse and play.
    Viewer,
    /// Also change the library itself.
    Admin,
}

impl From<LibraryRole> for LibraryRoleType {
    fn from(role: LibraryRole) -> Self {
        match role {
            LibraryRole::Viewer => Self::Viewer,
            LibraryRole::Admin => Self::Admin,
        }
    }
}

impl From<LibraryRoleType> for LibraryRole {
    fn from(role: LibraryRoleType) -> Self {
        match role {
            LibraryRoleType::Viewer => Self::Viewer,
            LibraryRoleType::Admin => Self::Admin,
        }
    }
}

/// What a user may do on the server as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ServerRoleType {
    /// Reaches the libraries it was given, with the role given on each.
    User,
    /// Reaches every library as its admin, and manages users, their libraries and connections,
    /// everyone's playback sessions and viewing history.
    Owner,
}

impl From<ServerRole> for ServerRoleType {
    fn from(role: ServerRole) -> Self {
        match role {
            ServerRole::User => Self::User,
            ServerRole::Owner => Self::Owner,
        }
    }
}

impl From<ServerRoleType> for ServerRole {
    fn from(role: ServerRoleType) -> Self {
        match role {
            ServerRoleType::User => Self::User,
            ServerRoleType::Owner => Self::Owner,
        }
    }
}

/// A protocol a user can connect through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ExtensionType {
    /// The Plex Media Server API.
    Plex,
    /// The Jellyfin server API.
    Jellyfin,
}

impl From<Extension> for ExtensionType {
    fn from(extension: Extension) -> Self {
        match extension {
            Extension::Plex => Self::Plex,
            Extension::Jellyfin => Self::Jellyfin,
        }
    }
}

impl From<ExtensionType> for Extension {
    fn from(extension: ExtensionType) -> Self {
        match extension {
            ExtensionType::Plex => Self::Plex,
            ExtensionType::Jellyfin => Self::Jellyfin,
        }
    }
}

/// The role the caller holds in a library, which is also whether it can see it.
pub(crate) fn viewer_role(context: &Context<'_>, section: SectionId) -> Option<LibraryRoleType> {
    role(context, section).map(Into::into)
}

/// Someone who can be given access to libraries.
#[derive(Debug, Clone)]
pub struct UserObject(pub User);

#[Object(name = "User")]
impl UserObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this user.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(USER_TYPE, self.0.id.get()))
    }

    /// The name the user is known and signs in by: unique, in lower case.
    async fn username(&self) -> &str {
        &self.0.username
    }

    /// The user's e-mail address, which it can also sign in with. Null when it gave none.
    async fn email(&self) -> Option<&str> {
        self.0.email.as_deref()
    }

    /// What the user may do on the server as a whole.
    async fn role(&self) -> ServerRoleType {
        self.0.role.into()
    }

    /// Whether this is the account the server creates for itself, `admin`: an owner that
    /// cannot be demoted, deactivated or deleted.
    async fn built_in(&self) -> bool {
        self.0.is_built_in()
    }

    /// Whether the user may use the server. A user who signs up waits inactive until an owner
    /// lets them in; inactive, nothing it presents opens anything.
    async fn active(&self) -> bool {
        self.0.active
    }

    /// Whether the user can sign in with a password: a user an owner created has none.
    async fn has_password(&self, context: &Context<'_>) -> Result<bool> {
        Ok(repository(context)?.has_password(self.0.id).await?)
    }

    /// When the user was created.
    async fn created_at(&self) -> DateTime {
        self.0.created_at.into()
    }

    /// When the user last changed.
    async fn updated_at(&self) -> DateTime {
        self.0.updated_at.into()
    }

    /// The libraries this user was given, and the role on each.
    async fn library_permissions(
        &self,
        context: &Context<'_>,
    ) -> Result<Connection<OpaqueCursor, LibraryPermissionObject, EmptyFields, EmptyFields>> {
        let permissions = repository(context)?.permissions(self.0.id).await?;
        let mut connection = Connection::new(false, false);
        connection
            .edges
            .extend(permissions.into_iter().map(|permission| {
                Edge::new(
                    OpaqueCursor(Cursor::by_id(permission.section.get())),
                    LibraryPermissionObject(permission),
                )
            }));
        Ok(connection)
    }

    /// The protocols this user is connected through.
    async fn extensions(
        &self,
        context: &Context<'_>,
    ) -> Result<Connection<OpaqueCursor, UserExtensionObject, EmptyFields, EmptyFields>> {
        let extensions = repository(context)?.extensions(self.0.id).await?;
        let mut connection = Connection::new(false, false);
        connection
            .edges
            .extend(extensions.into_iter().zip(0_i64..).map(|(extension, at)| {
                Edge::new(
                    OpaqueCursor(Cursor::by_id(at)),
                    UserExtensionObject(extension),
                )
            }));
        Ok(connection)
    }
}

/// One user's role on one library.
#[derive(Debug, Clone, Copy)]
pub struct LibraryPermissionObject(LibraryPermission);

#[Object(name = "LibraryPermission")]
impl LibraryPermissionObject {
    /// The library concerned. Null if it has left the server since.
    async fn library(&self, context: &Context<'_>) -> Result<Option<Library>> {
        crate::schema::load_library(context, self.0.section).await
    }

    /// What the user may do there.
    async fn role(&self) -> LibraryRoleType {
        self.0.role.into()
    }
}

/// One user connected through one extension.
#[derive(Debug, Clone)]
pub struct UserExtensionObject(UserExtension);

#[Object(name = "UserExtension")]
impl UserExtensionObject {
    /// The protocol.
    async fn extension(&self) -> ExtensionType {
        self.0.extension.into()
    }

    /// What the protocol calls this user's side of the server: for Plex, a machine identifier.
    /// A Jellyfin connection has one too, unused while every Jellyfin user shares the
    /// server's identifier.
    async fn external_id(&self) -> &str {
        &self.0.external_id
    }

    /// Whether the connection is on. Off, its token opens nothing.
    async fn enabled(&self) -> bool {
        self.0.enabled
    }

    /// The account this connection is paired with, if any.
    async fn account(&self, context: &Context<'_>) -> Result<Option<AccountLinkObject>> {
        Ok(repository(context)?
            .account_link(self.0.user, self.0.extension)
            .await?
            .map(AccountLinkObject))
    }

    /// The devices signed in through this connection with the user's password, each holding a
    /// token of its own: Jellyfin's clients. The latest sign-in first; empty for Plex.
    async fn devices(&self, context: &Context<'_>) -> Result<Vec<ExtensionDeviceObject>> {
        Ok(repository(context)?
            .devices(self.0.user, self.0.extension)
            .await?
            .into_iter()
            .map(ExtensionDeviceObject)
            .collect())
    }
}

/// A device signed in through an extension.
#[derive(Debug, Clone)]
pub struct ExtensionDeviceObject(SignedInDevice);

#[Object(name = "ExtensionDevice")]
impl ExtensionDeviceObject {
    /// What the client calls the device, stable across its sign-ins: what
    /// `forgetExtensionDevice` takes.
    async fn device_id(&self) -> &str {
        &self.0.device.id
    }

    /// The device's name, for a person.
    async fn device_name(&self) -> &str {
        &self.0.device.name
    }

    /// The application that signed in.
    async fn client(&self) -> &str {
        &self.0.device.client
    }

    /// The application's version.
    async fn client_version(&self) -> &str {
        &self.0.device.client_version
    }

    /// Its last sign-in.
    async fn signed_in_at(&self) -> DateTime {
        self.0.signed_in_at.into()
    }
}

pub(crate) async fn forget_extension_device(
    context: &Context<'_>,
    user: &ID,
    extension: ExtensionType,
    device_id: &str,
) -> Result<bool> {
    let user = user_id(user)?;
    require_owner_or_self(context, user)?;
    Ok(repository(context)?
        .forget_device(user, extension.into(), device_id)
        .await?)
}

/// A connection tied to an account on the extension's own service. The account's token stays
/// on the server: no field gives it out.
#[derive(Debug, Clone)]
pub struct AccountLinkObject(AccountLink);

#[Object(name = "AccountLink")]
impl AccountLinkObject {
    /// The account's name on that service.
    async fn account_name(&self) -> &str {
        &self.0.account.name
    }

    /// The address announced to that service for clients to reach this server.
    async fn connection_uri(&self) -> &str {
        &self.0.connection_uri
    }

    /// When that service last accepted the address. Null until it
    /// has; the server keeps trying.
    async fn published_at(&self) -> Option<DateTime> {
        self.0.published_at.map(DateTime::from)
    }
}

/// What connecting through an extension yields. The token cannot be read back later.
#[derive(Debug, Clone)]
pub struct ExtensionCredentials {
    extension: UserExtension,
    token: String,
}

#[Object]
impl ExtensionCredentials {
    /// The connection made.
    async fn user_extension(&self) -> UserExtensionObject {
        UserExtensionObject(self.extension.clone())
    }

    /// The token to give the client. Shown only here: the server keeps a digest of it.
    async fn token(&self) -> &str {
        &self.token
    }
}

/// A role to give a user on a library.
#[derive(Debug, InputObject)]
pub struct SetLibraryPermissionInput {
    /// The user, as returned in `User.id`.
    pub user_id: ID,
    /// The library, as returned in `Library.id`.
    pub library_id: ID,
    /// What the user may do there.
    pub role: LibraryRoleType,
}

/// What pairing a connection with an account takes.
#[derive(Debug, InputObject)]
pub struct PairExtensionInput {
    /// The user, as returned in `User.id`. It must already be connected through the extension.
    pub user_id: ID,
    /// The protocol to pair.
    pub extension: ExtensionType,
    /// The single-use code the account's owner got from the service: for Plex, from
    /// plex.tv/claim.
    pub code: String,
    /// Where clients can reach this server, as `https://host:port`.
    pub connection_uri: String,
}

/// A new address for a paired server.
#[derive(Debug, InputObject)]
pub struct MoveExtensionInput {
    /// The user, as returned in `User.id`. Its connection must be paired.
    pub user_id: ID,
    /// The protocol whose address changes.
    pub extension: ExtensionType,
    /// Where clients can now reach this server, as `https://host:port`.
    pub connection_uri: String,
}

/// Who is asking: what a client needs to know before it offers to manage users or a
/// connection.
#[derive(Debug, Clone, Copy)]
pub struct Viewer {
    caller: Option<(UserId, ServerRole)>,
}

#[Object]
impl Viewer {
    /// The caller's role on the server: an owner manages users and sees every library. Null
    /// for a request with no caller, which has signed in as nobody.
    async fn role(&self) -> Option<ServerRoleType> {
        self.caller.map(|(_, role)| role.into())
    }

    /// The user asking. Null for a request with no caller.
    async fn user(&self, context: &Context<'_>) -> Result<Option<UserObject>> {
        let Some((user, _)) = self.caller else {
            return Ok(None);
        };
        Ok(repository(context)?.user(user).await?.map(UserObject))
    }
}

pub(crate) fn viewer(context: &Context<'_>) -> Viewer {
    Viewer {
        caller: caller(context).map(|caller| (caller.user, caller.role)),
    }
}

pub(crate) async fn users(
    context: &Context<'_>,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<Connection<OpaqueCursor, UserObject, EmptyFields, EmptyFields>> {
    require_owner(context)?;
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let window = crate::schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?;
            let page = repository(context)?.users(window).await?;
            let mut connection = Connection::new(page.has_previous, page.has_next);
            connection.edges.extend(page.items.into_iter().map(|user| {
                Edge::new(OpaqueCursor(Cursor::by_id(user.id.get())), UserObject(user))
            }));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// A user is visible to an owner and to itself.
pub(crate) async fn user_node(context: &Context<'_>, id: i64) -> Result<Option<UserObject>> {
    let id = UserId::new(id);
    if !require_caller(context)?.owns(id) {
        return Ok(None);
    }
    Ok(repository(context)?.user(id).await?.map(UserObject))
}

/// A user to create.
#[derive(Debug, InputObject)]
pub struct CreateUserInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// 3 to 32 letters, digits, `.`, `_` or `-`, starting with a letter or a digit. Stored in
    /// lower case: two users cannot differ by case alone.
    pub username: String,
    /// An address the user can also sign in with. Compared without regard to case.
    pub email: Option<String>,
}

/// What `createUser` did.
#[derive(Debug, SimpleObject)]
pub struct CreateUserPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// The user created: active, with no password and no library. Null when none was.
    pub user: Option<UserObject>,
    /// Why none was. Empty when one was.
    pub user_errors: Vec<AccountUserError>,
}

pub(crate) async fn create_user(
    context: &Context<'_>,
    input: CreateUserInput,
) -> Result<CreateUserPayload> {
    require_owner(context)?;
    let refuse = |field: &str, code| {
        Ok(CreateUserPayload {
            client_mutation_id: input.client_mutation_id.clone(),
            user: None,
            user_errors: vec![AccountUserError::new(field, code)],
        })
    };
    let Some(username) = normalized_username(&input.username) else {
        return refuse("username", AccountErrorCode::InvalidUsername);
    };
    let email = match input.email.as_deref().map(normalized_email) {
        None => None,
        Some(Some(email)) => Some(email),
        Some(None) => return refuse("email", AccountErrorCode::InvalidEmail),
    };
    let created = repository(context)?
        .create_user(
            NewUser {
                username: &username,
                email: email.as_deref(),
                role: ServerRole::User,
                active: true,
                password_hash: None,
            },
            now_epoch(),
        )
        .await;
    match created {
        Ok(user) => Ok(CreateUserPayload {
            client_mutation_id: input.client_mutation_id,
            user: Some(UserObject(user)),
            user_errors: Vec::new(),
        }),
        Err(AccessError::UsernameTaken) => refuse("username", AccountErrorCode::UsernameTaken),
        Err(AccessError::EmailTaken) => refuse("email", AccountErrorCode::EmailTaken),
        Err(error) => Err(error.into()),
    }
}

/// The user a mutation names.
#[derive(Debug, InputObject)]
pub struct UserInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The user, as returned in `User.id`.
    pub user_id: ID,
}

/// What a change to a user did.
#[derive(Debug, SimpleObject)]
pub struct UserPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// The user as it now is. Null when it could not be changed.
    pub user: Option<UserObject>,
    /// Why it could not. Empty when it was.
    pub user_errors: Vec<AccountUserError>,
}

/// A server role to give a user.
#[derive(Debug, InputObject)]
pub struct SetUserRoleInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The user, as returned in `User.id`.
    pub user_id: ID,
    /// What the user may now do on the server.
    pub role: ServerRoleType,
}

/// What `deleteUser` did.
#[derive(Debug, SimpleObject)]
pub struct DeleteUserPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// Whether a user was deleted.
    pub deleted: bool,
    /// Why none was. Empty when one was.
    pub user_errors: Vec<AccountUserError>,
}

/// The user an owner names, unless it is the built-in account and `protect` says the change
/// would leave the server without its admin. The error to answer otherwise.
async fn managed_user(
    context: &Context<'_>,
    id: &ID,
    protect: bool,
) -> Result<std::result::Result<User, AccountUserError>> {
    require_owner(context)?;
    let found = match GlobalId::decode(id) {
        Ok(decoded) if decoded.type_name == USER_TYPE => {
            repository(context)?.user(UserId::new(decoded.key)).await?
        }
        _ => None,
    };
    Ok(match found {
        None => Err(AccountUserError::new(
            "userId",
            AccountErrorCode::UserNotFound,
        )),
        Some(user) if protect && user.is_built_in() => Err(AccountUserError::new(
            "userId",
            AccountErrorCode::BuiltInAccount,
        )),
        Some(user) => Ok(user),
    })
}

pub(crate) async fn set_role(
    context: &Context<'_>,
    input: SetUserRoleInput,
) -> Result<UserPayload> {
    let role = ServerRole::from(input.role);
    let user = match managed_user(context, &input.user_id, role != ServerRole::Owner).await? {
        Ok(user) => user,
        Err(error) => {
            return Ok(UserPayload {
                client_mutation_id: input.client_mutation_id,
                user: None,
                user_errors: vec![error],
            })
        }
    };
    let changed = repository(context)?
        .set_role(user.id, role, now_epoch())
        .await?;
    Ok(UserPayload {
        client_mutation_id: input.client_mutation_id,
        user: changed.map(UserObject),
        user_errors: Vec::new(),
    })
}

pub(crate) async fn set_active(
    context: &Context<'_>,
    input: UserInput,
    active: bool,
) -> Result<UserPayload> {
    let user = match managed_user(context, &input.user_id, !active).await? {
        Ok(user) => user,
        Err(error) => {
            return Ok(UserPayload {
                client_mutation_id: input.client_mutation_id,
                user: None,
                user_errors: vec![error],
            })
        }
    };
    let changed = repository(context)?
        .set_active(user.id, active, now_epoch())
        .await?;
    Ok(UserPayload {
        client_mutation_id: input.client_mutation_id,
        user: changed.map(UserObject),
        user_errors: Vec::new(),
    })
}

pub(crate) async fn delete_user(
    context: &Context<'_>,
    input: UserInput,
) -> Result<DeleteUserPayload> {
    let user = match managed_user(context, &input.user_id, true).await? {
        Ok(user) => user,
        Err(error) => {
            return Ok(DeleteUserPayload {
                client_mutation_id: input.client_mutation_id,
                deleted: false,
                user_errors: vec![error],
            })
        }
    };
    Ok(DeleteUserPayload {
        client_mutation_id: input.client_mutation_id,
        deleted: repository(context)?.delete_user(user.id).await?,
        user_errors: Vec::new(),
    })
}

pub(crate) async fn set_library_permission(
    context: &Context<'_>,
    input: SetLibraryPermissionInput,
) -> Result<bool> {
    require_owner(context)?;
    let section = SectionId::new(GlobalId::decode(&input.library_id)?.key);
    if crate::schema::load_library(context, section)
        .await?
        .is_none()
    {
        return Err(denied());
    }
    repository(context)?
        .set_permission(
            LibraryPermission {
                user: user_id(&input.user_id)?,
                section,
                role: input.role.into(),
            },
            now_epoch(),
        )
        .await?;
    Ok(true)
}

pub(crate) async fn remove_library_permission(
    context: &Context<'_>,
    user: &ID,
    library: &ID,
) -> Result<bool> {
    require_owner(context)?;
    let section = SectionId::new(GlobalId::decode(library)?.key);
    Ok(repository(context)?
        .remove_permission(user_id(user)?, section)
        .await?)
}

/// An owner, or for an extension its users connect themselves, the user itself.
fn require_connector(context: &Context<'_>, user: UserId, extension: ExtensionType) -> Result<()> {
    if Extension::from(extension).connected_by_its_user() {
        require_owner_or_self(context, user)
    } else {
        require_owner(context).map(drop)
    }
}

pub(crate) async fn disable_extension(
    context: &Context<'_>,
    user: &ID,
    extension: ExtensionType,
) -> Result<bool> {
    require_connector(context, user_id(user)?, extension)?;
    Ok(repository(context)?
        .disable_extension(user_id(user)?, extension.into(), now_epoch())
        .await?)
}

pub(crate) async fn connect_extension(
    context: &Context<'_>,
    user: &ID,
    extension: ExtensionType,
) -> Result<ExtensionCredentials> {
    require_connector(context, user_id(user)?, extension)?;
    let token = new_token();
    let extension = repository(context)?
        .connect_extension(
            user_id(user)?,
            extension.into(),
            &new_external_id(),
            &token_digest(&token),
            now_epoch(),
        )
        .await?;
    Ok(ExtensionCredentials { extension, token })
}

/// The address as stored: checked, and in lower case, since it is compared with `Host`
/// headers, which are matched in lower case.
fn connection_uri(uri: &str) -> Result<String> {
    if !is_connection_uri(uri) {
        return Err(async_graphql::Error::new(
            "the address must look like https://host:port, with nothing after it",
        ));
    }
    Ok(uri.to_ascii_lowercase())
}

/// Tells the service where the link's server is, recording when it took. A failure is
/// logged, not returned: the link is stored, the server publishes again on its own, and
/// `publishedAt` stays null until one attempt succeeds.
async fn publish_now(api: &ApiContext, link: &mut AccountLink) {
    match publish_link(
        api.access.as_ref(),
        api.directory.as_ref(),
        link,
        now_epoch(),
    )
    .await
    {
        Ok(published_at) => link.published_at = Some(published_at),
        Err(error) => tracing::warn!(%error, "could not publish a server's address"),
    }
}

pub(crate) async fn move_extension(
    context: &Context<'_>,
    input: MoveExtensionInput,
) -> Result<AccountLinkObject> {
    let user = user_id(&input.user_id)?;
    require_owner_or_self(context, user)?;
    let connection_uri = connection_uri(&input.connection_uri)?;
    let api = context.data::<ApiContext>()?;
    let mut link = api
        .access
        .move_account(user, input.extension.into(), &connection_uri, now_epoch())
        .await?
        .ok_or_else(|| async_graphql::Error::new("this connection is not paired"))?;
    publish_now(api, &mut link).await;
    Ok(AccountLinkObject(link))
}

pub(crate) async fn pair_extension(
    context: &Context<'_>,
    input: PairExtensionInput,
) -> Result<AccountLinkObject> {
    let user = user_id(&input.user_id)?;
    require_owner_or_self(context, user)?;
    let connection_uri = connection_uri(&input.connection_uri)?;
    let api = context.data::<ApiContext>()?;
    let extension: Extension = input.extension.into();
    let connection = api
        .access
        .extensions(user)
        .await?
        .into_iter()
        .find(|connection| connection.extension == extension && connection.enabled)
        .ok_or(AccessError::NotConnected)?;
    let account = api
        .directory
        .redeem(&connection.external_id, input.code.trim())
        .await?;
    let mut link = api
        .access
        .link_account(user, extension, &account, &connection_uri, now_epoch())
        .await?;
    // The code is spent and the link stored: failing to publish must not hide that.
    publish_now(api, &mut link).await;
    Ok(AccountLinkObject(link))
}

pub(crate) async fn viewer_account(
    context: &Context<'_>,
    extension: ExtensionType,
) -> Result<Option<AccountLinkObject>> {
    let Some(user) = viewer_id(context) else {
        return Ok(None);
    };
    Ok(repository(context)?
        .account_link(user, extension.into())
        .await?
        .map(AccountLinkObject))
}

pub(crate) async fn unpair_extension(
    context: &Context<'_>,
    user: &ID,
    extension: ExtensionType,
) -> Result<bool> {
    let user = user_id(user)?;
    require_owner_or_self(context, user)?;
    Ok(repository(context)?
        .unlink_account(user, extension.into())
        .await?)
}
