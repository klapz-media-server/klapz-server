//! Collections, Jellyfin's box sets: made, filled and emptied as Jellyfin's clients do, against
//! what the reference answered for a collection made for the capture.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use axum::http::{Method, StatusCode};
use common::{fixture, jellyfin_id, shape_differences, Harness, DOODLEBUG, EPISODE, FILMS};
use serde_json::Value;

/// What a box set holds that this server does not: artwork and a provider's metadata.
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

/// An owner's token: a collection is changed by its library's administrators.
async fn owner(harness: &Harness) -> String {
    harness.sign_in_as("owner", "device-2").await.json()["AccessToken"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn created(harness: &Harness, token: &str) -> String {
    let answer = harness
        .bare(
            Method::POST,
            &format!(
                "/jellyfin/Collections?Name=Bestioles&Ids={}",
                jellyfin_id("Item", DOODLEBUG)
            ),
            token,
        )
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    let body = answer.json();
    assert_eq!(
        shape_differences(&body, &fixture("collection_create"), &[]),
        Vec::<String>::new()
    );
    body["Id"].as_str().unwrap().to_owned()
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
async fn a_collection_is_made_in_its_first_items_library_and_listed_as_a_box_set() {
    let harness = Harness::new().await;
    let token = owner(&harness).await;
    let collection = created(&harness, &token).await;
    let listing = harness
        .get(
            "/jellyfin/Items?Recursive=true&IncludeItemTypes=BoxSet",
            &token,
        )
        .await
        .json();
    assert_eq!(
        shape_differences(&listing, &fixture("collections_listing"), NOT_HELD),
        Vec::<String>::new()
    );
    assert_eq!(names(&listing), ["Bestioles"]);
    let in_films = harness
        .get(
            &format!(
                "/jellyfin/Items?ParentId={}&IncludeItemTypes=BoxSet",
                jellyfin_id("Library", FILMS)
            ),
            &token,
        )
        .await
        .json();
    assert_eq!(names(&in_films), ["Bestioles"]);
    let inside = harness
        .get(&format!("/jellyfin/Items?ParentId={collection}"), &token)
        .await
        .json();
    assert_eq!(
        shape_differences(&inside, &fixture("collection_items"), NOT_HELD),
        Vec::<String>::new()
    );
    assert_eq!(inside["Items"][0]["Id"], jellyfin_id("Item", DOODLEBUG));
    let looked_up = harness
        .get(&format!("/jellyfin/Items/{collection}"), &token)
        .await
        .json();
    assert_eq!(looked_up["Type"], "BoxSet");
    assert_eq!(looked_up["ChildCount"], 1);
    let of_item = harness
        .get(
            &format!(
                "/jellyfin/Items/{}/Collections",
                jellyfin_id("Item", DOODLEBUG)
            ),
            &token,
        )
        .await
        .json();
    assert_eq!(
        shape_differences(&of_item, &fixture("item_collections"), NOT_HELD),
        Vec::<String>::new()
    );
    assert_eq!(names(&of_item), ["Bestioles"]);
}

#[tokio::test]
async fn the_collections_view_appears_among_the_libraries_once_there_is_one() {
    let harness = Harness::new().await;
    let token = owner(&harness).await;
    created(&harness, &token).await;
    let views = harness.get("/jellyfin/UserViews", &token).await.json();
    let view = views["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|view| view["Name"] == "Collections")
        .unwrap();
    let reference = &fixture("user_views_with_collection")["Items"][0];
    assert_eq!(view["CollectionType"], reference["CollectionType"]);
    assert_eq!(view["Type"], reference["Type"]);
    let inside = harness
        .get(
            &format!("/jellyfin/Items?ParentId={}", view["Id"].as_str().unwrap()),
            &token,
        )
        .await
        .json();
    assert_eq!(names(&inside), ["Bestioles"]);
}

#[tokio::test]
async fn items_join_and_leave_and_a_collection_is_deleted_its_items_kept() {
    let harness = Harness::new().await;
    let token = owner(&harness).await;
    let collection = created(&harness, &token).await;
    // An episode is of another library: not joinable, refused.
    let episode = harness
        .bare(
            Method::POST,
            &format!(
                "/jellyfin/Collections/{collection}/Items?ids={}",
                jellyfin_id("Item", EPISODE)
            ),
            &token,
        )
        .await;
    assert_eq!(episode.status, StatusCode::BAD_REQUEST);
    let left = harness
        .bare(
            Method::DELETE,
            &format!(
                "/jellyfin/Collections/{collection}/Items?ids={}",
                jellyfin_id("Item", DOODLEBUG)
            ),
            &token,
        )
        .await;
    assert_eq!(left.status, StatusCode::NO_CONTENT);
    let empty = harness
        .get(&format!("/jellyfin/Items?ParentId={collection}"), &token)
        .await
        .json();
    assert_eq!(empty["TotalRecordCount"], 0);
    let deleted = harness
        .bare(
            Method::DELETE,
            &format!("/jellyfin/Items/{collection}"),
            &token,
        )
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let film = harness
        .get(
            &format!("/jellyfin/Items/{}", jellyfin_id("Item", DOODLEBUG)),
            &token,
        )
        .await;
    assert_eq!(film.status, StatusCode::OK);
}

#[tokio::test]
async fn a_viewer_may_not_change_a_librarys_collections() {
    let harness = Harness::new().await;
    let alice = harness.token().await;
    let refused = harness
        .bare(
            Method::POST,
            &format!(
                "/jellyfin/Collections?Name=x&Ids={}",
                jellyfin_id("Item", DOODLEBUG)
            ),
            &alice,
        )
        .await;
    assert!(
        matches!(
            refused.status,
            StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST
        ),
        "{}",
        refused.status
    );
    let locked = harness
        .bare(
            Method::POST,
            &format!(
                "/jellyfin/Collections?Name=x&IsLocked=true&Ids={}",
                jellyfin_id("Item", DOODLEBUG)
            ),
            &alice,
        )
        .await;
    assert_eq!(locked.status, StatusCode::NOT_IMPLEMENTED);
}
