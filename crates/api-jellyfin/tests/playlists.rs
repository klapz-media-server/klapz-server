//! Playlists: made, filled, reordered and emptied as Jellyfin's clients do, against what the
//! reference answered for a playlist made for the capture.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use axum::http::{Method, StatusCode};
use common::{fixture, jellyfin_id, shape_differences, Harness, DOODLEBUG, EPISODE};
use serde_json::{json, Value};

/// What a playlist's entries carry that this server does not hold: artwork, and what a
/// provider told the reference about the items.
const NOT_HELD: &[&str] = &[
    "Primary",
    "Backdrop",
    "Thumb",
    "Logo",
    "PrimaryImageAspectRatio",
    "PremiereDate",
    "ProductionYear",
    "OfficialRating",
    "CommunityRating",
    "CriticRating",
    "HasSubtitles",
    "OriginalLanguage",
    "Etag",
];

async fn created(harness: &Harness, token: &str) -> String {
    let answer = harness
        .post(
            "/jellyfin/Playlists",
            token,
            json!({
                "Name": "Soirée",
                "Ids": [jellyfin_id("Item", DOODLEBUG)],
                "MediaType": "Video",
            }),
        )
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    let body = answer.json();
    assert_eq!(
        shape_differences(&body, &fixture("playlist_create"), &[]),
        Vec::<String>::new()
    );
    body["Id"].as_str().unwrap().to_owned()
}

async fn with_episode(harness: &Harness, token: &str) -> String {
    let playlist = created(harness, token).await;
    let added = harness
        .bare(
            Method::POST,
            &format!(
                "/jellyfin/Playlists/{playlist}/Items?ids={}",
                jellyfin_id("Item", EPISODE)
            ),
            token,
        )
        .await;
    assert_eq!(added.status, StatusCode::NO_CONTENT);
    playlist
}

fn names(listing: &Value) -> Vec<&str> {
    listing["Items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["Name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn a_playlist_is_made_filled_and_read_as_the_reference_reads_it() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let playlist = with_episode(&harness, &token).await;
    let held = harness
        .get(&format!("/jellyfin/Playlists/{playlist}"), &token)
        .await
        .json();
    assert_eq!(
        shape_differences(&held, &fixture("playlist"), &[]),
        Vec::<String>::new()
    );
    assert_eq!(
        held["ItemIds"],
        json!([jellyfin_id("Item", DOODLEBUG), jellyfin_id("Item", EPISODE)])
    );
    // Its owner's alone.
    assert_eq!(held["OpenAccess"], false);

    let entries = harness
        .get(&format!("/jellyfin/Playlists/{playlist}/Items"), &token)
        .await
        .json();
    assert_eq!(
        shape_differences(&entries, &fixture("playlist_items"), NOT_HELD),
        Vec::<String>::new()
    );
    assert_eq!(entries["TotalRecordCount"], 2);
    let first = &entries["Items"][0];
    assert_eq!(first["Id"], jellyfin_id("Item", DOODLEBUG));
    assert_ne!(
        first["PlaylistItemId"], first["Id"],
        "an entry has an id of its own"
    );
    let by_parent = harness
        .get(&format!("/jellyfin/Items?ParentId={playlist}"), &token)
        .await
        .json();
    assert_eq!(by_parent["TotalRecordCount"], 2);
}

#[tokio::test]
async fn playlists_are_listed_and_looked_up_as_folders() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let playlist = with_episode(&harness, &token).await;
    let listing = harness
        .get(
            "/jellyfin/Items?Recursive=true&IncludeItemTypes=Playlist",
            &token,
        )
        .await
        .json();
    assert_eq!(
        shape_differences(&listing, &fixture("playlists_listing"), NOT_HELD),
        Vec::<String>::new()
    );
    assert_eq!(names(&listing), ["Soirée"]);
    assert_eq!(listing["Items"][0]["ChildCount"], 2);
    assert_eq!(listing["Items"][0]["UserData"]["UnplayedItemCount"], 2);
    let looked_up = harness
        .get(&format!("/jellyfin/Items/{playlist}"), &token)
        .await
        .json();
    assert_eq!(looked_up["Type"], "Playlist");
    assert_eq!(looked_up["CanDelete"], true);
    assert_eq!(looked_up["MediaType"], "Video");
}

