//! Who is asking, and who may be given what.
//!
//! Every path out of the schema to a library's content goes through the checks here. A caller
//! attached by the transport is the only source of rights: with none, nothing is visible.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::{Context, Enum, InputObject, Object, Result, ID};
use open_plex_core::access::{
    is_connection_uri, new_external_id, new_token, publish_link, token_digest, AccessError,
    AccessRepository, AccountLink, Extension, LibraryPermission, LibraryRole, User, UserExtension,
};
use open_plex_core::page::Cursor;
use open_plex_core::{now_epoch, Principal, Scope, SectionId, UserId};

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

pub(crate) fn require_operator(context: &Context<'_>) -> Result<()> {
    if caller(context).is_some_and(|caller| caller.scope.allows(Scope::Admin)) {
        Ok(())
    } else {
        Err(denied())
    }
}

/// The operator, or a user acting on itself: how a user pairs its own server.
fn require_operator_or_self(context: &Context<'_>, user: UserId) -> Result<()> {
    let caller = require_caller(context)?;
    if caller.scope.allows(Scope::Admin) || caller.user == Some(user) {
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

/// A protocol a user can connect through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ExtensionType {
    /// The Plex Media Server API.
    Plex,
}

impl From<Extension> for ExtensionType {
    fn from(extension: Extension) -> Self {
        match extension {
            Extension::Plex => Self::Plex,
        }
    }
}

impl From<ExtensionType> for Extension {
    fn from(extension: ExtensionType) -> Self {
        match extension {
            ExtensionType::Plex => Self::Plex,
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

    /// The server-local number.
    async fn database_id(&self) -> String {
        self.0.id.to_string()
    }

    /// The user's e-mail address.
    async fn email(&self) -> &str {
        &self.0.email
    }

    /// Whether the user may use the server. A user who signs up waits inactive until the
    /// operator lets them in; inactive, nothing it presents opens anything.
    async fn active(&self) -> bool {
        self.0.active
    }

    /// Whether the user can sign in with a password: a user the operator created has none.
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
    operator: bool,
    user: Option<UserId>,
}

#[Object]
impl Viewer {
    /// Whether the caller is the server's operator, who manages users and sees every library.
    async fn is_operator(&self) -> bool {
        self.operator
    }

    /// The caller as a user. Null for the operator, who is nobody's user, and for a request
    /// with no caller.
    async fn user(&self, context: &Context<'_>) -> Result<Option<UserObject>> {
        let Some(user) = self.user else {
            return Ok(None);
        };
        Ok(repository(context)?.user(user).await?.map(UserObject))
    }
}

pub(crate) fn viewer(context: &Context<'_>) -> Viewer {
    let caller = caller(context);
    Viewer {
        operator: caller.is_some_and(|caller| caller.scope.allows(Scope::Admin)),
        user: caller.and_then(|caller| caller.user),
    }
}

pub(crate) async fn users(
    context: &Context<'_>,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<Connection<OpaqueCursor, UserObject, EmptyFields, EmptyFields>> {
    require_operator(context)?;
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

/// A user is visible to the operator and to itself.
pub(crate) async fn user_node(context: &Context<'_>, id: i64) -> Result<Option<UserObject>> {
    let id = UserId::new(id);
    if !require_caller(context)?.owns(Some(id)) {
        return Ok(None);
    }
    Ok(repository(context)?.user(id).await?.map(UserObject))
}

pub(crate) async fn create_user(context: &Context<'_>, email: String) -> Result<UserObject> {
    require_operator(context)?;
    Ok(UserObject(
        repository(context)?
            .create_user(&email, now_epoch())
            .await?,
    ))
}

pub(crate) async fn delete_user(context: &Context<'_>, id: &ID) -> Result<bool> {
    require_operator(context)?;
    Ok(repository(context)?.delete_user(user_id(id)?).await?)
}

pub(crate) async fn set_library_permission(
    context: &Context<'_>,
    input: SetLibraryPermissionInput,
) -> Result<bool> {
    require_operator(context)?;
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
    require_operator(context)?;
    let section = SectionId::new(GlobalId::decode(library)?.key);
    Ok(repository(context)?
        .remove_permission(user_id(user)?, section)
        .await?)
}

pub(crate) async fn disable_extension(
    context: &Context<'_>,
    user: &ID,
    extension: ExtensionType,
) -> Result<bool> {
    require_operator(context)?;
    Ok(repository(context)?
        .disable_extension(user_id(user)?, extension.into(), now_epoch())
        .await?)
}

pub(crate) async fn connect_extension(
    context: &Context<'_>,
    user: &ID,
    extension: ExtensionType,
) -> Result<ExtensionCredentials> {
    require_operator(context)?;
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
    require_operator_or_self(context, user)?;
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
    require_operator_or_self(context, user)?;
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
    let Some(user) = caller(context).and_then(|caller| caller.user) else {
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
    require_operator_or_self(context, user)?;
    Ok(repository(context)?
        .unlink_account(user, extension.into())
        .await?)
}
