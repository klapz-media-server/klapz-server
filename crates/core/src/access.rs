//! Users, what they may reach, and how an extension knows them.
//!
//! Our own model, not Plex's: a user is a username, perhaps with an e-mail address; a permission
//! gives one user one role on one library, and an extension is one protocol a user can connect
//! through. Without a permission on a library, a user does not know it exists — unless it owns
//! the server, which reaches every library. A user that is not active — one that signed up and
//! waits for an owner — opens nothing, however it authenticates.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::auth::Principal;
use crate::ids::{SectionId, UserId};
use crate::page::{Page, Window};

/// The account every server has, created at startup: an owner that cannot be demoted,
/// deactivated or deleted, so that someone can always manage the server.
pub const ADMIN_USERNAME: &str = "admin";

/// What a user may do on the server as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ServerRole {
    /// Reaches the libraries it was given.
    #[default]
    User,
    /// Reaches every library as its admin, and manages users, sessions and everyone's history.
    Owner,
}

/// What a user may do in one library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LibraryRole {
    /// Browse and play.
    Viewer,
    /// Also change the library itself.
    Admin,
}

/// A protocol a user can connect through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Extension {
    Plex,
    Jellyfin,
}

impl Extension {
    /// Whether the protocol's clients sign in with a user's password, each device keeping a
    /// token of its own. Plex's are handed the connection's token instead.
    #[must_use]
    pub const fn signs_in_with_password(self) -> bool {
        matches!(self, Self::Jellyfin)
    }
}

/// A client a user signed in from through an extension, with a password: Jellyfin's clients
/// do, where Plex's are handed the connection's token. Each keeps a token of its own, so that
/// signing one out leaves the others signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionDevice {
    /// What the client calls itself, stable across its sign-ins: a device signing in again
    /// replaces its token.
    pub id: String,
    /// The device's name, for a person: `Chrome`, `Living room TV`.
    pub name: String,
    /// The application: `Jellyfin Web`, `Infuse`.
    pub client: String,
    pub client_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    /// How the user is known and signs in: lower case, unique.
    pub username: String,
    pub email: Option<String>,
    pub role: ServerRole,
    /// Off until an owner lets in someone who signed up; off again when it shuts one out.
    pub active: bool,
    /// Epoch seconds, as is the one below.
    pub created_at: i64,
    pub updated_at: i64,
}

impl User {
    /// Whether this is the account the server creates for itself, which always stays an
    /// active owner.
    #[must_use]
    pub fn is_built_in(&self) -> bool {
        self.username == ADMIN_USERNAME
    }
}

/// A user to create.
#[derive(Debug, Clone, Copy)]
pub struct NewUser<'a> {
    /// Already normalized: see [`crate::account::normalized_username`].
    pub username: &'a str,
    /// Already normalized: see [`crate::account::normalized_email`].
    pub email: Option<&'a str>,
    pub role: ServerRole,
    pub active: bool,
    /// An Argon2id hash, for a user that signs in with a password.
    pub password_hash: Option<&'a str>,
}

/// What a user signs in with: its username, or its e-mail address when it has one. Never
/// ambiguous, since a username holds no `@`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Login {
    Username(String),
    Email(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibraryPermission {
    pub user: UserId,
    pub section: SectionId,
    pub role: LibraryRole,
}

/// One user connected through one extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserExtension {
    pub user: UserId,
    pub extension: Extension,
    /// What the extension calls this user's side of the server: a Plex machine identifier.
    pub external_id: String,
    /// Switched off, its token opens nothing; the identifier is kept for when it comes back.
    pub enabled: bool,
}

/// An account on the service behind an extension: for Plex, a plex.tv account.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteAccount {
    /// How that service identifies the account. Its name can change, this cannot.
    pub id: String,
    pub name: String,
    pub token: String,
}

impl std::fmt::Debug for RemoteAccount {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteAccount")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// A user's connection tied to an account on the extension's own service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountLink {
    pub user: UserId,
    pub extension: Extension,
    pub server_id: String,
    pub enabled: bool,
    pub account: RemoteAccount,
    pub connection_uri: String,
    pub published_at: Option<i64>,
}

