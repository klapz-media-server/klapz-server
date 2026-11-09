//! Users, what they may reach, and how an extension knows them.
//!
//! Our own model, not Plex's: a user is an e-mail address, a permission gives one user one role
//! on one library, and an extension is one protocol a user can connect through. Without a
//! permission on a library, a user does not know it exists.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::auth::{Principal, Scope};
use crate::ids::{SectionId, UserId};
use crate::page::{Page, Window};

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    pub email: String,
    /// Epoch seconds, as is the one below.
    pub created_at: i64,
    pub updated_at: i64,
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
        let libraries: HashMap<SectionId, LibraryRole> = self
            .permissions
            .iter()
            .map(|permission| (permission.section, permission.role))
            .collect();
        Principal {
            name: self.user.email,
            scope: Scope::User,
            libraries: LibraryAccess::Only(libraries),
            user: Some(self.user.id),
            server_identifier: Some(self.extension.external_id),
        }
    }
}

/// Which libraries a caller reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryAccess {
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
    /// # Errors
    ///
    /// [`AccessError::EmailTaken`], or a storage failure.
    async fn create_user(&self, email: &str, now: i64) -> Result<User, AccessError>;

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

    /// Switches a connection off. Whether there was one still on.
    async fn disable_extension(
        &self,
        user: UserId,
        extension: Extension,
        now: i64,
    ) -> Result<bool, AccessError>;

    async fn extensions(&self, user: UserId) -> Result<Vec<UserExtension>, AccessError>;

    /// What a token opens, if it is one of an extension's.
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
    /// on.
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
                email: "a@example.org".to_owned(),
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
        assert_eq!(principal.scope, Scope::User);
        assert_eq!(principal.server_identifier.as_deref(), Some("server"));
    }

    #[test]
    fn a_user_without_permissions_reaches_nothing() {
        let principal = grant(Vec::new()).into_principal();
        assert_eq!(principal.libraries.sections(), Some(Vec::new()));
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
