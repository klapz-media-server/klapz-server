//! Signing in with a username or an e-mail address and a password, the session that follows,
//! and the built-in `admin` account.
//!
//! A password is stored as an Argon2id hash. A session is a signed token naming the user and
//! when it expires; it is not stored, so what it opens is decided again on every request from
//! the user's current state — a user switched off loses its sessions at once.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};

use crate::access::{AccessError, AccessRepository, Login, NewUser, ServerRole, ADMIN_USERNAME};
use crate::ids::UserId;

/// Shorter is guessable. Longer is refused rather than hashed: the cost of Argon2 grows with
/// its input, and a client controls it.
pub const PASSWORD_LENGTH: std::ops::RangeInclusive<usize> = 8..=256;

/// Long enough to tell people apart, short enough to type and to fit a display.
pub const USERNAME_LENGTH: std::ops::RangeInclusive<usize> = 3..=32;

/// How long a session lasts, in seconds. It is not renewed: signing in again is the renewal.
pub const SESSION_LIFETIME_SECONDS: i64 = 30 * 24 * 3600;

/// Failed attempts on one identifier before it is refused for [`THROTTLE_WINDOW_SECONDS`].
pub const MAX_FAILED_SIGN_INS: u32 = 5;
pub const THROTTLE_WINDOW_SECONDS: i64 = 15 * 60;

const ISSUER: &str = "open-plex";

/// The username as it is stored and compared: trimmed and in lower case, so that two users
/// cannot differ by case alone. `None` unless it is 3 to 32 of `a-z`, `0-9`, `.`, `_` and `-`,
/// starting with a letter or a digit: nothing a URL, a log line or an e-mail address would
/// misread.
#[must_use]
pub fn normalized_username(username: &str) -> Option<String> {
    let username = username.trim().to_ascii_lowercase();
    let valid = USERNAME_LENGTH.contains(&username.len())
        && username
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && username.bytes().all(is_username_byte);
    valid.then_some(username)
}

const fn is_username_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
}

/// A valid username made from an e-mail address's local part, for a user that had none: what
/// the charset refuses becomes `-`, and what is left too short is completed. Not unique: the
/// caller adds a suffix when it is taken.
#[must_use]
pub fn username_from_email(email: &str) -> String {
    let local = email.split('@').next().unwrap_or_default();
    let mut username: String = local
        .to_ascii_lowercase()
        .bytes()
        .map(|byte| {
            if is_username_byte(byte) {
                char::from(byte)
            } else {
                '-'
            }
        })
        .collect();
    let leading = username.len() - username.trim_start_matches(['.', '_', '-']).len();
    username.drain(..leading);
    // Room left for a suffix such as `-12`.
    username.truncate(USERNAME_LENGTH.end() - 4);
    if username.len() < *USERNAME_LENGTH.start() {
        username = if username.is_empty() {
            "user".to_owned()
        } else {
            format!("{username}-user")
        };
    }
    username
}

/// What someone typed to sign in, read as a login: an address when it holds an `@`, a username
/// otherwise. `None` when it can be neither, which no account can match.
#[must_use]
pub fn login_of(identifier: &str) -> Option<Login> {
    if identifier.contains('@') {
        normalized_email(identifier).map(Login::Email)
    } else {
        normalized_username(identifier).map(Login::Username)
    }
}

/// The address as it is stored and compared: trimmed and in lower case. `None` when it is not
/// one at all.
#[must_use]
pub fn normalized_email(email: &str) -> Option<String> {
    let email = email.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let plausible = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && email.len() <= 254
        && !email.chars().any(char::is_whitespace);
    plausible.then_some(email)
}

#[must_use]
pub fn is_acceptable_password(password: &str) -> bool {
    PASSWORD_LENGTH.contains(&password.chars().count())
}

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("could not hash the password: {0}")]
    Hash(argon2::password_hash::Error),
}