/// Everything a presented token gives access to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub user: User,
    pub extension: UserExtension,
    pub permissions: Vec<LibraryPermission>,
}

impl Grant {
    #[must_use]
    pub fn into_principal(self) -> Principal {
        Principal {
            server_identifier: Some(self.extension.external_id),
            ..user_principal(self.user, &self.permissions)
        }
    }
}

/// The caller a user is, with the libraries it was given, or all of them for an owner. No
/// server identifier: that is an extension's, and a user signed in with its password came
/// through none.
#[must_use]
pub fn user_principal(user: User, permissions: &[LibraryPermission]) -> Principal {
    let libraries = match user.role {
        ServerRole::Owner => LibraryAccess::All,
        ServerRole::User => LibraryAccess::Only(
            permissions
                .iter()
                .map(|permission| (permission.section, permission.role))
                .collect::<HashMap<SectionId, LibraryRole>>(),
        ),
    };
    Principal {
        user: user.id,
        name: user.username,
        role: user.role,
        libraries,
        server_identifier: None,
    }
}

/// Which libraries a caller reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryAccess {
    /// An owner's: every library, as its admin.
    All,
    Only(HashMap<SectionId, LibraryRole>),
}

impl LibraryAccess {
    /// `None` when the caller has no role there, which is also what hides the library.
    #[must_use]
    pub fn role(&self, section: SectionId) -> Option<LibraryRole> {
        match self {
            Self::All => Some(LibraryRole::Admin),
            Self::Only(roles) => roles.get(&section).copied(),
        }
    }

    /// Whether the caller may change what a library holds: its items' metadata, its settings.
    /// A library's admins may, and an owner, who administers every library.
    #[must_use]
    pub fn may_edit(&self, section: SectionId) -> bool {
        self.role(section) == Some(LibraryRole::Admin)
    }

    /// The sections a query may be narrowed to, or `None` for no restriction.
    #[must_use]
    pub fn sections(&self) -> Option<Vec<SectionId>> {
        match self {
            Self::All => None,
            Self::Only(roles) => Some(roles.keys().copied().collect()),
        }
    }
}

/// The token an extension hands a client. Long enough that guessing it is not a plan.
#[must_use]
pub fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Forty hexadecimal digits, the shape of a Plex machine identifier. Stable once stored: clients
/// recognize a server by it.
#[must_use]
pub fn new_external_id() -> String {
    let mut id = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    id.truncate(40);
    id
}

