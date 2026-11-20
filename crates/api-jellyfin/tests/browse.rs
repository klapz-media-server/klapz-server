//! Browsing by genre, person, studio and year, and the search box's hints, against what the
//! reference answered about its films.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use common::{fixture, jellyfin_id, shape_differences, Harness, DOODLEBUG, FILMS, NOLAN, THRILLER};
use serde_json::{json, Value};

/// Artwork the reference keeps for genres and studios, and none is held here.
const NOT_HELD: &[&str] = &[
    "Primary",
    "Thumb",
    "PrimaryImageAspectRatio",
    "Etag",
    "DateCreated",
    "Path",
];

fn names(listing: &Value) -> Vec<&str> {
    listing["Items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["Name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn a_librarys_genres_are_listed_with_their_counts() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let films = jellyfin_id("Library", FILMS);
    let genres = harness
        .get(
            &format!(
                "/jellyfin/Genres?ParentId={films}&SortBy=SortName&Recursive=true\
                 &Fields=PrimaryImageAspectRatio,ItemCounts&StartIndex=0"
            ),
            &token,
        )
        .await;
    assert_eq!(genres.status, StatusCode::OK);
    let genres = genres.json();
    assert_eq!(
        shape_differences(&genres, &fixture("genres"), NOT_HELD),
        Vec::<String>::new()
    );
    assert_eq!(names(&genres), ["Thriller"]);
    let thriller = &genres["Items"][0];
    assert_eq!(thriller["Id"], jellyfin_id("Tag", THRILLER));
    assert_eq!(thriller["MovieCount"], 1);
    assert_eq!(thriller["SeriesCount"], 0);
    let one = harness
        .get("/jellyfin/Genres/thriller", &token)
        .await
        .json();
    assert_eq!(one["Name"], "Thriller");
    let none = harness.get("/jellyfin/Genres/Western", &token).await;
    assert_eq!(none.status, StatusCode::NOT_FOUND);
    let by_genre = harness
        .get(
            &format!(
                "/jellyfin/Items?GenreIds={}&Recursive=true&IncludeItemTypes=Movie",
                jellyfin_id("Tag", THRILLER)
            ),
            &token,
        )
        .await
        .json();
    assert_eq!(names(&by_genre), ["doodlebug"]);
}

#[tokio::test]
async fn people_are_found_and_list_what_credits_them() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let found = harness
        .get("/jellyfin/Persons?searchTerm=nolan", &token)
        .await
        .json();
    assert_eq!(
        shape_differences(&found, &fixture("persons"), NOT_HELD),
        Vec::<String>::new()
    );
    assert_eq!(names(&found), ["Christopher Nolan"]);
    assert_eq!(found["Items"][0]["Id"], jellyfin_id("Person", NOLAN));
    let directors = harness
        .get("/jellyfin/Persons?personTypes=Director", &token)
        .await
        .json();
    assert_eq!(names(&directors), ["Christopher Nolan"]);
    let actors = harness
        .get("/jellyfin/Persons?personTypes=Actor", &token)
        .await
        .json();
    assert_eq!(actors["TotalRecordCount"], 0);
    let everyone = harness.get("/jellyfin/Persons", &token).await;
    assert_eq!(everyone.status, StatusCode::NOT_IMPLEMENTED);
    let one = harness
        .get("/jellyfin/Persons/Christopher%20Nolan", &token)
        .await
        .json();
    assert_eq!(one["Type"], "Person");
    let credits = harness
        .get(
            &format!(
                "/jellyfin/Items?PersonIds={}&Recursive=true&IncludeItemTypes=Movie,Series",
                jellyfin_id("Person", NOLAN)
            ),
            &token,
        )
        .await;
    assert_eq!(credits.status, StatusCode::OK);
    let credits = credits.json();
    assert_eq!(
        shape_differences(
            &json!({ "TotalRecordCount": credits["TotalRecordCount"], "StartIndex": 0 }),
            &json!({ "TotalRecordCount": 1, "StartIndex": 0 }),
            &[]
        ),
        Vec::<String>::new()
    );
    assert_eq!(names(&credits), ["doodlebug"]);
    let two = harness
        .get(
            &format!(
                "/jellyfin/Items?PersonIds={},{}",
                jellyfin_id("Person", NOLAN),
                jellyfin_id("Person", NOLAN + 1)
            ),
            &token,
        )
        .await;
    assert_eq!(two.status, StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn studios_and_years_are_pages_of_what_the_items_hold() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let films = jellyfin_id("Library", FILMS);
    let studios = harness
        .get(&format!("/jellyfin/Studios?ParentId={films}"), &token)
        .await
        .json();
    assert_eq!(
        shape_differences(&studios, &fixture("studios"), NOT_HELD),
        Vec::<String>::new()
    );
    assert_eq!(names(&studios), ["Following Productions"]);
    let studio = harness
        .get("/jellyfin/Studios/Following%20Productions", &token)
        .await;
    assert_eq!(studio.status, StatusCode::OK);
    let years = harness
        .get(
            &format!("/jellyfin/Years?ParentId={films}&Recursive=true&SortOrder=Descending"),
            &token,
        )
        .await
        .json();
    assert_eq!(
        shape_differences(&years, &fixture("years"), NOT_HELD),
        Vec::<String>::new()
    );
    assert_eq!(names(&years), ["1997"]);
    let year = harness.get("/jellyfin/Years/1997", &token).await.json();
    assert_eq!(year["Name"], "1997");
    assert_eq!(year["UserData"]["Key"], "Year-1997");
    let absent = harness.get("/jellyfin/Years/1890", &token).await;
    assert_eq!(absent.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_search_box_hints_items_then_people() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let hints = harness
        .get("/jellyfin/Search/Hints?searchTerm=doodle&Limit=20", &token)
        .await;
    assert_eq!(hints.status, StatusCode::OK);
    let hints = hints.json();
    assert_eq!(
        shape_differences(&hints, &fixture("search_hints"), &[]),
        Vec::<String>::new()
    );
    assert_eq!(hints["TotalRecordCount"], 1);
    let hint = &hints["SearchHints"][0];
    assert_eq!(hint["Id"], jellyfin_id("Item", DOODLEBUG));
    assert_eq!(hint["Type"], "Movie");
    assert_eq!(hint["ProductionYear"], 1997);
    assert_eq!(hint["RunTimeTicks"], 1_783_010_000_i64);
    let people = harness
        .get("/jellyfin/Search/Hints?searchTerm=christopher", &token)
        .await
        .json();
    assert_eq!(people["SearchHints"][0]["Type"], "Person");
    let without = harness
        .get(
            "/jellyfin/Search/Hints?searchTerm=christopher&includePeople=false",
            &token,
        )
        .await
        .json();
    assert_eq!(without["TotalRecordCount"], 0);
    let paged = harness
        .get(
            "/jellyfin/Search/Hints?searchTerm=doodle&StartIndex=20",
            &token,
        )
        .await;
    assert_eq!(paged.status, StatusCode::NOT_IMPLEMENTED);
}
