//! Who a request comes from, and what it may do.
//!
//! Authorization is a domain concern, so it lives here; how a credential reaches the server is
//! a protocol concern and lives in the bindings. Every caller is a user: there is no caller
//! without one, and what an owner may do beyond the rest is its server role.

use crate::access::{LibraryAccess, ServerRole};
use crate::ids::UserId;

/// An authenticated caller: always a user.
#[derive(Debug, Clone)]
pub struct Principal {
    pub user: UserId,
    /// The user's username, for logs.
    pub name: String,
    pub role: ServerRole,
    pub libraries: LibraryAccess,
    /// The identifier a caller's own view of the server goes by, when it has one.
    pub server_identifier: Option<String>,
}

impl Principal {
    /// Whether this caller manages the server: users, every library, everyone's sessions.
    #[must_use]
    pub fn is_owner(&self) -> bool {
        self.role == ServerRole::Owner
    }

    /// The name a session is stored under. Clients choose their own session identifiers, so
    /// without this one user could reach or replace another's by naming it.
    #[must_use]
    pub fn session_key(&self, client_key: &str) -> String {
        format!("{}{client_key}", Self::session_prefix(self.user))
    }

    /// The session identifier as this caller chose it.
    #[must_use]
    pub fn client_key<'a>(&self, stored: &'a str) -> &'a str {
        stored
            .strip_prefix(&Self::session_prefix(self.user))
            .unwrap_or(stored)
    }

    fn session_prefix(user: UserId) -> String {
        format!("u{user}~")
    }

    /// Whether this caller may reach something that belongs to `owner`: its own, or anyone's
    /// for an owner of the server.
    #[must_use]
    pub fn owns(&self, owner: UserId) -> bool {
        self.user == owner || self.is_owner()
    }
}

/// The server published at the address a request naming no account was sent to. It opens
/// nothing: it only says whose identifier to show, which is how a client matches the server it
/// is talking to with its entry on the account service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedServer(pub String);

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn caller(id: i64, role: ServerRole) -> Principal {
        Principal {
            user: UserId::new(id),
            name: format!("user{id}"),
            role,
            libraries: LibraryAccess::Only(HashMap::new()),
            server_identifier: None,
        }
    }

    #[test]
    fn two_users_choosing_the_same_session_do_not_share_it() {
        let one = caller(1, ServerRole::User);
        let two = caller(2, ServerRole::Owner);
        assert_ne!(one.session_key("abc"), two.session_key("abc"));
    }

    #[test]
    fn a_user_gets_back_the_identifier_it_chose() {
        let one = caller(1, ServerRole::User);
        assert_eq!(one.client_key(&one.session_key("abc")), "abc");
    }

    #[test]
    fn a_user_owns_its_own_and_an_owner_everyone_s() {
        let owner = UserId::new(1);
        assert!(caller(1, ServerRole::User).owns(owner));
        assert!(!caller(2, ServerRole::User).owns(owner));
        assert!(caller(2, ServerRole::Owner).owns(owner));
    }
}