#[tokio::test]
async fn the_playlists_view_appears_among_the_libraries_once_there_is_one() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let before = harness.get("/jellyfin/UserViews", &token).await.json();
    assert_eq!(names(&before), ["Films", "Séries"]);
    let playlist = created(&harness, &token).await;
    let views = harness.get("/jellyfin/UserViews", &token).await.json();
    assert_eq!(names(&views), ["Films", "Playlists", "Séries"]);
    let view = &views["Items"][1];
    let reference = &fixture("user_views_with_playlist")["Items"][1];
    assert_eq!(view["CollectionType"], reference["CollectionType"]);
    assert_eq!(view["Type"], reference["Type"]);
    let inside = harness
        .get(
            &format!("/jellyfin/Items?ParentId={}", view["Id"].as_str().unwrap()),
            &token,
        )
        .await
        .json();
    assert_eq!(inside["Items"][0]["Id"], playlist.as_str());
}

fn entries(answer: &Value) -> Vec<(String, String)> {
    answer["Items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["Id"].as_str().unwrap().to_owned(),
                item["PlaylistItemId"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn an_entry_moves_to_an_index_and_is_removed_by_its_own_id() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let playlist = with_episode(&harness, &token).await;
    let path = format!("/jellyfin/Playlists/{playlist}/Items");
    let listed = entries(&harness.get(&path, &token).await.json());
    let (episode, episode_entry) = listed[1].clone();
    let moved = harness
        .bare(
            Method::POST,
            &format!("{path}/{episode_entry}/Move/0"),
            &token,
        )
        .await;
    assert_eq!(moved.status, StatusCode::NO_CONTENT);
    let reordered = entries(&harness.get(&path, &token).await.json());
    assert_eq!(reordered[0].0, episode);
    let removed = harness
        .bare(
            Method::DELETE,
            &format!("{path}?entryIds={episode_entry}"),
            &token,
        )
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    let left = harness
        .get(&format!("/jellyfin/Playlists/{playlist}"), &token)
        .await
        .json();
    assert_eq!(left["ItemIds"], json!([jellyfin_id("Item", DOODLEBUG)]));
    let again = harness
        .bare(
            Method::DELETE,
            &format!("{path}?entryIds={episode_entry}"),
            &token,
        )
        .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_playlist_is_renamed_deleted_and_shared_with_nobody() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let playlist = created(&harness, &token).await;
    let renamed = harness
        .post(
            &format!("/jellyfin/Playlists/{playlist}"),
            &token,
            json!({ "Name": "Dimanche" }),
        )
        .await;
    assert_eq!(renamed.status, StatusCode::NO_CONTENT);
    let looked_up = harness
        .get(&format!("/jellyfin/Items/{playlist}"), &token)
        .await
        .json();
    assert_eq!(looked_up["Name"], "Dimanche");
    let users = harness
        .get(&format!("/jellyfin/Playlists/{playlist}/Users"), &token)
        .await
        .json();
    assert_eq!(users, fixture("playlist_users"));
    let shared = harness
        .post(
            &format!("/jellyfin/Playlists/{playlist}"),
            &token,
            json!({ "IsPublic": true }),
        )
        .await;
    assert_eq!(shared.status, StatusCode::NOT_IMPLEMENTED);
    let deleted = harness
        .bare(
            Method::DELETE,
            &format!("/jellyfin/Items/{playlist}"),
            &token,
        )
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let gone = harness
        .get(&format!("/jellyfin/Playlists/{playlist}"), &token)
        .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    let film = harness
        .bare(
            Method::DELETE,
            &format!("/jellyfin/Items/{}", jellyfin_id("Item", DOODLEBUG)),
            &token,
        )
        .await;
    assert_eq!(
        film.status,
        StatusCode::NOT_IMPLEMENTED,
        "media files are never deleted"
    );
}

#[tokio::test]
async fn another_users_playlist_does_not_exist_for_the_caller() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let playlist = created(&harness, &token).await;
    let owner = harness.sign_in_as("owner", "device-2").await.json()["AccessToken"]
        .as_str()
        .unwrap()
        .to_owned();
    let theirs = harness
        .get(&format!("/jellyfin/Playlists/{playlist}"), &owner)
        .await;
    assert_eq!(theirs.status, StatusCode::NOT_FOUND);
}
