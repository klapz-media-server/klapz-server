//! Signing up, in and out with an e-mail address and a password.
//!
//! These are the operations a caller makes before it is anyone, so none of them requires a
//! caller. A session travels as a signed token in an `HttpOnly` cookie, set here and read by the
//! transport on every request; what a session opens is decided again each time from the user's
//! current state. Someone who signs up waits, inactive, until the operator lets them in.

use std::sync::Arc;

use async_graphql::{Context, Enum, InputObject, Result, SimpleObject, ID};
use open_plex_core::access::AccessError;
use open_plex_core::account::{
    hash_password, is_acceptable_password, normalized_email, verify_against_nothing,
    verify_password, SessionKey, SignInThrottle, PASSWORD_LENGTH, SESSION_LIFETIME_SECONDS,
};
use open_plex_core::now_epoch;

use crate::access::{require_operator, UserObject};
use crate::node::GlobalId;
use crate::schema::ApiContext;

/// The cookie a session travels in.
pub const SESSION_COOKIE: &str = "open_plex_session";

/// Argon2 runs at once at most this many times: each one holds a blocking thread and 19 MiB,
/// and sign-ins are open to anyone.
const CONCURRENT_HASHES: usize = 4;

/// What the server signs sessions with, and how it sets their cookie. Attached to each request
/// by the transport; without it, nobody can sign in.
#[derive(Debug)]
pub struct Sessions {
    key: Arc<SessionKey>,
    secure_cookie: bool,
    throttle: SignInThrottle,
    hashing: tokio::sync::Semaphore,
}

impl Sessions {
    /// `secure_cookie` marks the cookie for HTTPS only, which browsers also honor on
    /// `http://localhost`; off, a session also works over plain HTTP on a local network.
    #[must_use]
    pub fn new(key: Arc<SessionKey>, secure_cookie: bool) -> Self {
        Self {
            key,
            secure_cookie,
            throttle: SignInThrottle::default(),
            hashing: tokio::sync::Semaphore::new(CONCURRENT_HASHES),
        }
    }

    #[must_use]
    pub fn key(&self) -> &SessionKey {
        &self.key
    }

    fn cookie(&self, value: &str, max_age: i64) -> String {
        let secure = if self.secure_cookie { "; Secure" } else { "" };
        format!(
            "{SESSION_COOKIE}={value}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}"
        )
    }

    /// Runs Argon2 off the async workers, a bounded number at a time.
    async fn hash_work<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T> {
        let _permit = self.hashing.acquire().await?;
        Ok(tokio::task::spawn_blocking(work).await?)
    }
}

fn sessions<'c>(context: &Context<'c>) -> Result<&'c Sessions> {
    context
        .data_opt::<Arc<Sessions>>()
        .map(AsRef::as_ref)
        .ok_or_else(|| async_graphql::Error::new("signing in is not available on this server"))
}

/// Why an account operation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum AccountErrorCode {
    /// Not an e-mail address.
    InvalidEmail,
    /// Shorter than 8 characters, or longer than 256.
    WeakPassword,
    /// The address already has a user.
    EmailTaken,
    /// No user with this address and this password. Worded the same for both, so a stranger
    /// cannot learn which addresses have an account.
    InvalidCredentials,
    /// The password is right, but the operator has not let this user in yet, or shut it out.
    Inactive,
    /// Too many failed attempts on this address: wait fifteen minutes.
    TooManyAttempts,
}

/// A problem with what was asked, as opposed to a failure of the server.
#[derive(Debug, Clone, SimpleObject)]
pub struct AccountUserError {
    /// The input field at fault, as a path from the input's root.
    pub field: Vec<String>,
    /// What went wrong, for a person.
    pub message: String,
    /// What went wrong, for a program.
    pub code: AccountErrorCode,
}

impl AccountUserError {
    fn new(field: &str, code: AccountErrorCode) -> Self {
        let message = match code {
            AccountErrorCode::InvalidEmail => "this is not an e-mail address".to_owned(),
            AccountErrorCode::WeakPassword => format!(
                "a password has between {} and {} characters",
                PASSWORD_LENGTH.start(),
                PASSWORD_LENGTH.end()
            ),
            AccountErrorCode::EmailTaken => "this address already has an account".to_owned(),
            AccountErrorCode::InvalidCredentials => "wrong address or password".to_owned(),
            AccountErrorCode::Inactive => "this account waits for the operator".to_owned(),
            AccountErrorCode::TooManyAttempts => "too many attempts: try again later".to_owned(),
        };
        Self {
            field: vec!["input".to_owned(), field.to_owned()],
            message,
            code,
        }
    }
}

