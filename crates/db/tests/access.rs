//! Stored users, permissions and extensions, against a real database.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use klapz_core::access::{
    token_digest, AccessError, AccessRepository, Extension, ExtensionDevice, LibraryPermission,
    LibraryRole, Login, NewUser, RemoteAccount, ServerRole,
};
use klapz_core::page::Window;
use klapz_core::{SectionId, UserId};
use klapz_db::{connect, SqlAccessRepository};

const NOW: i64 = 1_700_000_000;

async fn repository() -> SqlAccessRepository {
    SqlAccessRepository::new(connect("sqlite::memory:").await.expect("database"))
}

/// An active user with no password, named after its address, as an owner creates one.
fn active(email: &'static str) -> NewUser<'static> {
    NewUser {
        username: email.split('@').next().unwrap(),
        email: Some(email),
        role: ServerRole::User,
        active: true,
        password_hash: None,
    }
}

/// A user that signed up: inactive, with its password.
fn signed_up(email: &'static str, password_hash: &'static str) -> NewUser<'static> {
    NewUser {
        active: false,
        password_hash: Some(password_hash),
        ..active(email)
    }
}

fn by_email(email: &str) -> Login {
    Login::Email(email.to_owned())
}

fn permission(user: UserId, section: i64, role: LibraryRole) -> LibraryPermission {
    LibraryPermission {
        user,
        section: SectionId::new(section),
        role,
    }
}

#[tokio::test]
async fn a_user_is_stored_with_its_dates() {
    let repository = repository().await;
    let user = repository
        .create_user(active("a@example.org"), NOW)
        .await
        .unwrap();
    assert_eq!(user.username, "a");
    assert_eq!(user.email.as_deref(), Some("a@example.org"));
    assert_eq!(user.role, ServerRole::User);
    assert_eq!((user.created_at, user.updated_at), (NOW, NOW));
    assert_eq!(repository.user(user.id).await.unwrap(), Some(user.clone()));
    let page = repository
        .users(Window::new(None, None, Some(10), None).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items, [user]);
}

fn account(id: &str) -> RemoteAccount {
    RemoteAccount {
        id: id.to_owned(),
        name: format!("name of {id}"),
        token: format!("token of {id}"),
    }
}

async fn connected(repository: &SqlAccessRepository, email: &'static str) -> UserId {
    let user = repository.create_user(active(email), NOW).await.unwrap().id;
    repository
        .connect_extension(user, Extension::Plex, email, NOW)
        .await
        .unwrap();
    user
}