/// A PHC string (`$argon2id$v=19$…`): the parameters and the salt travel with the hash, so they
/// can be raised later without invalidating stored passwords.
///
/// # Errors
///
/// What Argon2 reports, which with default parameters is nothing in practice.
pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(PasswordError::Hash)
}

/// Whether a password matches a stored hash. A hash that does not parse matches nothing.
#[must_use]
pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}

/// Spends the time a real verification takes, for an address with no password: answering
/// faster would tell a stranger which addresses have an account.
pub fn verify_against_nothing(password: &str) {
    static DECOY: OnceLock<Option<String>> = OnceLock::new();
    if let Some(hash) = DECOY.get_or_init(|| hash_password("not a password anyone has").ok()) {
        let _ = verify_password(password, hash);
    }
}

/// A password for the `admin` account when none is configured: 24 characters drawn by the
/// operating system from 64, 144 bits. 64 divides 256, so taking six bits of a byte is not
/// biased.
#[must_use]
pub fn generate_password() -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bytes = [0_u8; 24];
    OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte & 63)]))
        .collect()
}

/// What starting the server did to the `admin` account.
#[derive(PartialEq, Eq)]
pub enum AdminAccount {
    /// Created. With the password generated for it when none was configured, which is shown
    /// this once and kept nowhere.
    Created { generated_password: Option<String> },
    /// It existed, and took the configured password.
    PasswordReplaced,
    /// It existed, and no password was configured: its own stays.
    Kept,
}