/// An account to create.
#[derive(Debug, InputObject)]
pub struct SignUpInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The address to sign in with. Compared without regard to case.
    pub email: String,
    /// Between 8 and 256 characters.
    #[graphql(secret)]
    pub password: String,
}

/// What `signUp` did.
#[derive(Debug, SimpleObject)]
pub struct SignUpPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// Whether the account was created. It waits for the operator to let it in.
    pub created: bool,
    /// Why it was not. Empty when it was.
    pub user_errors: Vec<AccountUserError>,
}

/// A user signing in.
#[derive(Debug, InputObject)]
pub struct SignInInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The address the account was created with.
    pub email: String,
    /// Its password.
    #[graphql(secret)]
    pub password: String,
}

/// What `signIn` did. On success the session is in a cookie, out of reach of scripts: there is
/// no token here to keep.
#[derive(Debug, SimpleObject)]
pub struct SignInPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// Who signed in. Null when nobody did.
    pub user: Option<UserObject>,
    /// Why nobody did. Empty when someone did.
    pub user_errors: Vec<AccountUserError>,
}

pub(crate) async fn sign_up(context: &Context<'_>, input: SignUpInput) -> Result<SignUpPayload> {
    let refuse = |field: &str, code| {
        Ok(SignUpPayload {
            client_mutation_id: input.client_mutation_id.clone(),
            created: false,
            user_errors: vec![AccountUserError::new(field, code)],
        })
    };
    let Some(email) = normalized_email(&input.email) else {
        return refuse("email", AccountErrorCode::InvalidEmail);
    };
    if !is_acceptable_password(&input.password) {
        return refuse("password", AccountErrorCode::WeakPassword);
    }
    let sessions = sessions(context)?;
    let password = input.password.clone();
    let hash = sessions
        .hash_work(move || hash_password(&password))
        .await??;
    let api = context.data::<ApiContext>()?;
    match api.access.sign_up(&email, &hash, now_epoch()).await {
        Ok(_) => Ok(SignUpPayload {
            client_mutation_id: input.client_mutation_id,
            created: true,
            user_errors: Vec::new(),
        }),
        Err(AccessError::EmailTaken) => refuse("email", AccountErrorCode::EmailTaken),
        Err(error) => Err(error.into()),
    }
}

pub(crate) async fn sign_in(context: &Context<'_>, input: SignInInput) -> Result<SignInPayload> {
    let refuse = |field: &str, code| {
        Ok(SignInPayload {
            client_mutation_id: input.client_mutation_id.clone(),
            user: None,
            user_errors: vec![AccountUserError::new(field, code)],
        })
    };
    let sessions = sessions(context)?;
    let now = now_epoch();
    // Not an address cannot have an account: the same answer as a wrong password.
    let email = normalized_email(&input.email).unwrap_or_default();
    if !sessions.throttle.allows(&email, now) {
        return refuse("email", AccountErrorCode::TooManyAttempts);
    }
    let api = context.data::<ApiContext>()?;
    let credentials = api.access.credentials(&email).await?;
    let password = input.password.clone();
    let (user, matches) = if let Some((user, hash)) = credentials {
        let matches = sessions
            .hash_work(move || verify_password(&password, &hash))
            .await?;
        (Some(user), matches)
    } else {
        sessions
            .hash_work(move || verify_against_nothing(&password))
            .await?;
        (None, false)
    };
    let Some(user) = user.filter(|_| matches) else {
        sessions.throttle.record_failure(&email, now);
        return refuse("password", AccountErrorCode::InvalidCredentials);
    };
    sessions.throttle.clear(&email);
    if !user.active {
        return refuse("email", AccountErrorCode::Inactive);
    }
    let token = sessions.key.issue(user.id, now)?;
    context.append_http_header(
        "Set-Cookie",
        sessions.cookie(&token, SESSION_LIFETIME_SECONDS),
    );
    Ok(SignInPayload {
        client_mutation_id: input.client_mutation_id,
        user: Some(UserObject(user)),
        user_errors: Vec::new(),
    })
}

/// Clears the session cookie. The token itself stays valid until it expires, as nothing stores
/// it: what ends it early is shutting the user out.
pub(crate) fn sign_out(context: &Context<'_>) -> Result<bool> {
    let sessions = sessions(context)?;
    context.append_http_header("Set-Cookie", sessions.cookie("", 0));
    Ok(true)
}

pub(crate) async fn set_active(
    context: &Context<'_>,
    user: &ID,
    active: bool,
) -> Result<UserObject> {
    require_operator(context)?;
    let id = open_plex_core::UserId::new(GlobalId::decode(user)?.key);
    context
        .data::<ApiContext>()?
        .access
        .set_active(id, active, now_epoch())
        .await?
        .map(UserObject)
        .ok_or_else(crate::access::denied)
}