/// What is stored of a token, so that reading the database does not give anyone a login. A
/// plain digest is enough: unlike a password, a token is random and long.
#[must_use]
pub fn token_digest(token: &str) -> String {
    use std::fmt::Write as _;
    Sha256::digest(token.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// Whether this is an address clients can be sent to: a scheme, a host, perhaps a port, and
/// nothing a client appending `/library/sections` to it would trip over.
#[must_use]
pub fn is_connection_uri(uri: &str) -> bool {
    let Some(authority) = uri
        .strip_prefix("https://")
        .or_else(|| uri.strip_prefix("http://"))
    else {
        return false;
    };
    let (host, port) = authority
        .split_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    let host_is_valid = !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    let port_is_valid = match port {
        None => true,
        Some(port) => port.parse::<u16>().is_ok_and(|port| port > 0),
    };
    host_is_valid && port_is_valid
}

/// The connection addresses a request's `Host` could have been published as. A default port
/// may be written or left out on either side.
#[must_use]
pub fn connection_uris_for_host(host: &str) -> Vec<String> {
    let host = host.to_ascii_lowercase();
    let mut uris = vec![format!("http://{host}"), format!("https://{host}")];
    match host.rsplit_once(':') {
        None => {
            uris.push(format!("http://{host}:80"));
            uris.push(format!("https://{host}:443"));
        }
        Some((name, "80")) => uris.push(format!("http://{name}")),
        Some((name, "443")) => uris.push(format!("https://{name}")),
        Some(_) => {}
    }
    uris
}

/// Announces a link's address to the service and records that it was accepted. When it was,
/// in epoch seconds.
///
/// # Errors
///
/// What the service or the storage answered.
pub async fn publish_link(
    users: &dyn AccessRepository,
    directory: &dyn AccountDirectory,
    link: &AccountLink,
    now: i64,
) -> Result<i64, AccessError> {
    directory.publish(link).await?;
    users.mark_published(link.user, link.extension, now).await?;
    Ok(now)
}

#[derive(Debug, thiserror::Error)]
pub enum AccessError {
    #[error("this username is taken")]
    UsernameTaken,
    #[error("this e-mail address already has a user")]
    EmailTaken,
    #[error("no such user")]
    UnknownUser,
    #[error("this user is not connected through this extension")]
    NotConnected,
    #[error("this account is already paired with another user")]
    AccountTaken,
    #[error("the account service refused this request")]
    Refused,
    #[error("the account service could not be reached")]
    Unreachable(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("access storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait::async_trait]
pub trait AccessRepository: Send + Sync + std::fmt::Debug {
    /// A user, with its password when it has one.
    ///
    /// # Errors
    ///
    /// [`AccessError::UsernameTaken`], [`AccessError::EmailTaken`], or a storage failure.
    async fn create_user(&self, new: NewUser<'_>, now: i64) -> Result<User, AccessError>;

    /// The user a login names and its password hash, when it has one.
    async fn credentials(&self, login: &Login) -> Result<Option<(User, String)>, AccessError>;

    async fn user_named(&self, username: &str) -> Result<Option<User>, AccessError>;

    async fn has_password(&self, user: UserId) -> Result<bool, AccessError>;

    /// Gives a user a password, replacing the one it had.
    ///
    /// # Errors
    ///
    /// [`AccessError::UnknownUser`], or a storage failure.
    async fn set_password(
        &self,
        user: UserId,
        password_hash: &str,
        now: i64,
    ) -> Result<(), AccessError>;

    /// Makes a user an owner of the server, or no longer one. The user as it now is; `None`
    /// when there is none.
    async fn set_role(
        &self,
        user: UserId,
        role: ServerRole,
        now: i64,
    ) -> Result<Option<User>, AccessError>;

    /// Lets a user in or shuts it out. The user as it now is; `None` when there is none.
    async fn set_active(
        &self,
        user: UserId,
        active: bool,
        now: i64,
    ) -> Result<Option<User>, AccessError>;

    async fn users(&self, window: Window) -> Result<Page<User>, AccessError>;

    async fn user(&self, id: UserId) -> Result<Option<User>, AccessError>;

    /// Removes a user with its permissions and extensions. Whether there was one.
    async fn delete_user(&self, id: UserId) -> Result<bool, AccessError>;

    /// Gives a user this role on a library, replacing the one it had.
    ///
    /// # Errors
    ///
    /// [`AccessError::UnknownUser`], or a storage failure.
    async fn set_permission(
        &self,
        permission: LibraryPermission,
        now: i64,
    ) -> Result<(), AccessError>;

    /// Takes a library away from a user. Whether it had access.
    async fn remove_permission(
        &self,
        user: UserId,
        section: SectionId,
    ) -> Result<bool, AccessError>;

    async fn permissions(&self, user: UserId) -> Result<Vec<LibraryPermission>, AccessError>;

    /// Connects a user through an extension, or replaces its token if already connected, and
    /// switches it on: the external identifier stays, so clients keep recognizing the server.
    ///
    /// # Errors
    ///
    /// [`AccessError::UnknownUser`], or a storage failure.
    async fn connect_extension(
        &self,
        user: UserId,
        extension: Extension,
        external_id: &str,
        token_digest: &str,
        now: i64,
    ) -> Result<UserExtension, AccessError>;

    /// Switches a connection off and forgets its devices. Whether there was one still on.
    async fn disable_extension(
        &self,
        user: UserId,
        extension: Extension,
        now: i64,
    ) -> Result<bool, AccessError>;

    async fn extensions(&self, user: UserId) -> Result<Vec<UserExtension>, AccessError>;

    /// Gives a device its own token on a user's connection, replacing the one it had.
    ///
    /// # Errors
    ///
    /// [`AccessError::NotConnected`] when the connection is missing or switched off, or a
    /// storage failure.
    async fn issue_device_token(
        &self,
        user: UserId,
        extension: Extension,
        device: &ExtensionDevice,
        token_digest: &str,
        now: i64,
    ) -> Result<(), AccessError>;

    /// Forgets the device a token was issued to, when it is this user's. Whether there was one.
    async fn revoke_device_token(
        &self,
        user: UserId,
        extension: Extension,
        token_digest: &str,
    ) -> Result<bool, AccessError>;

    /// What a token opens, if it is one of an extension's — the connection's own or one of its
    /// devices' — and its user is active.
    async fn grant(
        &self,
        extension: Extension,
        token_digest: &str,
    ) -> Result<Option<Grant>, AccessError>;

    /// Ties a connection to an account, replacing the one it had.
    ///
    /// # Errors
    ///
    /// [`AccessError::NotConnected`], [`AccessError::AccountTaken`] when another user holds the
    /// account, or a storage failure.
    async fn link_account(
        &self,
        user: UserId,
        extension: Extension,
        account: &RemoteAccount,
        connection_uri: &str,
        now: i64,
    ) -> Result<AccountLink, AccessError>;

    async fn account_link(
        &self,
        user: UserId,
        extension: Extension,
    ) -> Result<Option<AccountLink>, AccessError>;

    /// The switched-on connection published at one of these addresses, if there is exactly
    /// one. Shared by several users, an address cannot say whose server a stranger reached.
    async fn connection_at(
        &self,
        extension: Extension,
        connection_uris: &[String],
    ) -> Result<Option<UserExtension>, AccessError>;

    /// Changes where a paired server is reached, and forgets when it was last published:
    /// the new address is not published until it is. `None` when the connection is not
    /// paired.
    ///
    /// # Errors
    ///
    /// A storage failure.
    async fn move_account(
        &self,
        user: UserId,
        extension: Extension,
        connection_uri: &str,
        now: i64,
    ) -> Result<Option<AccountLink>, AccessError>;

    /// Whether there was a link to forget.
    async fn unlink_account(&self, user: UserId, extension: Extension)
        -> Result<bool, AccessError>;

    async fn mark_published(
        &self,
        user: UserId,
        extension: Extension,
        at: i64,
    ) -> Result<(), AccessError>;

    /// What an account opens: the rights of the user it is paired with, if that connection is
    /// on and that user active.
    async fn grant_for_account(
        &self,
        extension: Extension,
        account_id: &str,
    ) -> Result<Option<Grant>, AccessError>;
}

/// The service behind an extension, which knows accounts and tells clients where servers are.
#[async_trait::async_trait]
pub trait AccountDirectory: Send + Sync + std::fmt::Debug {
    /// Spends a pairing code on this server, and returns the account it belongs to.
    ///
    /// # Errors
    ///
    /// [`AccessError::Refused`] for a code the service does not accept, or
    /// [`AccessError::Unreachable`].
    async fn redeem(&self, server_id: &str, code: &str) -> Result<RemoteAccount, AccessError>;

    /// Tells the service where a link's server can be reached.
    ///
    /// # Errors
    ///
    /// [`AccessError::Refused`] or [`AccessError::Unreachable`].
    async fn publish(&self, link: &AccountLink) -> Result<(), AccessError>;

    /// The account a token belongs to, or `None` when the service does not know it.
    ///
    /// # Errors
    ///
    /// [`AccessError::Unreachable`]: unknown is not the same as unanswered.
    async fn account_of(&self, token: &str) -> Result<Option<RemoteAccount>, AccessError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(permissions: Vec<LibraryPermission>) -> Grant {
        Grant {
            user: User {
                id: UserId::new(3),
                username: "alice".to_owned(),
                email: Some("a@example.org".to_owned()),
                role: ServerRole::User,
                active: true,
                created_at: 0,
                updated_at: 0,
            },
            extension: UserExtension {
                user: UserId::new(3),
                extension: Extension::Plex,
                external_id: "server".to_owned(),
                enabled: true,
            },
            permissions,
        }
    }

    #[test]
    fn a_user_reaches_only_the_libraries_it_was_given() {
        let principal = grant(vec![LibraryPermission {
            user: UserId::new(3),
            section: SectionId::new(4),
            role: LibraryRole::Viewer,
        }])
        .into_principal();
        assert_eq!(
            principal.libraries.role(SectionId::new(4)),
            Some(LibraryRole::Viewer)
        );
        assert_eq!(principal.libraries.role(SectionId::new(5)), None);
        assert!(!principal.is_owner());
        assert_eq!(principal.server_identifier.as_deref(), Some("server"));
    }

    #[test]
    fn only_jellyfin_clients_sign_in_with_a_password() {
        assert!(Extension::Jellyfin.signs_in_with_password());
        assert!(!Extension::Plex.signs_in_with_password());
    }

    #[test]
    fn a_user_without_permissions_reaches_nothing() {
        let principal = grant(Vec::new()).into_principal();
        assert_eq!(principal.libraries.sections(), Some(Vec::new()));
    }

    #[test]
    fn an_owner_reaches_every_library_whatever_it_was_given() {
        let mut owner = grant(Vec::new());
        owner.user.role = ServerRole::Owner;
        let principal = owner.into_principal();
        assert!(principal.is_owner());
        assert_eq!(principal.libraries, LibraryAccess::All);
    }

    #[test]
    fn the_owner_is_an_admin_of_every_library() {
        assert_eq!(
            LibraryAccess::All.role(SectionId::new(99)),
            Some(LibraryRole::Admin)
        );
        assert_eq!(LibraryAccess::All.sections(), None);
    }

    #[test]
    fn a_connection_address_is_a_scheme_a_host_and_perhaps_a_port() {
        for good in [
            "https://alice.plex.example.org:443",
            "https://alice.plex.example.org",
            "http://192.168.1.30:32400",
        ] {
            assert!(is_connection_uri(good), "{good}");
        }
        for bad in [
            "alice.plex.example.org",
            "ftp://example.org",
            "https://",
            "https://example.org/",
            "https://example.org:0",
            "https://example.org:port",
            "https://user@example.org",
            "https://example.org/?x=1",
        ] {
            assert!(!is_connection_uri(bad), "{bad}");
        }
    }

    #[test]
    fn a_host_matches_the_addresses_it_could_have_been_published_as() {
        assert_eq!(
            connection_uris_for_host("192.168.1.30:32500"),
            ["http://192.168.1.30:32500", "https://192.168.1.30:32500"]
        );
        let bare = connection_uris_for_host("A.example.org");
        assert!(bare.contains(&"https://a.example.org:443".to_owned()));
        assert!(bare.contains(&"https://a.example.org".to_owned()));
        assert!(connection_uris_for_host("a.example.org:443")
            .contains(&"https://a.example.org".to_owned()));
    }

    #[test]
    fn an_account_token_stays_out_of_debug_output() {
        let account = RemoteAccount {
            id: "7".to_owned(),
            name: "alice".to_owned(),
            token: "secret-token".to_owned(),
        };
        assert!(!format!("{account:?}").contains("secret-token"));
    }

    #[test]
    fn an_external_id_has_the_shape_of_a_machine_identifier() {
        let id = new_external_id();
        assert_eq!(id.len(), 40);
        assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(id, new_external_id());
    }

    #[test]
    fn a_digest_is_not_the_token_and_depends_on_all_of_it() {
        let token = new_token();
        assert_eq!(token.len(), 64);
        assert_ne!(token_digest(&token), token);
        assert_eq!(token_digest(&token).len(), 64);
        assert_ne!(token_digest("abc"), token_digest("abd"));
    }
}