impl std::fmt::Debug for AdminAccount {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created { generated_password } => formatter
                .debug_struct("Created")
                .field(
                    "generated_password",
                    &generated_password.as_ref().map(|_| ".."),
                )
                .finish(),
            Self::PasswordReplaced => formatter.write_str("PasswordReplaced"),
            Self::Kept => formatter.write_str("Kept"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdminAccountError {
    #[error(
        "the configured admin password must have between {} and {} characters",
        PASSWORD_LENGTH.start(),
        PASSWORD_LENGTH.end()
    )]
    WeakPassword,
    #[error(transparent)]
    Password(#[from] PasswordError),
    #[error(transparent)]
    Access(#[from] AccessError),
}

/// Makes sure the server has its `admin` account, an active owner, before it serves anyone.
///
/// `password` is the configured one: a new account takes it, or a generated one without it;
/// an existing one takes it in place of its own, and keeps its own without it. Either way the
/// account comes out active and an owner, whatever was done to the row by hand.
///
/// # Errors
///
/// [`AdminAccountError::WeakPassword`] for a configured password the policy refuses, checked
/// before anything is written; or what hashing and storage report.
pub async fn ensure_admin(
    users: &dyn AccessRepository,
    password: Option<&str>,
    now: i64,
) -> Result<AdminAccount, AdminAccountError> {
    if password.is_some_and(|password| !is_acceptable_password(password)) {
        return Err(AdminAccountError::WeakPassword);
    }
    let Some(admin) = users.user_named(ADMIN_USERNAME).await? else {
        let generated = password.is_none().then(generate_password);
        let chosen = password.or(generated.as_deref()).unwrap_or_default();
        let hash = hash_password(chosen)?;
        users
            .create_user(
                NewUser {
                    username: ADMIN_USERNAME,
                    email: None,
                    role: ServerRole::Owner,
                    active: true,
                    password_hash: Some(&hash),
                },
                now,
            )
            .await?;
        return Ok(AdminAccount::Created {
            generated_password: generated,
        });
    };
    if admin.role != ServerRole::Owner {
        users.set_role(admin.id, ServerRole::Owner, now).await?;
    }
    if !admin.active {
        users.set_active(admin.id, true, now).await?;
    }
    match password {
        Some(password) => {
            users
                .set_password(admin.id, &hash_password(password)?, now)
                .await?;
            Ok(AdminAccount::PasswordReplaced)
        }
        None => Ok(AdminAccount::Kept),
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Claims {
    sub: String,
    iss: String,
    iat: i64,
    exp: i64,
}

/// The secret sessions are signed with. Its bytes never reach `Debug` output.
pub struct SessionKey {
    encoding: EncodingKey,
    decoding: DecodingKey,
}

impl std::fmt::Debug for SessionKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("SessionKey").finish_non_exhaustive()
    }
}

impl SessionKey {
    /// Below 32 bytes an HMAC-SHA256 key is weaker than the hash it signs with.
    pub const MIN_SECRET_BYTES: usize = 32;

    /// `None` when the secret is too short to sign with.
    #[must_use]
    pub fn from_secret(secret: &[u8]) -> Option<Self> {
        (secret.len() >= Self::MIN_SECRET_BYTES).then(|| Self {
            encoding: EncodingKey::from_secret(secret),
            decoding: DecodingKey::from_secret(secret),
        })
    }

    /// A key of this process alone: sessions it signs end with it.
    #[must_use]
    pub fn generate() -> Self {
        let mut secret = [0_u8; 64];
        OsRng.fill_bytes(&mut secret);
        Self {
            encoding: EncodingKey::from_secret(&secret),
            decoding: DecodingKey::from_secret(&secret),
        }
    }

    /// A session for this user, valid [`SESSION_LIFETIME_SECONDS`] from `now`.
    ///
    /// # Errors
    ///
    /// What the signer reports, which for HMAC is nothing in practice.
    pub fn issue(&self, user: UserId, now: i64) -> Result<String, jsonwebtoken::errors::Error> {
        let claims = Claims {
            sub: user.get().to_string(),
            iss: ISSUER.to_owned(),
            iat: now,
            exp: now + SESSION_LIFETIME_SECONDS,
        };
        jsonwebtoken::encode(&Header::new(Algorithm::HS256), &claims, &self.encoding)
    }

    /// The user a session names, if this key signed it and it has not expired at `now`.
    #[must_use]
    pub fn verify(&self, token: &str, now: i64) -> Option<UserId> {
        let mut validation = Validation::new(Algorithm::HS256);
        // Expiry is checked against `now` below, so time stays a parameter.
        validation.validate_exp = false;
        validation.set_required_spec_claims(&["exp", "sub", "iss"]);
        validation.set_issuer(&[ISSUER]);
        let claims = jsonwebtoken::decode::<Claims>(token, &self.decoding, &validation)
            .ok()?
            .claims;
        if claims.exp <= now {
            return None;
        }
        claims.sub.parse().ok().map(UserId::new)
    }
}

/// Counts failed sign-ins per identifier — a username or an address, as typed once normalized —
/// to stop a password being guessed by trying them all.
#[derive(Debug, Default)]
pub struct SignInThrottle {
    failures: Mutex<HashMap<String, (u32, i64)>>,
}

impl SignInThrottle {
    /// Whether this identifier may try again at `now`.
    #[must_use]
    pub fn allows(&self, identifier: &str, now: i64) -> bool {
        let failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        failures.get(identifier).is_none_or(|&(count, since)| {
            count < MAX_FAILED_SIGN_INS || now - since >= THROTTLE_WINDOW_SECONDS
        })
    }

    pub fn record_failure(&self, identifier: &str, now: i64) {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        // Entries older than the window are dropped as others are written, so the table holds
        // only the identifiers tried in the last fifteen minutes.
        failures.retain(|_, &mut (_, since)| now - since < THROTTLE_WINDOW_SECONDS);
        let entry = failures.entry(identifier.to_owned()).or_insert((0, now));
        entry.0 += 1;
    }

    pub fn clear(&self, identifier: &str) {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(identifier);
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn an_address_is_trimmed_lowercased_and_must_look_like_one() {
        assert_eq!(
            normalized_email("  Alice@Example.org "),
            Some("alice@example.org".to_owned())
        );
        for bad in [
            "",
            "alice",
            "@example.org",
            "alice@",
            "alice@example",
            "a b@example.org",
            "alice@.org",
        ] {
            assert_eq!(normalized_email(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_username_is_three_to_32_safe_characters_in_lower_case() {
        assert_eq!(normalized_username(" Alice.B "), Some("alice.b".to_owned()));
        assert_eq!(normalized_username("a_1-2"), Some("a_1-2".to_owned()));
        assert_eq!(
            normalized_username(&"x".repeat(32)).map(|name| name.len()),
            Some(32)
        );
        for bad in [
            "",
            "ab",
            &"x".repeat(33),
            "-alice",
            ".alice",
            "al ice",
            "alice@example.org",
            "élodie",
            "a/b",
        ] {
            assert_eq!(normalized_username(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_login_is_an_address_with_an_at_sign_and_a_username_without() {
        assert_eq!(
            login_of(" Alice@Example.org"),
            Some(Login::Email("alice@example.org".to_owned()))
        );
        assert_eq!(login_of("Alice"), Some(Login::Username("alice".to_owned())));
        assert_eq!(login_of("al"), None);
        assert_eq!(login_of("alice@"), None);
    }

    #[test]
    fn an_address_gives_a_valid_username() {
        for (email, expected) in [
            ("alice@example.org", "alice"),
            ("Alice.Smith+plex@example.org", "alice.smith-plex"),
            ("__bob@example.org", "bob"),
            ("é@example.org", "user"),
            ("jo@example.org", "jo-user"),
            ("@example.org", "user"),
        ] {
            let username = username_from_email(email);
            assert_eq!(username, expected, "{email}");
            assert_eq!(normalized_username(&username).as_deref(), Some(expected));
        }
        let long = username_from_email(&format!("{}@example.org", "x".repeat(60)));
        assert_eq!(long.len(), 28, "room for a suffix");
    }

    #[test]
    fn a_generated_password_is_long_random_and_acceptable() {
        let password = generate_password();
        assert_eq!(password.len(), 24);
        assert!(is_acceptable_password(&password));
        assert_ne!(password, generate_password());
    }

    mod admin {
        use super::*;
        use crate::testing::InMemoryAccess;

        async fn admin_of(users: &InMemoryAccess) -> (crate::access::User, String) {
            users
                .credentials(&Login::Username(ADMIN_USERNAME.to_owned()))
                .await
                .unwrap()
                .unwrap()
        }

        #[tokio::test]
        async fn a_missing_admin_is_created_with_the_configured_password() {
            let users = InMemoryAccess::default();
            let outcome = ensure_admin(&users, Some("configured secret"), 10)
                .await
                .unwrap();
            assert_eq!(
                outcome,
                AdminAccount::Created {
                    generated_password: None
                }
            );
            let (admin, hash) = admin_of(&users).await;
            assert!(admin.active);
            assert_eq!(admin.role, ServerRole::Owner);
            assert_eq!(admin.email, None);
            assert!(verify_password("configured secret", &hash));
        }

        #[tokio::test]
        async fn a_missing_admin_without_a_configured_password_gets_a_generated_one() {
            let users = InMemoryAccess::default();
            let AdminAccount::Created {
                generated_password: Some(generated),
            } = ensure_admin(&users, None, 10).await.unwrap()
            else {
                panic!("expected a generated password");
            };
            let (admin, hash) = admin_of(&users).await;
            assert!(admin.active && admin.role == ServerRole::Owner);
            assert!(verify_password(&generated, &hash));
        }

        /// An admin that was demoted, shut out and given a password by hand.
        async fn tampered_admin() -> InMemoryAccess {
            let users = InMemoryAccess::default();
            let hash = hash_password("old password").unwrap();
            let admin = users
                .create_user(
                    NewUser {
                        username: ADMIN_USERNAME,
                        email: None,
                        role: ServerRole::User,
                        active: false,
                        password_hash: Some(&hash),
                    },
                    0,
                )
                .await
                .unwrap();
            assert!(!admin.active);
            users
        }

        #[tokio::test]
        async fn an_existing_admin_takes_the_configured_password_and_its_role_back() {
            let users = tampered_admin().await;
            let outcome = ensure_admin(&users, Some("new password"), 10)
                .await
                .unwrap();
            assert_eq!(outcome, AdminAccount::PasswordReplaced);
            let (admin, hash) = admin_of(&users).await;
            assert!(admin.active && admin.role == ServerRole::Owner);
            assert!(verify_password("new password", &hash));
            assert!(!verify_password("old password", &hash));
        }

        #[tokio::test]
        async fn an_existing_admin_keeps_its_password_when_none_is_configured() {
            let users = tampered_admin().await;
            assert_eq!(
                ensure_admin(&users, None, 10).await.unwrap(),
                AdminAccount::Kept
            );
            let (admin, hash) = admin_of(&users).await;
            assert!(admin.active && admin.role == ServerRole::Owner);
            assert!(verify_password("old password", &hash));
        }

        #[tokio::test]
        async fn a_weak_configured_password_is_refused_before_anything_is_written() {
            let users = InMemoryAccess::default();
            assert!(matches!(
                ensure_admin(&users, Some("short"), 10).await,
                Err(AdminAccountError::WeakPassword)
            ));
            assert_eq!(users.user_named(ADMIN_USERNAME).await.unwrap(), None);
        }

        #[test]
        fn a_generated_password_stays_out_of_debug_output() {
            let outcome = AdminAccount::Created {
                generated_password: Some("visible-secret".to_owned()),
            };
            assert!(!format!("{outcome:?}").contains("visible-secret"));
        }
    }

    #[test]
    fn a_password_is_between_eight_and_256_characters() {
        assert!(!is_acceptable_password("short"));
        assert!(is_acceptable_password("long enough"));
        assert!(is_acceptable_password("éééééééé"));
        assert!(!is_acceptable_password(&"x".repeat(257)));
    }

    #[test]
    fn a_hash_verifies_its_password_and_no_other() {
        let hash = hash_password("correct horse").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse", &hash));
        assert!(!verify_password("correct horsf", &hash));
        assert!(!verify_password("correct horse", "not a hash"));
        assert_ne!(hash, hash_password("correct horse").unwrap(), "salted");
    }

    #[test]
    fn a_session_names_its_user_until_it_expires() {
        let key = SessionKey::generate();
        let token = key.issue(UserId::new(7), 1_000).unwrap();
        assert_eq!(key.verify(&token, 1_001), Some(UserId::new(7)));
        assert_eq!(key.verify(&token, 1_000 + SESSION_LIFETIME_SECONDS), None);
    }

    #[test]
    fn a_session_signed_by_another_key_or_altered_opens_nothing() {
        let key = SessionKey::generate();
        let token = key.issue(UserId::new(7), 1_000).unwrap();
        assert_eq!(SessionKey::generate().verify(&token, 1_001), None);
        let mut altered = token.clone();
        altered.pop();
        altered.push(if token.ends_with('A') { 'B' } else { 'A' });
        assert_eq!(key.verify(&altered, 1_001), None);
        assert_eq!(key.verify("not.a.token", 1_001), None);
    }

    #[test]
    fn a_short_secret_is_refused() {
        assert!(SessionKey::from_secret(b"too short").is_none());
        assert!(SessionKey::from_secret(&[1; 32]).is_some());
    }

    #[test]
    fn an_address_is_refused_after_five_failures_for_fifteen_minutes() {
        let throttle = SignInThrottle::default();
        for _ in 0..MAX_FAILED_SIGN_INS {
            assert!(throttle.allows("a@example.org", 0));
            throttle.record_failure("a@example.org", 0);
        }
        assert!(!throttle.allows("a@example.org", 60));
        assert!(throttle.allows("b@example.org", 60));
        assert!(throttle.allows("a@example.org", THROTTLE_WINDOW_SECONDS));
        throttle.clear("a@example.org");
        assert!(throttle.allows("a@example.org", 60));
    }
}
