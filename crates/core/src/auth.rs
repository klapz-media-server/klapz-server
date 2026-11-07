//! Who a token belongs to, and what it is allowed to do.
//!
//! Authorization is a domain concern, so it lives here; how a token reaches the server is a
//! protocol concern and lives in the bindings.

use subtle::ConstantTimeEq;

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
