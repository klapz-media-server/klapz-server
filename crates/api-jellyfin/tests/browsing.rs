//! Browsing: views, listings, lookups and the home screen's shelves, against what the
//! reference Jellyfin answered about the same files.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use common::{
    fixture, jellyfin_id, shape_differences, Harness, DOODLEBUG, DOODLEBUG_FILE, EPISODE, FILMS,
    HIDDEN_FILM, PRIVATE, SEASON, SERIES, SHOW,
};
use serde_json::Value;

/// What the reference holds and this server does not: artwork, metadata from online
/// providers, and the tags of its own caches. Each item comparison may lack these.
const NOT_HELD: &[&str] = &[
    "Primary",
    "Backdrop",
    "Thumb",
    "Logo",
    "Banner",
    "PrimaryImageAspectRatio",
    "Etag",
    "ETag",
    "Tmdb",
    "Imdb",
    "Tvdb",
    "TvRage",
    "OriginalLanguage",
    "Trickplay",
    "CriticRating",
    "CommunityRating",
    "OfficialRating",
    "PremiereDate",
    "DateLastMediaAdded",
    "ProductionLocations",
    "SeriesStudio",
    "ParentLogoItemId",
    "ParentLogoImageTag",
    "ParentBackdropItemId",
    "ParentBackdropImageTags",
    "ParentThumbItemId",
    "ParentThumbImageTag",
    "ParentPrimaryImageItemId",
    "ParentPrimaryImageTag",
    "SeriesPrimaryImageTag",
    "Status",
    "AirDays",
    "EndDate",
    "Path",
    "Overview",
    "ProductionYear",
    "RunTimeTicks",
    "CumulativeRunTimeTicks",
    "OriginalTitle",
    // What ffprobe reports and the scan does not keep.
    "TimeBase",
    "NalLengthSize",
    "CodecTag",
    "ColorSpace",
    "ColorTransfer",
    "ColorPrimaries",
    "LocalizedDefault",
    "LocalizedExternal",
    "LocalizedLanguage",
    "LocalizedOriginal",
    "HasSubtitles",
    "Title",
    // A trace of the reference's own request handling.
    "traceId",
];

fn no_differences(ours: &Value, reference: &Value) {
    assert_eq!(
        shape_differences(ours, reference, NOT_HELD),
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn the_views_are_the_libraries_the_user_may_read_with_the_reference_shape() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let answer = harness
        .get(
            &format!(
                "/jellyfin/UserViews?userId={}",
                jellyfin_id("User", harness.alice.get())
            ),
            &token,
        )
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    let views = answer.json();
    no_differences(&views, &fixture("user_views"));
    let names: Vec<&str> = views["Items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|view| view["Name"].as_str().unwrap())
        .collect();
    // Not the private library, which alice was not given.
    assert_eq!(names, ["Films", "Séries"]);
    assert_eq!(views["Items"][0]["CollectionType"], "movies");
    assert_eq!(views["Items"][1]["CollectionType"], "tvshows");
    assert_eq!(views["Items"][0]["Id"], jellyfin_id("Library", FILMS));
    assert_eq!(views["Items"][0]["ChildCount"], 1);
    let legacy = harness
        .get(
            &format!(
                "/jellyfin/Users/{}/Views",
                jellyfin_id("User", harness.alice.get())
            ),
            &token,
        )
        .await;
    assert_eq!(legacy.json(), views);
}

#[tokio::test]
async fn a_library_lists_its_films_with_the_reference_shape() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let answer = harness
        .get(
            &format!(
                "/jellyfin/Items?ParentId={}&Recursive=true&IncludeItemTypes=Movie\
                 &SortBy=SortName,ProductionYear&SortOrder=Ascending&StartIndex=0&Limit=100\
                 &Fields=PrimaryImageAspectRatio,SortName,Path,MediaSourceCount",
                jellyfin_id("Library", FILMS)
            ),
            &token,
        )
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(
        answer.content_type(),
        Some("application/json; charset=utf-8")
    );
    let page = answer.json();
    no_differences(&page, &fixture("items_movies"));
    assert_eq!(page["TotalRecordCount"], 1);
    assert_eq!(page["StartIndex"], 0);
    let film = &page["Items"][0];
    assert_eq!(film["Name"], "doodlebug");
    assert_eq!(film["Id"], jellyfin_id("Item", DOODLEBUG));
    assert_eq!(film["SortName"], "doodlebug");
    assert_eq!(film["Container"], "mkv");
    assert_eq!(film["Path"], "/media/Movies/VO/doodlebug.1997.VO.H264.mkv");
    // Asked of none of these.
    for absent in [
        "MediaSources",
        "MediaStreams",
        "Overview",
        "People",
        "Genres",
    ] {
        assert!(film.get(absent).is_none(), "{absent}");
    }
}

#[tokio::test]
async fn a_series_library_lists_its_series_and_their_children() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let series = harness
        .get(
            &format!(
                "/jellyfin/Items?ParentId={}&Recursive=true&IncludeItemTypes=Series\
                 &SortBy=SortName&Fields=PrimaryImageAspectRatio,SortName,Path",
                jellyfin_id("Library", SERIES)
            ),
            &token,
        )
        .await
        .json();
    no_differences(&series, &fixture("items_series"));
    assert_eq!(series["Items"][0]["Name"], "90210");
    assert_eq!(series["Items"][0]["Type"], "Series");
    assert_eq!(series["Items"][0]["UserData"]["UnplayedItemCount"], 1);

    let seasons = harness
        .get(
            &format!(
                "/jellyfin/Shows/{}/Seasons?Fields=ItemCounts,PrimaryImageAspectRatio",
                jellyfin_id("Item", SHOW)
            ),
            &token,
        )
        .await
        .json();
    no_differences(&seasons, &fixture("show_seasons"));
    assert_eq!(seasons["Items"][0]["Id"], jellyfin_id("Item", SEASON));
    assert_eq!(seasons["Items"][0]["IndexNumber"], 1);
    assert_eq!(seasons["Items"][0]["SeriesId"], jellyfin_id("Item", SHOW));

    let episodes = harness
        .get(
            &format!(
                "/jellyfin/Shows/{}/Episodes?seasonId={}&Fields=ItemCounts,Overview",
                jellyfin_id("Item", SHOW),
                jellyfin_id("Item", SEASON)
            ),
            &token,
        )
        .await
        .json();
    no_differences(&episodes, &fixture("show_episodes"));
    let episode = &episodes["Items"][0];
    assert_eq!(episode["Name"], "The Dionysian Debacle");
    assert_eq!(episode["IndexNumber"], 21);
    assert_eq!(episode["ParentIndexNumber"], 1);
    assert_eq!(episode["SeasonId"], jellyfin_id("Item", SEASON));
    assert_eq!(episode["SeriesName"], "90210");

    // The same children, listed under their parent.
    let under_season = harness
        .get(
            &format!("/jellyfin/Items?ParentId={}", jellyfin_id("Item", SEASON)),
            &token,
        )
        .await
        .json();
    assert_eq!(under_season["Items"][0]["Id"], jellyfin_id("Item", EPISODE));
    assert_eq!(under_season["TotalRecordCount"], 1);
}

