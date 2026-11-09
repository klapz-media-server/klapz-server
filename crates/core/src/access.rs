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

#[derive(Debug, thiserror::Error)]
pub enum AccessError {
    #[error("this e-mail address already has a user")]
    EmailTaken,
    #[error("no such user")]
    UnknownUser,
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

    /// Connects a user through an extension, or replaces its token if already connected: the
    /// external identifier stays, so clients keep recognizing the server.
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

    async fn extensions(&self, user: UserId) -> Result<Vec<UserExtension>, AccessError>;

    /// What a token opens, if it is one of an extension's.
    async fn grant(
        &self,
        extension: Extension,
        token_digest: &str,
    ) -> Result<Option<Grant>, AccessError>;
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
