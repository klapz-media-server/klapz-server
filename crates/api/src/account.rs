//! Signing up, in and out with a username or an e-mail address and a password.
//!
//! These are the operations a caller makes before it is anyone, so none of them requires a
//! caller. A session is a signed token, set here in an `HttpOnly` cookie and read by the
//! transport on every request; a client that is no browser reads it from that cookie and sends
//! it back as `Authorization: Bearer`. What a session opens is decided again each time from the
//! user's current state. Someone who signs up waits, inactive, until an owner lets them in.

use std::sync::Arc;

use async_graphql::{Context, Enum, InputObject, Result, SimpleObject, ID};
use klapz_core::access::{
    new_token, token_digest, AccessError, Extension, ExtensionDevice, Login, NewUser, ServerRole,
    User,
};
use klapz_core::account::{
    hash_password, is_acceptable_password, login_of, normalized_email, normalized_username,
    verify_against_nothing, verify_password, SessionKey, SignInThrottle, PASSWORD_LENGTH,
    SESSION_LIFETIME_SECONDS, USERNAME_LENGTH,
};
use klapz_core::{now_epoch, UserId};

use crate::access::{require_caller, ExtensionType, UserObject, USER_TYPE};
use crate::node::GlobalId;
use crate::schema::ApiContext;

/// The cookie a session travels in.
pub const SESSION_COOKIE: &str = "klapz_session";

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
    /// Not 3 to 32 letters, digits, `.`, `_` or `-` starting with a letter or a digit.
    InvalidUsername,
    /// Another user has this username, whatever its case.
    UsernameTaken,
    /// Not an e-mail address.
    InvalidEmail,
    /// Shorter than 8 characters, or longer than 256.
    WeakPassword,
    /// The address already has a user.
    EmailTaken,
    /// No user with this username or address and this password. Worded the same for both, so
    /// a stranger cannot learn which accounts exist.
    InvalidCredentials,
    /// The password is right, but no owner has let this user in yet, or one shut it out.
    Inactive,
    /// Too many failed attempts on this username or address: wait fifteen minutes.
    TooManyAttempts,
    /// No such user.
    UserNotFound,
    /// The built-in `admin` account stays an active owner: it cannot be demoted, deactivated
    /// or deleted.
    BuiltInAccount,
    /// The password is right, but the user is not connected through this extension, or its
    /// connection is off.
    NotConnected,
    /// The extension's clients do not sign in with a password: Plex's are handed a token.
    NoPasswordSignIn,
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
    pub(crate) fn new(field: &str, code: AccountErrorCode) -> Self {
        let message = match code {
            AccountErrorCode::InvalidUsername => format!(
                "a username has {} to {} letters, digits, dots, dashes or underscores, \
                 and starts with a letter or a digit",
                USERNAME_LENGTH.start(),
                USERNAME_LENGTH.end()
            ),
            AccountErrorCode::UsernameTaken => "this username is taken".to_owned(),
            AccountErrorCode::InvalidEmail => "this is not an e-mail address".to_owned(),
            AccountErrorCode::WeakPassword => format!(
                "a password has between {} and {} characters",
                PASSWORD_LENGTH.start(),
                PASSWORD_LENGTH.end()
            ),
            AccountErrorCode::EmailTaken => "this address already has an account".to_owned(),
            AccountErrorCode::InvalidCredentials => {
                "wrong username, address or password".to_owned()
            }
            AccountErrorCode::Inactive => "this account waits for an owner".to_owned(),
            AccountErrorCode::TooManyAttempts => "too many attempts: try again later".to_owned(),
            AccountErrorCode::UserNotFound => "no such user".to_owned(),
            AccountErrorCode::BuiltInAccount => {
                "the admin account stays an active owner".to_owned()
            }
            AccountErrorCode::NotConnected => {
                "this account may not sign in through this extension".to_owned()
            }
            AccountErrorCode::NoPasswordSignIn => {
                "this extension's clients do not sign in with a password".to_owned()
            }
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
    /// The name to sign in with: 3 to 32 letters, digits, `.`, `_` or `-`, starting with a
    /// letter or a digit. Stored in lower case.
    pub username: String,
    /// An address to sign in with as well. Compared without regard to case.
    pub email: Option<String>,
    /// Between 8 and 256 characters.
    #[graphql(secret)]
    pub password: String,
}

/// What `signUp` did.
#[derive(Debug, SimpleObject)]
pub struct SignUpPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// Whether the account was created. It waits for an owner to let it in.
    pub created: bool,
    /// Why it was not. Empty when it was.
    pub user_errors: Vec<AccountUserError>,
}

/// A user signing in.
#[derive(Debug, InputObject)]
pub struct SignInInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The account's username, or its e-mail address.
    pub login: String,
    /// Its password.
    #[graphql(secret)]
    pub password: String,
}

