//! Stored users, permissions and extensions, against a real database.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::access::{
    token_digest, AccessError, AccessRepository, Extension, LibraryPermission, LibraryRole,
};
use open_plex_core::page::Window;
use open_plex_core::{SectionId, UserId};
use open_plex_db::{connect, SqlAccessRepository};

const NOW: i64 = 1_700_000_000;

async fn repository() -> SqlAccessRepository {
    SqlAccessRepository::new(connect("sqlite::memory:").await.expect("database"))
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
    let user = repository.create_user("a@example.org", NOW).await.unwrap();
    assert_eq!(user.email, "a@example.org");
    assert_eq!((user.created_at, user.updated_at), (NOW, NOW));
    assert_eq!(repository.user(user.id).await.unwrap(), Some(user.clone()));
    let page = repository
        .users(Window::new(None, None, Some(10), None).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items, [user]);
}

#[tokio::test]
async fn an_address_has_one_user() {
    let repository = repository().await;
    repository.create_user("a@example.org", NOW).await.unwrap();
    assert!(matches!(
        repository.create_user("a@example.org", NOW).await,
        Err(AccessError::EmailTaken)
    ));
}

#[tokio::test]
async fn a_permission_is_per_library_and_can_change_or_go() {
    let repository = repository().await;
    let user = repository
        .create_user("a@example.org", NOW)
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

#[tokio::test]
async fn a_token_opens_its_users_access_and_nothing_else() {
    let repository = repository().await;
    let first = repository
        .create_user("a@example.org", NOW)
        .await
        .unwrap()
        .id;
    let second = repository
        .create_user("b@example.org", NOW)
        .await
        .unwrap()
        .id;
    repository
        .set_permission(permission(first, 4, LibraryRole::Viewer), NOW)
        .await
        .unwrap();
    repository
        .connect_extension(first, Extension::Plex, "server-a", &token_digest("a"), NOW)
        .await
        .unwrap();
    repository
        .connect_extension(second, Extension::Plex, "server-b", &token_digest("b"), NOW)
        .await
        .unwrap();

    let grant = repository
        .grant(Extension::Plex, &token_digest("a"))
        .await
        .unwrap()
        .expect("a grant");
    assert_eq!(grant.user.id, first);
    assert_eq!(grant.extension.external_id, "server-a");
    assert_eq!(grant.permissions.len(), 1);

    assert!(repository
        .grant(Extension::Plex, &token_digest("wrong"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn reconnecting_replaces_the_token_and_keeps_the_identifier() {
    let repository = repository().await;
    let user = repository
        .create_user("a@example.org", NOW)
        .await
        .unwrap()
        .id;
    repository
        .connect_extension(user, Extension::Plex, "first", &token_digest("old"), NOW)
        .await
        .unwrap();
    let again = repository
        .connect_extension(user, Extension::Plex, "second", &token_digest("new"), NOW)
        .await
        .unwrap();
    assert_eq!(again.external_id, "first");
    assert!(repository
        .grant(Extension::Plex, &token_digest("old"))
        .await
        .unwrap()
        .is_none());
    assert!(repository
        .grant(Extension::Plex, &token_digest("new"))
        .await
        .unwrap()
        .is_some());
    assert_eq!(repository.extensions(user).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_disabled_connection_opens_nothing_and_comes_back_with_its_identifier() {
    let repository = repository().await;
    let user = repository
        .create_user("a@example.org", NOW)
        .await
        .unwrap()
        .id;
    let digest = token_digest("old");
    repository
        .connect_extension(user, Extension::Plex, "first", &digest, NOW)
        .await
        .unwrap();

    assert!(repository
        .disable_extension(user, Extension::Plex, NOW)
        .await
        .unwrap());
    assert!(!repository
        .disable_extension(user, Extension::Plex, NOW)
        .await
        .unwrap());
    assert!(repository
        .grant(Extension::Plex, &digest)
        .await
        .unwrap()
        .is_none());
    assert!(!repository.extensions(user).await.unwrap()[0].enabled);

    let back = repository
        .connect_extension(user, Extension::Plex, "second", &token_digest("new"), NOW)
        .await
        .unwrap();
    assert_eq!(back.external_id, "first");
    assert!(back.enabled);
    assert!(repository
        .grant(Extension::Plex, &digest)
        .await
        .unwrap()
        .is_none());
    assert!(repository
        .grant(Extension::Plex, &token_digest("new"))
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn deleting_a_user_closes_its_access() {
    let repository = repository().await;
    let user = repository
        .create_user("a@example.org", NOW)
        .await
        .unwrap()
        .id;
    repository
        .set_permission(permission(user, 4, LibraryRole::Viewer), NOW)
        .await
        .unwrap();
    repository
        .connect_extension(user, Extension::Plex, "server", &token_digest("a"), NOW)
        .await
        .unwrap();
    assert!(repository.delete_user(user).await.unwrap());
    assert!(!repository.delete_user(user).await.unwrap());
    assert!(repository.permissions(user).await.unwrap().is_empty());
    assert!(repository
        .grant(Extension::Plex, &token_digest("a"))
        .await
        .unwrap()
        .is_none());
}
