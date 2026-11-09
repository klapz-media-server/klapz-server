//! Who a token belongs to, and what it is allowed to do.
//!
//! Authorization is a domain concern, so it lives here; how a token reaches the server is a
//! protocol concern and lives in the bindings.

use subtle::ConstantTimeEq;

use crate::access::LibraryAccess;
use crate::ids::UserId;

/// What a caller is allowed to do.
///
/// Mirrors the two scopes the Plex spec declares. Deliberately not tied to a subscription:
/// open-plex has no paid tier, so role is the only authorization axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// Can read libraries shared with them.
    User,
    /// Can additionally change server configuration and destroy data.
    Admin,
}

impl Scope {
    /// Whether this scope satisfies a requirement.
    pub fn allows(self, required: Self) -> bool {
        self >= required
    }
}

/// An authenticated caller.
#[derive(Debug, Clone)]
pub struct Principal {
    pub name: String,
    pub scope: Scope,
    pub libraries: LibraryAccess,
    /// `None` for the server's own operator, who owns everything and is nobody's user.
    pub user: Option<UserId>,
    /// The identifier a caller's own view of the server goes by, when it has one.
    pub server_identifier: Option<String>,
}

impl Principal {
    /// The server's operator: every library, and every session.
    pub fn operator(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            scope: Scope::Admin,
            libraries: LibraryAccess::All,
            user: None,
            server_identifier: None,
        }
    }

    /// Someone who named no account, at an address published for one server. It reaches
    /// nothing: the address only says whose identifier to show, which is how a client matches
    /// the server it is talking to with its entry on the account service.
    #[must_use]
    pub fn visitor_of(server_identifier: String) -> Self {
        Self {
            name: "visitor".to_owned(),
            scope: Scope::User,
            libraries: LibraryAccess::Only(std::collections::HashMap::new()),
            user: None,
            server_identifier: Some(server_identifier),
        }
    }

    /// The name a session is stored under. Clients choose their own session identifiers, so
    /// without this one user could reach or replace another's by naming it.
    #[must_use]
    pub fn session_key(&self, client_key: &str) -> String {
        match self.user {
            Some(user) => format!("{}{client_key}", Self::session_prefix(user)),
            None => client_key.to_owned(),
        }
    }

    /// The session identifier as this caller chose it.
    #[must_use]
    pub fn client_key<'a>(&self, stored: &'a str) -> &'a str {
        self.user
            .and_then(|user| stored.strip_prefix(&Self::session_prefix(user)))
            .unwrap_or(stored)
    }

    fn session_prefix(user: UserId) -> String {
        format!("u{user}~")
    }

    /// Whether this caller may see something that belongs to `owner`.
    #[must_use]
    pub fn owns(&self, owner: Option<UserId>) -> bool {
        self.user.is_none() || self.user == owner
    }
}

/// A credential the server accepts.
#[derive(Debug, Clone)]
pub struct Credential {
    token: String,
    principal: Principal,
}

impl Credential {
    pub fn new(token: impl Into<String>, name: impl Into<String>, scope: Scope) -> Self {
        Self {
            token: token.into(),
            principal: Principal {
                name: name.into(),
                scope,
                libraries: LibraryAccess::All,
                user: None,
                server_identifier: None,
            },
        }
    }
}

/// The set of credentials the server accepts.
#[derive(Debug, Clone, Default)]
pub struct Accounts {
    credentials: Vec<Credential>,
}

impl Accounts {
    pub const fn new(credentials: Vec<Credential>) -> Self {
        Self { credentials }
    }

    pub fn is_empty(&self) -> bool {
        self.credentials.is_empty()
    }

    /// Resolves a presented token to its principal.
    ///
    /// The comparison is constant-time. A `==` on strings stops at the first differing byte,
    /// which leaks the length of the correct prefix and lets an attacker rebuild a token one
    /// byte at a time. Every candidate is checked so the work does not depend on which
    /// credential matches either.
    pub fn authenticate(&self, presented: &str) -> Option<&Principal> {
        let mut found = None;
        for credential in &self.credentials {
            let matches: bool = credential
                .token
                .as_bytes()
                .ct_eq(presented.as_bytes())
                .into();
            if matches && found.is_none() {
                found = Some(&credential.principal);
            }
        }
        found
    }
}

#[cfg(test)]
mod session_key_tests {
    use super::*;

    fn user(id: i64) -> Principal {
        Principal {
            user: Some(UserId::new(id)),
            ..Principal::operator("someone")
        }
    }

    #[test]
    fn two_users_choosing_the_same_session_do_not_share_it() {
        assert_ne!(user(1).session_key("abc"), user(2).session_key("abc"));
    }

    #[test]
    fn a_user_gets_back_the_identifier_it_chose() {
        let caller = user(1);
        assert_eq!(caller.client_key(&caller.session_key("abc")), "abc");
    }

    #[test]
    fn the_operator_keeps_identifiers_as_they_are() {
        let operator = Principal::operator("owner");
        assert_eq!(operator.session_key("abc"), "abc");
        assert_eq!(operator.client_key("u1~abc"), "u1~abc");
    }

    #[test]
    fn only_the_owner_and_the_operator_own_something() {
        let owner = Some(UserId::new(1));
        assert!(user(1).owns(owner));
        assert!(!user(2).owns(owner));
        assert!(!user(1).owns(None));
        assert!(Principal::operator("owner").owns(owner));
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn accounts() -> Accounts {
        Accounts::new(vec![
            Credential::new("owner-token", "owner", Scope::Admin),
            Credential::new("guest-token", "guest", Scope::User),
        ])
    }

    #[test]
    fn resolves_a_known_token_to_its_principal() {
        let accounts = accounts();
        let principal = accounts.authenticate("guest-token").expect("known token");
        assert_eq!(principal.name, "guest");
        assert_eq!(principal.scope, Scope::User);
    }

    #[test]
    fn rejects_an_unknown_token() {
        assert!(accounts().authenticate("nope").is_none());
    }

    #[test]
    fn rejects_a_prefix_of_a_valid_token() {
        // The case a non-constant-time comparison would make discoverable byte by byte.
        assert!(accounts().authenticate("owner-toke").is_none());
        assert!(accounts().authenticate("").is_none());
    }

    #[test]
    fn admin_satisfies_user_but_not_the_reverse() {
        assert!(Scope::Admin.allows(Scope::User));
        assert!(Scope::Admin.allows(Scope::Admin));
        assert!(Scope::User.allows(Scope::User));
        assert!(!Scope::User.allows(Scope::Admin));
    }
}
