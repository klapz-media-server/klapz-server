//! Signing in with an e-mail address and a password, and the session that follows.
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

use crate::ids::UserId;

/// Shorter is guessable. Longer is refused rather than hashed: the cost of Argon2 grows with
/// its input, and a client controls it.
pub const PASSWORD_LENGTH: std::ops::RangeInclusive<usize> = 8..=256;

/// How long a session lasts, in seconds. It is not renewed: signing in again is the renewal.
pub const SESSION_LIFETIME_SECONDS: i64 = 30 * 24 * 3600;

/// Failed attempts on one address before it is refused for [`THROTTLE_WINDOW_SECONDS`].
pub const MAX_FAILED_SIGN_INS: u32 = 5;
pub const THROTTLE_WINDOW_SECONDS: i64 = 15 * 60;

const ISSUER: &str = "open-plex";

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

/// Counts failed sign-ins per address, to stop a password being guessed by trying them all.
#[derive(Debug, Default)]
pub struct SignInThrottle {
    failures: Mutex<HashMap<String, (u32, i64)>>,
}

impl SignInThrottle {
    /// Whether this address may try again at `now`.
    #[must_use]
    pub fn allows(&self, email: &str, now: i64) -> bool {
        let failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        failures.get(email).is_none_or(|&(count, since)| {
            count < MAX_FAILED_SIGN_INS || now - since >= THROTTLE_WINDOW_SECONDS
        })
    }

    pub fn record_failure(&self, email: &str, now: i64) {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        // Entries older than the window are dropped as others are written, so the table holds
        // only the addresses tried in the last fifteen minutes.
        failures.retain(|_, &mut (_, since)| now - since < THROTTLE_WINDOW_SECONDS);
        let entry = failures.entry(email.to_owned()).or_insert((0, now));
        entry.0 += 1;
    }

    pub fn clear(&self, email: &str) {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(email);
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::unwrap_used)]

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