#[tokio::test]
async fn an_episode_is_listed_by_season_number_too() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let show = jellyfin_id("Item", SHOW);
    let first = harness
        .get(&format!("/jellyfin/Shows/{show}/Episodes?season=1"), &token)
        .await
        .json();
    assert_eq!(first["TotalRecordCount"], 1);
    let second = harness
        .get(&format!("/jellyfin/Shows/{show}/Episodes?season=2"), &token)
        .await
        .json();
    assert_eq!(second["TotalRecordCount"], 0);
}

#[tokio::test]
async fn a_film_looked_up_carries_every_field_and_its_media_as_the_reference() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let answer = harness
        .get(
            &format!("/jellyfin/Items/{}", jellyfin_id("Item", DOODLEBUG)),
            &token,
        )
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    let film = answer.json();
    let reference = fixture("item_movie");
    no_differences(&film, &reference);
    let source = &film["MediaSources"][0];
    assert_eq!(source["Id"], jellyfin_id("MediaFile", DOODLEBUG_FILE));
    assert_eq!(source["Container"], "mkv");
    assert_eq!(source["Size"], 14_876_546);
    assert_eq!(source["DefaultAudioStreamIndex"], 1);
    // The tracks the reference measured on the same file, field for field where we hold them.
    for (index, expected) in reference["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let ours = &source["MediaStreams"][index];
        for key in [
            "Codec",
            "Language",
            "Type",
            "Index",
            "DisplayTitle",
            "IsDefault",
            "IsForced",
            "IsExternal",
            "IsInterlaced",
            "Height",
            "Width",
            "Level",
            "Profile",
            "ChannelLayout",
            "Channels",
            "SampleRate",
            "VideoRange",
            "VideoRangeType",
            "AspectRatio",
        ] {
            if expected.get(key).is_some() {
                assert_eq!(ours[key], expected[key], "track {index}: {key}");
            }
        }
    }
    // As the reference, under every user's path too.
    let legacy = harness
        .get(
            &format!(
                "/jellyfin/Users/{}/Items/{}",
                jellyfin_id("User", harness.alice.get()),
                jellyfin_id("Item", DOODLEBUG)
            ),
            &token,
        )
        .await;
    assert_eq!(legacy.json(), film);
}

#[tokio::test]
async fn a_series_season_and_episode_looked_up_have_the_reference_shape() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    for (key, name) in [
        (SHOW, "item_series"),
        (SEASON, "item_season"),
        (EPISODE, "item_episode"),
    ] {
        let answer = harness
            .get(
                &format!("/jellyfin/Items/{}", jellyfin_id("Item", key)),
                &token,
            )
            .await;
        assert_eq!(answer.status, StatusCode::OK, "{name}");
        no_differences(&answer.json(), &fixture(name));
    }
}