/// What `signIn` did. On success the session is in the `klapz_session` cookie, out of reach
/// of a page's scripts: there is no token here to keep. A client that is no browser reads the
/// cookie's value from `Set-Cookie` and presents it as `Authorization: Bearer <value>`.
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
    let Some(username) = normalized_username(&input.username) else {
        return refuse("username", AccountErrorCode::InvalidUsername);
    };
    let email = match input.email.as_deref().map(normalized_email) {
        None => None,
        Some(Some(email)) => Some(email),
        Some(None) => return refuse("email", AccountErrorCode::InvalidEmail),
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
    let created = api
        .access
        .create_user(
            NewUser {
                username: &username,
                email: email.as_deref(),
                role: ServerRole::User,
                active: false,
                password_hash: Some(&hash),
            },
            now_epoch(),
        )
        .await;
    match created {
        Ok(_) => Ok(SignUpPayload {
            client_mutation_id: input.client_mutation_id,
            created: true,
            user_errors: Vec::new(),
        }),
        Err(AccessError::UsernameTaken) => refuse("username", AccountErrorCode::UsernameTaken),
        Err(AccessError::EmailTaken) => refuse("email", AccountErrorCode::EmailTaken),
        Err(error) => Err(error.into()),
    }
}

/// What a login's failures are counted under: the identifier as normalized, so that `Alice`
/// and `alice` share their attempts. What can be no login shares one count.
fn throttle_key(login: Option<&Login>) -> &str {
    match login {
        Some(Login::Username(name) | Login::Email(name)) => name,
        None => "",
    }
}

/// The active user a login and a password name, or why there is none. Failures count against
/// the login, and a login with no account costs as much time as a wrong password.
async fn authenticate(
    context: &Context<'_>,
    sessions: &Sessions,
    login: &str,
    password: &str,
) -> Result<std::result::Result<User, AccountUserError>> {
    let now = now_epoch();
    // What can be no login cannot have an account: the same answer as a wrong password.
    let login = login_of(login);
    let key = throttle_key(login.as_ref());
    if !sessions.throttle.allows(key, now) {
        return Ok(Err(AccountUserError::new(
            "login",
            AccountErrorCode::TooManyAttempts,
        )));
    }
    let api = context.data::<ApiContext>()?;
    let credentials = match &login {
        Some(login) => api.access.credentials(login).await?,
        None => None,
    };
    let password = password.to_owned();
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
        sessions.throttle.record_failure(key, now);
        return Ok(Err(AccountUserError::new(
            "password",
            AccountErrorCode::InvalidCredentials,
        )));
    };
    sessions.throttle.clear(key);
    if !user.active {
        return Ok(Err(AccountUserError::new(
            "login",
            AccountErrorCode::Inactive,
        )));
    }
    Ok(Ok(user))
}

pub(crate) async fn sign_in(context: &Context<'_>, input: SignInInput) -> Result<SignInPayload> {
    let sessions = sessions(context)?;
    let user = match authenticate(context, sessions, &input.login, &input.password).await? {
        Ok(user) => user,
        Err(error) => {
            return Ok(SignInPayload {
                client_mutation_id: input.client_mutation_id,
                user: None,
                user_errors: vec![error],
            })
        }
    };
    let token = sessions.key.issue(user.id, now_epoch())?;
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

/// A client signing a user in through an extension whose clients sign in with a password.
#[derive(Debug, InputObject)]
pub struct ExtensionSignInInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The protocol the client speaks. The user must be connected through it.
    pub extension: ExtensionType,
    /// The account's username, or its e-mail address.
    pub login: String,
    /// Its password.
    #[graphql(secret)]
    pub password: String,
    /// What the client calls the device, stable across its sign-ins: signing in again from it
    /// replaces its token.
    pub device_id: String,
    /// The device's name, for a person.
    pub device_name: String,
    /// The application signing in.
    pub client: String,
    /// The application's version.
    pub client_version: String,
}

/// What `signInThroughExtension` did.
#[derive(Debug, SimpleObject)]
pub struct ExtensionSignInPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// Who signed in. Null when nobody did.
    pub user: Option<UserObject>,
    /// The token the device presents from now on, shown only here: the server keeps a digest
    /// of it. Null when nobody signed in.
    pub token: Option<String>,
    /// Why nobody did. Empty when someone did.
    pub user_errors: Vec<AccountUserError>,
}