#[tokio::test]
async fn a_paired_account_is_stored_and_opens_what_its_user_may_reach() {
    let repository = repository().await;
    let user = connected(&repository, "a@example.org").await;
    repository
        .set_permission(permission(user, 4, LibraryRole::Viewer), NOW)
        .await
        .unwrap();
    let link = repository
        .link_account(
            user,
            Extension::Plex,
            &account("7"),
            "https://a.example.org:443",
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(link.server_id, "a@example.org");
    assert_eq!(link.published_at, None);

    repository
        .mark_published(user, Extension::Plex, NOW + 5)
        .await
        .unwrap();
    let stored = repository
        .account_link(user, Extension::Plex)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.published_at, Some(NOW + 5));
    assert_eq!(stored.account, account("7"));

    let grant = repository
        .grant_for_account(Extension::Plex, "7")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grant.user.id, user);
    assert_eq!(grant.permissions.len(), 1);
    assert!(repository
        .grant_for_account(Extension::Plex, "8")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn pairing_needs_a_connection_and_a_free_account() {
    let repository = repository().await;
    let bare = repository
        .create_user(active("bare@example.org"), NOW)
        .await
        .unwrap()
        .id;
    assert!(matches!(
        repository
            .link_account(bare, Extension::Plex, &account("7"), "https://a", NOW)
            .await,
        Err(AccessError::NotConnected)
    ));

    let first = connected(&repository, "a@example.org").await;
    let second = connected(&repository, "b@example.org").await;
    repository
        .link_account(first, Extension::Plex, &account("7"), "https://a", NOW)
        .await
        .unwrap();
    assert!(matches!(
        repository
            .link_account(second, Extension::Plex, &account("7"), "https://b", NOW)
            .await,
        Err(AccessError::AccountTaken)
    ));
}

#[tokio::test]
async fn pairing_again_replaces_the_account_and_forgets_the_publication() {
    let repository = repository().await;
    let user = connected(&repository, "a@example.org").await;
    repository
        .link_account(user, Extension::Plex, &account("7"), "https://a", NOW)
        .await
        .unwrap();
    repository
        .mark_published(user, Extension::Plex, NOW)
        .await
        .unwrap();
    repository
        .link_account(user, Extension::Plex, &account("8"), "https://b", NOW)
        .await
        .unwrap();
    let stored = repository
        .account_link(user, Extension::Plex)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.account.id, "8");
    assert_eq!(stored.published_at, None);
    assert!(repository
        .grant_for_account(Extension::Plex, "7")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_switched_off_connection_opens_nothing_through_its_account() {
    let repository = repository().await;
    let user = connected(&repository, "a@example.org").await;
    repository
        .link_account(user, Extension::Plex, &account("7"), "https://a", NOW)
        .await
        .unwrap();
    repository
        .disable_extension(user, Extension::Plex, NOW)
        .await
        .unwrap();
    assert!(repository
        .grant_for_account(Extension::Plex, "7")
        .await
        .unwrap()
        .is_none());
    assert!(
        !repository
            .account_link(user, Extension::Plex)
            .await
            .unwrap()
            .unwrap()
            .enabled
    );
}

#[tokio::test]
async fn a_published_address_names_its_switched_on_connection() {
    let repository = repository().await;
    let user = connected(&repository, "a@example.org").await;
    repository
        .link_account(user, Extension::Plex, &account("7"), "https://a", NOW)
        .await
        .unwrap();
    let at = |uri: &str| vec![uri.to_owned(), "http://elsewhere".to_owned()];
    assert_eq!(
        repository
            .connection_at(Extension::Plex, &at("https://a"))
            .await
            .unwrap()
            .unwrap()
            .user,
        user
    );
    assert!(repository
        .connection_at(Extension::Plex, &at("https://b"))
        .await
        .unwrap()
        .is_none());

    repository
        .disable_extension(user, Extension::Plex, NOW)
        .await
        .unwrap();
    assert!(repository
        .connection_at(Extension::Plex, &at("https://a"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn an_address_several_users_share_names_nobody() {
    let repository = repository().await;
    let first = connected(&repository, "a@example.org").await;
    let second = connected(&repository, "b@example.org").await;
    for (user, id) in [(first, "7"), (second, "8")] {
        repository
            .link_account(user, Extension::Plex, &account(id), "https://shared", NOW)
            .await
            .unwrap();
    }
    let shared = ["https://shared".to_owned()];
    assert!(repository
        .connection_at(Extension::Plex, &shared)
        .await
        .unwrap()
        .is_none());

    // With one of them switched off, the address is the other's alone again.
    repository
        .disable_extension(second, Extension::Plex, NOW)
        .await
        .unwrap();
    assert_eq!(
        repository
            .connection_at(Extension::Plex, &shared)
            .await
            .unwrap()
            .unwrap()
            .user,
        first
    );
}

#[tokio::test]
async fn unpairing_or_deleting_the_user_forgets_the_account() {
    let repository = repository().await;
    let user = connected(&repository, "a@example.org").await;
    repository
        .link_account(user, Extension::Plex, &account("7"), "https://a", NOW)
        .await
        .unwrap();
    assert!(repository
        .unlink_account(user, Extension::Plex)
        .await
        .unwrap());
    assert!(!repository
        .unlink_account(user, Extension::Plex)
        .await
        .unwrap());

    repository
        .link_account(user, Extension::Plex, &account("7"), "https://a", NOW)
        .await
        .unwrap();
    repository.delete_user(user).await.unwrap();
    assert!(repository
        .grant_for_account(Extension::Plex, "7")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn an_address_and_a_username_have_one_user_each() {
    let repository = repository().await;
    repository
        .create_user(active("a@example.org"), NOW)
        .await
        .unwrap();
    assert!(matches!(
        repository
            .create_user(
                NewUser {
                    username: "other",
                    ..active("a@example.org")
                },
                NOW
            )
            .await,
        Err(AccessError::EmailTaken)
    ));
    assert!(matches!(
        repository.create_user(active("a@other.example"), NOW).await,
        Err(AccessError::UsernameTaken)
    ));
}

#[tokio::test]
async fn users_without_an_address_do_not_collide() {
    let repository = repository().await;
    for username in ["admin", "bob"] {
        let user = repository
            .create_user(
                NewUser {
                    username,
                    email: None,
                    role: ServerRole::Owner,
                    active: true,
                    password_hash: Some("$argon2id$hash"),
                },
                NOW,
            )
            .await
            .unwrap();
        assert_eq!(user.email, None);
        assert_eq!(user.role, ServerRole::Owner);
    }
    let (admin, hash) = repository
        .credentials(&Login::Username("admin".to_owned()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (admin.username.as_str(), hash.as_str()),
        ("admin", "$argon2id$hash")
    );
    assert_eq!(
        repository
            .user_named("bob")
            .await
            .unwrap()
            .unwrap()
            .username,
        "bob"
    );
    assert_eq!(repository.user_named("carol").await.unwrap(), None);
}

#[tokio::test]
async fn a_role_and_a_password_can_change() {
    let repository = repository().await;
    let user = repository
        .create_user(active("a@example.org"), NOW)
        .await
        .unwrap();
    let promoted = repository
        .set_role(user.id, ServerRole::Owner, NOW + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (promoted.role, promoted.updated_at),
        (ServerRole::Owner, NOW + 1)
    );
    for hash in ["$argon2id$first", "$argon2id$second"] {
        repository.set_password(user.id, hash, NOW).await.unwrap();
        let (_, stored) = repository
            .credentials(&by_email("a@example.org"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored, hash);
    }
    assert!(matches!(
        repository
            .set_password(UserId::new(999), "$argon2id$x", NOW)
            .await,
        Err(AccessError::UnknownUser)
    ));
    assert!(repository
        .set_role(UserId::new(999), ServerRole::Owner, NOW)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_permission_is_per_library_and_can_change_or_go() {
    let repository = repository().await;
    let user = repository
        .create_user(active("a@example.org"), NOW)
        .await
        .unwrap()
        .id;
    repository
        .set_permission(permission(user, 4, LibraryRole::Viewer), NOW)
        .await
        .unwrap();
    repository
        .set_permission(permission(user, 5, LibraryRole::Viewer), NOW)
        .await
        .unwrap();
    repository
        .set_permission(permission(user, 4, LibraryRole::Admin), NOW + 1)
        .await
        .unwrap();
    assert_eq!(
        repository.permissions(user).await.unwrap(),
        [
            permission(user, 4, LibraryRole::Admin),
            permission(user, 5, LibraryRole::Viewer),
        ]
    );
    assert!(repository
        .remove_permission(user, SectionId::new(5))
        .await
        .unwrap());
    assert!(!repository
        .remove_permission(user, SectionId::new(5))
        .await
        .unwrap());
    assert_eq!(repository.permissions(user).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_permission_needs_a_user() {
    let repository = repository().await;
    assert!(matches!(
        repository
            .set_permission(permission(UserId::new(9), 4, LibraryRole::Viewer), NOW)
            .await,
        Err(AccessError::UnknownUser)
    ));
}

/// A user connected through Plex and paired with the account `account_id`.
async fn paired(repository: &SqlAccessRepository, email: &'static str, account_id: &str) -> UserId {
    let user = connected(repository, email).await;
    repository
        .link_account(
            user,
            Extension::Plex,
            &account(account_id),
            "https://a.example.org:443",
            NOW,
        )
        .await
        .unwrap();
    user
}

async fn opens_through_account(
    repository: &SqlAccessRepository,
    account_id: &str,
) -> Option<UserId> {
    repository
        .grant_for_account(Extension::Plex, account_id)
        .await
        .unwrap()
        .map(|grant| grant.user.id)
}

#[tokio::test]
async fn reconnecting_keeps_the_identifier() {
    let repository = repository().await;
    let user = repository
        .create_user(active("a@example.org"), NOW)
        .await
        .unwrap()
        .id;
    repository
        .connect_extension(user, Extension::Plex, "first", NOW)
        .await
        .unwrap();
    let again = repository
        .connect_extension(user, Extension::Plex, "second", NOW)
        .await
        .unwrap();
    assert_eq!(again.external_id, "first");
    assert_eq!(repository.extensions(user).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_disabled_connection_opens_nothing_and_comes_back_with_its_identifier() {
    let repository = repository().await;
    let user = paired(&repository, "a@example.org", "7").await;

    assert!(repository
        .disable_extension(user, Extension::Plex, NOW)
        .await
        .unwrap());
    assert!(!repository
        .disable_extension(user, Extension::Plex, NOW)
        .await
        .unwrap());
    assert_eq!(opens_through_account(&repository, "7").await, None);
    assert!(!repository.extensions(user).await.unwrap()[0].enabled);

    let back = repository
        .connect_extension(user, Extension::Plex, "second", NOW)
        .await
        .unwrap();
    assert_eq!(back.external_id, "a@example.org");
    assert!(back.enabled);
    assert_eq!(opens_through_account(&repository, "7").await, Some(user));
}

#[tokio::test]
async fn deleting_a_user_closes_its_access() {
    let repository = repository().await;
    let user = paired(&repository, "a@example.org", "7").await;
    repository
        .set_permission(permission(user, 4, LibraryRole::Viewer), NOW)
        .await
        .unwrap();
    assert!(repository.delete_user(user).await.unwrap());
    assert!(!repository.delete_user(user).await.unwrap());
    assert!(repository.permissions(user).await.unwrap().is_empty());
    assert_eq!(opens_through_account(&repository, "7").await, None);
}

#[tokio::test]
async fn a_user_who_signs_up_waits_inactive_with_its_password() {
    let repository = repository().await;
    let user = repository
        .create_user(signed_up("b@example.org", "$argon2id$hash"), NOW)
        .await
        .unwrap();
    assert!(!user.active);
    assert!(repository.has_password(user.id).await.unwrap());
    let (found, hash) = repository
        .credentials(&by_email("b@example.org"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((found.id, hash.as_str()), (user.id, "$argon2id$hash"));

    let created = repository
        .create_user(active("c@example.org"), NOW)
        .await
        .unwrap();
    assert!(created.active, "a user an owner creates is let in");
    assert!(!repository.has_password(created.id).await.unwrap());
    assert!(repository
        .credentials(&by_email("c@example.org"))
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        repository
            .create_user(
                NewUser {
                    username: "someone-else",
                    ..signed_up("c@example.org", "$argon2id$x")
                },
                NOW
            )
            .await,
        Err(AccessError::EmailTaken)
    ));
}

#[tokio::test]
async fn an_inactive_user_opens_nothing_until_it_is_let_in() {
    let repository = repository().await;
    let user = paired(&repository, "a@example.org", "7").await;
    let deactivated = repository
        .set_active(user, false, NOW + 1)
        .await
        .unwrap()
        .unwrap();
    assert!(!deactivated.active);
    assert_eq!(deactivated.updated_at, NOW + 1);
    assert_eq!(opens_through_account(&repository, "7").await, None);

    repository.set_active(user, true, NOW + 2).await.unwrap();
    assert_eq!(opens_through_account(&repository, "7").await, Some(user));
    assert!(repository
        .set_active(UserId::new(999), true, NOW)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn deleting_a_user_forgets_its_password() {
    let repository = repository().await;
    let user = repository
        .create_user(signed_up("b@example.org", "$argon2id$hash"), NOW)
        .await
        .unwrap();
    assert!(repository.delete_user(user.id).await.unwrap());
    assert!(!repository.has_password(user.id).await.unwrap());
    let again = repository
        .create_user(signed_up("b@example.org", "$argon2id$other"), NOW)
        .await
        .unwrap();
    assert_eq!(
        repository
            .credentials(&by_email("b@example.org"))
            .await
            .unwrap()
            .unwrap()
            .1,
        "$argon2id$other"
    );
    assert_ne!(again.id, user.id);
}

fn device(id: &str) -> ExtensionDevice {
    ExtensionDevice {
        id: id.to_owned(),
        name: "Chrome".to_owned(),
        client: "Jellyfin Web".to_owned(),
        client_version: "10.11.0".to_owned(),
    }
}

async fn on_jellyfin(repository: &SqlAccessRepository, email: &'static str) -> UserId {
    let user = repository.create_user(active(email), NOW).await.unwrap().id;
    repository
        .connect_extension(user, Extension::Jellyfin, email, NOW)
        .await
        .unwrap();
    user
}

async fn opener(
    repository: &SqlAccessRepository,
    extension: Extension,
    token: &str,
) -> Option<UserId> {
    repository
        .grant(extension, &token_digest(token))
        .await
        .unwrap()
        .map(|grant| grant.user.id)
}

#[tokio::test]
async fn a_jellyfin_connection_is_stored_beside_a_plex_one() {
    let repository = repository().await;
    let user = connected(&repository, "a@example.org").await;
    repository
        .connect_extension(user, Extension::Jellyfin, "jf", NOW)
        .await
        .unwrap();
    let mut extensions: Vec<_> = repository
        .extensions(user)
        .await
        .unwrap()
        .into_iter()
        .map(|connection| {
            (
                connection.extension == Extension::Jellyfin,
                connection.external_id,
            )
        })
        .collect();
    extensions.sort();
    assert_eq!(
        extensions,
        [(false, "a@example.org".to_owned()), (true, "jf".to_owned())]
    );
}

#[tokio::test]
async fn a_device_token_opens_its_users_access_through_its_extension_only() {
    let repository = repository().await;
    let user = on_jellyfin(&repository, "a@example.org").await;
    repository
        .set_permission(permission(user, 4, LibraryRole::Viewer), NOW)
        .await
        .unwrap();
    repository
        .issue_device_token(
            user,
            Extension::Jellyfin,
            &device("d1"),
            &token_digest("t1"),
            NOW,
        )
        .await
        .unwrap();
    let grant = repository
        .grant(Extension::Jellyfin, &token_digest("t1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grant.user.id, user);
    assert_eq!(grant.extension.extension, Extension::Jellyfin);
    assert_eq!(
        grant.permissions,
        [permission(user, 4, LibraryRole::Viewer)]
    );
    assert_eq!(opener(&repository, Extension::Plex, "t1").await, None);
}

#[tokio::test]
async fn a_device_token_needs_a_switched_on_connection() {
    let repository = repository().await;
    let bare = repository
        .create_user(active("a@example.org"), NOW)
        .await
        .unwrap()
        .id;
    let plex_only = connected(&repository, "b@example.org").await;
    for user in [bare, plex_only] {
        assert!(matches!(
            repository
                .issue_device_token(
                    user,
                    Extension::Jellyfin,
                    &device("d1"),
                    &token_digest("t"),
                    NOW
                )
                .await,
            Err(AccessError::NotConnected)
        ));
    }
    let user = on_jellyfin(&repository, "c@example.org").await;
    repository
        .disable_extension(user, Extension::Jellyfin, NOW)
        .await
        .unwrap();
    assert!(matches!(
        repository
            .issue_device_token(
                user,
                Extension::Jellyfin,
                &device("d1"),
                &token_digest("t"),
                NOW
            )
            .await,
        Err(AccessError::NotConnected)
    ));
}

#[tokio::test]
async fn a_device_signing_in_again_replaces_its_token_and_keeps_the_others() {
    let repository = repository().await;
    let user = on_jellyfin(&repository, "a@example.org").await;
    for (id, token) in [("d1", "first"), ("d2", "other"), ("d1", "second")] {
        repository
            .issue_device_token(
                user,
                Extension::Jellyfin,
                &device(id),
                &token_digest(token),
                NOW,
            )
            .await
            .unwrap();
    }
    assert_eq!(
        opener(&repository, Extension::Jellyfin, "first").await,
        None
    );
    assert_eq!(
        opener(&repository, Extension::Jellyfin, "second").await,
        Some(user)
    );
    assert_eq!(
        opener(&repository, Extension::Jellyfin, "other").await,
        Some(user)
    );
}

#[tokio::test]
async fn revoking_a_device_token_takes_only_the_users_own() {
    let repository = repository().await;
    let alice = on_jellyfin(&repository, "a@example.org").await;
    let bob = on_jellyfin(&repository, "b@example.org").await;
    repository
        .issue_device_token(
            alice,
            Extension::Jellyfin,
            &device("d1"),
            &token_digest("t"),
            NOW,
        )
        .await
        .unwrap();
    assert!(!repository
        .revoke_device_token(bob, Extension::Jellyfin, &token_digest("t"))
        .await
        .unwrap());
    assert!(!repository
        .revoke_device_token(alice, Extension::Plex, &token_digest("t"))
        .await
        .unwrap());
    assert_eq!(
        opener(&repository, Extension::Jellyfin, "t").await,
        Some(alice)
    );
    assert!(repository
        .revoke_device_token(alice, Extension::Jellyfin, &token_digest("t"))
        .await
        .unwrap());
    assert_eq!(opener(&repository, Extension::Jellyfin, "t").await, None);
}

#[tokio::test]
async fn switching_a_connection_off_or_deleting_its_user_forgets_its_devices() {
    let repository = repository().await;
    let alice = on_jellyfin(&repository, "a@example.org").await;
    repository
        .issue_device_token(
            alice,
            Extension::Jellyfin,
            &device("d1"),
            &token_digest("t"),
            NOW,
        )
        .await
        .unwrap();
    repository
        .disable_extension(alice, Extension::Jellyfin, NOW)
        .await
        .unwrap();
    repository
        .connect_extension(alice, Extension::Jellyfin, "again", NOW)
        .await
        .unwrap();
    assert_eq!(opener(&repository, Extension::Jellyfin, "t").await, None);

    let bob = on_jellyfin(&repository, "b@example.org").await;
    repository
        .issue_device_token(
            bob,
            Extension::Jellyfin,
            &device("d1"),
            &token_digest("u"),
            NOW,
        )
        .await
        .unwrap();
    assert!(repository.delete_user(bob).await.unwrap());
    assert_eq!(opener(&repository, Extension::Jellyfin, "u").await, None);
}

#[tokio::test]
async fn an_inactive_user_opens_nothing_through_a_device() {
    let repository = repository().await;
    let user = on_jellyfin(&repository, "a@example.org").await;
    repository
        .issue_device_token(
            user,
            Extension::Jellyfin,
            &device("d1"),
            &token_digest("t"),
            NOW,
        )
        .await
        .unwrap();
    repository.set_active(user, false, NOW).await.unwrap();
    assert_eq!(opener(&repository, Extension::Jellyfin, "t").await, None);
}

#[tokio::test]
async fn devices_are_listed_latest_sign_in_first_and_forgotten_one_by_one() {
    let repository = repository().await;
    let user = on_jellyfin(&repository, "d@example.org").await;
    for (id, at) in [("d1", NOW), ("d2", NOW + 10)] {
        repository
            .issue_device_token(
                user,
                Extension::Jellyfin,
                &device(id),
                &token_digest(id),
                at,
            )
            .await
            .unwrap();
    }
    let listed = repository.devices(user, Extension::Jellyfin).await.unwrap();
    let order: Vec<(&str, i64)> = listed
        .iter()
        .map(|signed_in| (signed_in.device.id.as_str(), signed_in.signed_in_at))
        .collect();
    assert_eq!(order, [("d2", NOW + 10), ("d1", NOW)]);
    assert!(repository
        .devices(user, Extension::Plex)
        .await
        .unwrap()
        .is_empty());

    assert!(repository
        .forget_device(user, Extension::Jellyfin, "d1")
        .await
        .unwrap());
    assert!(!repository
        .forget_device(user, Extension::Jellyfin, "d1")
        .await
        .unwrap());
    assert!(repository
        .grant(Extension::Jellyfin, &token_digest("d1"))
        .await
        .unwrap()
        .is_none());
    assert!(repository
        .grant(Extension::Jellyfin, &token_digest("d2"))
        .await
        .unwrap()
        .is_some());
}