#[tokio::test]
async fn a_library_the_user_may_not_read_does_not_exist() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    for uri in [
        format!(
            "/jellyfin/Items?ParentId={}",
            jellyfin_id("Library", PRIVATE)
        ),
        format!("/jellyfin/Items/{}", jellyfin_id("Item", HIDDEN_FILM)),
        format!("/jellyfin/Items/{}", jellyfin_id("Library", PRIVATE)),
    ] {
        let answer = harness.get(&uri, &token).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{uri}");
    }
    let everything = harness
        .get(
            "/jellyfin/Items?Recursive=true&IncludeItemTypes=Movie",
            &token,
        )
        .await
        .json();
    assert_eq!(everything["TotalRecordCount"], 1);
    assert_eq!(everything["Items"][0]["Id"], jellyfin_id("Item", DOODLEBUG));
}

#[tokio::test]
async fn another_users_listing_is_not_found() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let other = jellyfin_id("User", harness.alice.get() + 1);
    for uri in [
        format!("/jellyfin/Users/{other}/Items"),
        format!("/jellyfin/Items?userId={other}"),
        format!("/jellyfin/UserViews?userId={other}"),
    ] {
        assert_eq!(
            harness.get(&uri, &token).await.status,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn ids_ask_for_items_by_id() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let page = harness
        .get(
            &format!(
                "/jellyfin/Items?Ids={},{}",
                jellyfin_id("Item", DOODLEBUG),
                jellyfin_id("Item", HIDDEN_FILM)
            ),
            &token,
        )
        .await
        .json();
    // The hidden film is not the caller's to see.
    assert_eq!(page["TotalRecordCount"], 1);
    assert_eq!(page["Items"][0]["Name"], "doodlebug");
}

#[tokio::test]
async fn filters_that_cannot_be_honored_are_refused_and_favorites_are_none() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let library = jellyfin_id("Library", FILMS);
    for refused in [
        "NameStartsWith=D",
        "PersonIds=x",
        "SortBy=AlbumArtist",
        "Is4K=true",
    ] {
        let answer = harness
            .get(
                &format!("/jellyfin/Items?ParentId={library}&{refused}"),
                &token,
            )
            .await;
        assert_eq!(answer.status, StatusCode::NOT_IMPLEMENTED, "{refused}");
    }
    let favorites = harness
        .get("/jellyfin/Items?Recursive=true&Filters=IsFavorite", &token)
        .await
        .json();
    assert_eq!(favorites["TotalRecordCount"], 0);
    let unplayed = harness
        .get(
            &format!("/jellyfin/Items?ParentId={library}&Filters=IsUnplayed"),
            &token,
        )
        .await
        .json();
    assert_eq!(unplayed["TotalRecordCount"], 1);
    let played = harness
        .get(
            &format!("/jellyfin/Items?ParentId={library}&Filters=IsPlayed"),
            &token,
        )
        .await
        .json();
    assert_eq!(played["TotalRecordCount"], 0);
}

#[tokio::test]
async fn a_window_pages_a_listing() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let page = harness
        .get(
            &format!(
                "/jellyfin/Items?ParentId={}&StartIndex=1&Limit=10",
                jellyfin_id("Library", FILMS)
            ),
            &token,
        )
        .await
        .json();
    assert_eq!(page["TotalRecordCount"], 1);
    assert_eq!(page["StartIndex"], 1);
    assert_eq!(page["Items"], serde_json::json!([]));
}

#[tokio::test]
async fn the_home_shelves_answer_as_the_reference() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let resume = harness
        .get(
            "/jellyfin/UserItems/Resume?Limit=12&MediaTypes=Video",
            &token,
        )
        .await
        .json();
    assert_eq!(resume, fixture("resume"));
    let next_up = harness
        .get("/jellyfin/Shows/NextUp?Limit=24", &token)
        .await
        .json();
    assert_eq!(next_up, fixture("next_up"));
    let latest = harness
        .get(
            &format!(
                "/jellyfin/Items/Latest?ParentId={}&Limit=16",
                jellyfin_id("Library", FILMS)
            ),
            &token,
        )
        .await;
    assert_eq!(latest.status, StatusCode::OK);
    let latest = latest.json();
    no_differences(&latest, &fixture("latest_movies"));
    assert_eq!(latest[0]["Id"], jellyfin_id("Item", DOODLEBUG));
}

#[tokio::test]
async fn no_artwork_is_served() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let film = jellyfin_id("Item", DOODLEBUG);
    for uri in [
        format!("/jellyfin/Items/{film}/Images/Primary"),
        format!("/jellyfin/Items/{film}/Images/Backdrop/0"),
    ] {
        assert_eq!(
            harness.get(&uri, &token).await.status,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn an_id_that_is_no_guid_is_a_bad_request_and_an_unknown_one_not_found() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let malformed = harness.get("/jellyfin/Items/zzz", &token).await;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        malformed.json()["status"],
        fixture("item_malformed")["status"]
    );
    let unknown = harness
        .get("/jellyfin/Items/00000000000000000000000000000001", &token)
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    no_differences(&unknown.json(), &fixture("item_unknown"));
}