pub(crate) async fn sign_in_through_extension(
    context: &Context<'_>,
    input: ExtensionSignInInput,
) -> Result<ExtensionSignInPayload> {
    let refuse = |error| {
        Ok(ExtensionSignInPayload {
            client_mutation_id: input.client_mutation_id.clone(),
            user: None,
            token: None,
            user_errors: vec![error],
        })
    };
    let extension: Extension = input.extension.into();
    if !extension.signs_in_with_password() {
        return refuse(AccountUserError::new(
            "extension",
            AccountErrorCode::NoPasswordSignIn,
        ));
    }
    let sessions = sessions(context)?;
    let user = match authenticate(context, sessions, &input.login, &input.password).await? {
        Ok(user) => user,
        Err(error) => return refuse(error),
    };
    let token = new_token();
    let device = ExtensionDevice {
        id: input.device_id.clone(),
        name: input.device_name.clone(),
        client: input.client.clone(),
        client_version: input.client_version.clone(),
    };
    let api = context.data::<ApiContext>()?;
    let issued = api
        .access
        .issue_device_token(
            user.id,
            extension,
            &device,
            &token_digest(&token),
            now_epoch(),
        )
        .await;
    match issued {
        Ok(()) => Ok(ExtensionSignInPayload {
            client_mutation_id: input.client_mutation_id,
            user: Some(UserObject(user)),
            token: Some(token),
            user_errors: Vec::new(),
        }),
        Err(AccessError::NotConnected) => refuse(AccountUserError::new(
            "extension",
            AccountErrorCode::NotConnected,
        )),
        Err(error) => Err(error.into()),
    }
}

/// Forgets the device a token was issued to, so that it opens nothing more. Only the
/// caller's own devices.
pub(crate) async fn sign_out_of_extension(
    context: &Context<'_>,
    extension: ExtensionType,
    token: &str,
) -> Result<bool> {
    let caller = require_caller(context)?;
    let api = context.data::<ApiContext>()?;
    Ok(api
        .access
        .revoke_device_token(caller.user, extension.into(), &token_digest(token))
        .await?)
}

/// A password for the caller, or for a user an owner names.
#[derive(Debug, InputObject)]
pub struct SetPasswordInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// Whose password, as returned in `User.id`: another user's for an owner only, who needs
    /// no current password to set or reset one. Left out, the caller's own.
    pub user_id: Option<ID>,
    /// The password the caller has now, to change its own. Required when it has one.
    #[graphql(secret)]
    pub current_password: Option<String>,
    /// The new password: between 8 and 256 characters.
    #[graphql(secret)]
    pub new_password: String,
}

/// What `setPassword` did.
#[derive(Debug, SimpleObject)]
pub struct SetPasswordPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// The caller, with its new password. Null when it was refused.
    pub user: Option<UserObject>,
    /// Why it was refused. Empty when it was not.
    pub user_errors: Vec<AccountUserError>,
}

pub(crate) async fn set_password(
    context: &Context<'_>,
    input: SetPasswordInput,
) -> Result<SetPasswordPayload> {
    let caller = require_caller(context)?;
    let refuse = |field: &str, code| {
        Ok(SetPasswordPayload {
            client_mutation_id: input.client_mutation_id.clone(),
            user: None,
            user_errors: vec![AccountUserError::new(field, code)],
        })
    };
    let api = context.data::<ApiContext>()?;
    let target = match &input.user_id {
        None => caller.user,
        Some(id) => {
            let named = GlobalId::decode(id)
                .ok()
                .filter(|decoded| decoded.type_name == USER_TYPE)
                .map(|decoded| UserId::new(decoded.key));
            match named {
                Some(user) if user == caller.user => user,
                // Whether another user exists is the owners' to know.
                Some(user) if caller.is_owner() => {
                    if api.access.user(user).await?.is_none() {
                        return refuse("userId", AccountErrorCode::UserNotFound);
                    }
                    user
                }
                _ if caller.is_owner() => return refuse("userId", AccountErrorCode::UserNotFound),
                _ => return Err(crate::access::denied()),
            }
        }
    };
    if !is_acceptable_password(&input.new_password) {
        return refuse("newPassword", AccountErrorCode::WeakPassword);
    }
    let sessions = sessions(context)?;
    // The current password guards a session left open on someone else's screen, and its
    // failures count with sign-ins', or this would be a way around their limit. An owner
    // setting another's needs none: it manages the accounts.
    if target == caller.user && api.access.has_password(caller.user).await? {
        let now = now_epoch();
        if !sessions.throttle.allows(&caller.name, now) {
            return refuse("currentPassword", AccountErrorCode::TooManyAttempts);
        }
        let credentials = api
            .access
            .credentials(&Login::Username(caller.name.clone()))
            .await?;
        let current = input.current_password.clone().unwrap_or_default();
        let matches = match credentials {
            Some((user, hash)) if user.id == caller.user => {
                sessions
                    .hash_work(move || verify_password(&current, &hash))
                    .await?
            }
            _ => false,
        };
        if !matches {
            sessions.throttle.record_failure(&caller.name, now);
            return refuse("currentPassword", AccountErrorCode::InvalidCredentials);
        }
        sessions.throttle.clear(&caller.name);
    }
    let password = input.new_password.clone();
    let hash = sessions
        .hash_work(move || hash_password(&password))
        .await??;
    api.access.set_password(target, &hash, now_epoch()).await?;
    Ok(SetPasswordPayload {
        client_mutation_id: input.client_mutation_id,
        user: api.access.user(target).await?.map(UserObject),
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
