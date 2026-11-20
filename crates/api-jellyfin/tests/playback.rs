//! Playing: what a client may play and how, the stream addresses, and what it reports back.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use axum::http::{Method, StatusCode};
use common::{
    client_header, fixture, jellyfin_id, shape_differences, Harness, DOODLEBUG, DOODLEBUG_AUDIO,
    EPISODE, HIDDEN_FILM,
};
use open_plex_core::stream::Delivery;
use serde_json::{json, Value};

/// A browser's profile: MKV and MP4 with H.264, AAC or Opus, as Jellyfin Web posts it.
fn browser(max_bitrate: i64) -> Value {
    json!({
        "MaxStreamingBitrate": max_bitrate,
        "DeviceProfile": {
            "MaxStreamingBitrate": max_bitrate,
            "DirectPlayProfiles": [
                { "Container": "webm", "Type": "Video", "VideoCodec": "vp8,vp9,av1", "AudioCodec": "vorbis,opus" },
                { "Container": "mp4,m4v,mkv", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac,opus" },
            ],
            "TranscodingProfiles": [
                { "Container": "ts", "Type": "Video", "AudioCodec": "aac", "VideoCodec": "h264", "Protocol": "hls" },
            ],
        },
    })
}

/// A player of MP4 alone, which cannot read the film's MKV.
fn mp4_only() -> Value {
    json!({
        "DeviceProfile": {
            "DirectPlayProfiles": [
                { "Container": "mp4", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac" },
            ],
        },
    })
}

async fn playback_info(harness: &Harness, token: &str, body: Value) -> Value {
    let answer = harness
        .post(
            &format!(
                "/jellyfin/Items/{}/PlaybackInfo?userId={}",
                jellyfin_id("Item", DOODLEBUG),
                jellyfin_id("User", harness.alice.get())
            ),
            token,
            body,
        )
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    answer.json()
}

#[tokio::test]
async fn a_player_that_reads_the_file_plays_it_directly() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let info = playback_info(&harness, &token, browser(120_000_000)).await;
    let source = &info["MediaSources"][0];
    assert_eq!(source["Id"], jellyfin_id("Item", DOODLEBUG));
    assert_eq!(source["SupportsDirectPlay"], true);
    // What a browser, which cannot open the path, reads it through.
    assert_eq!(source["SupportsDirectStream"], true);
    assert_eq!(source["SupportsTranscoding"], true);
    assert!(source.get("TranscodingUrl").is_none());
    assert_eq!(info["PlaySessionId"].as_str().unwrap().len(), 32);
}

#[tokio::test]
async fn a_player_that_cannot_read_it_is_given_a_conversion_with_the_reference_shape() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let info = playback_info(&harness, &token, mp4_only()).await;
    assert_eq!(
        shape_differences(
            &info,
            &fixture("playback_info"),
            &[
                "ETag",
                "TimeBase",
                "NalLengthSize",
                "LocalizedDefault",
                "LocalizedExternal",
                "LocalizedLanguage",
                "LocalizedOriginal",
            ],
        ),
        Vec::<String>::new()
    );
    let source = &info["MediaSources"][0];
    assert_eq!(source["SupportsDirectPlay"], false);
    assert_eq!(source["SupportsDirectStream"], false);
    assert_eq!(source["TranscodingSubProtocol"], "hls");
    assert_eq!(source["TranscodingContainer"], "ts");
    let url = source["TranscodingUrl"].as_str().unwrap();
    assert!(url.starts_with("/videos/"), "{url}");
    assert!(url.contains("/master.m3u8?"), "{url}");
    assert!(url.contains(&format!(
        "PlaySessionId={}",
        info["PlaySessionId"].as_str().unwrap()
    )));
    assert!(url.contains(&format!("ApiKey={token}")), "{url}");
}

#[tokio::test]
async fn a_file_heavier_than_the_player_takes_is_converted() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    // The film's version weighs 667 bits per second in the harness.
    let info = playback_info(&harness, &token, browser(100)).await;
    assert_eq!(info["MediaSources"][0]["SupportsDirectPlay"], false);
}

#[tokio::test]
async fn a_client_that_disables_direct_play_is_given_a_conversion() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let mut body = browser(120_000_000);
    body["EnableDirectPlay"] = json!(false);
    let info = playback_info(&harness, &token, body).await;
    assert_eq!(info["MediaSources"][0]["SupportsDirectPlay"], false);
}

#[tokio::test]
async fn an_item_the_user_may_not_read_has_no_playback_info() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let answer = harness
        .post(
            &format!(
                "/jellyfin/Items/{}/PlaybackInfo",
                jellyfin_id("Item", HIDDEN_FILM)
            ),
            &token,
            browser(120_000_000),
        )
        .await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn direct_play_starts_the_stream_and_sends_the_player_to_the_file() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let answer = harness
        .get(
            &format!(
                "/jellyfin/Videos/{}/stream.mkv?static=true&mediaSourceId={}&PlaySessionId=ps1",
                jellyfin_id("Item", DOODLEBUG),
                jellyfin_id("Item", DOODLEBUG)
            ),
            &token,
        )
        .await;
    assert_eq!(answer.status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(answer.headers["location"], "/streams/stub/file?token=stub");
    let started = harness.streams.started.lock().unwrap();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].delivery, Delivery::Original);
    assert!(started[0].key.ends_with("ps1"), "{}", started[0].key);
}

#[tokio::test]
async fn the_transcoding_address_starts_an_hls_conversion() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let info = playback_info(&harness, &token, mp4_only()).await;
    let url = info["MediaSources"][0]["TranscodingUrl"].as_str().unwrap();
    // Relative to the server's base, as clients resolve it; the token travels in it.
    let answer = harness
        .send(
            Method::GET,
            &format!("/jellyfin{url}&AudioStreamIndex=1&StartTimeTicks=600000000"),
            &[],
            None,
        )
        .await;
    assert_eq!(answer.status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        answer.headers["location"],
        "/streams/stub/master.m3u8?token=stub"
    );
    let started = harness.streams.started.lock().unwrap();
    assert_eq!(
        started[0].delivery,
        Delivery::Converted(open_plex_core::stream::StreamProtocol::Hls)
    );
    assert_eq!(started[0].position_ms, 60_000);
    assert_eq!(started[0].source.audio.codec, "opus");
    // Stored under the caller's name for it, as every client-chosen key is.
    let session = info["PlaySessionId"].as_str().unwrap();
    assert!(started[0]
        .playback
        .as_deref()
        .is_some_and(|playback| playback.ends_with(session)));
}

#[tokio::test]
async fn a_stream_converted_to_another_container_is_refused() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let answer = harness
        .get(
            &format!(
                "/jellyfin/Videos/{}/stream.mp4",
                jellyfin_id("Item", DOODLEBUG)
            ),
            &token,
        )
        .await;
    assert_eq!(answer.status, StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn progress_reports_keep_the_position_and_stopping_ends_the_session() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let film = jellyfin_id("Item", DOODLEBUG);
    for (path, ticks) in [
        ("/jellyfin/Sessions/Playing", 0_i64),
        ("/jellyfin/Sessions/Playing/Progress", 900_000_000),
        ("/jellyfin/Sessions/Playing/Stopped", 1_000_000_000),
    ] {
        let answer = harness
            .post(
                path,
                &token,
                json!({
                    "ItemId": film,
                    "MediaSourceId": jellyfin_id("Item", DOODLEBUG),
                    "PlaySessionId": "ps1",
                    "PositionTicks": ticks,
                    "IsPaused": false,
                }),
            )
            .await;
        assert_eq!(answer.status, StatusCode::NO_CONTENT, "{path}");
    }
    let lookup = harness
        .get(&format!("/jellyfin/Items/{film}"), &token)
        .await
        .json();
    // 100 s of a 178 s film: kept, as the official API keeps a position past a minute.
    assert_eq!(lookup["UserData"]["PlaybackPositionTicks"], 1_000_000_000);
    assert_eq!(lookup["UserData"]["Played"], false);
    let resume = harness
        .get("/jellyfin/UserItems/Resume", &token)
        .await
        .json();
    assert_eq!(resume["Items"][0]["Id"], film);
}

#[tokio::test]
async fn a_report_past_ninety_percent_counts_a_viewing() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let film = jellyfin_id("Item", DOODLEBUG);
    for ticks in [100_000_000_i64, 1_700_000_000] {
        harness
            .post(
                "/jellyfin/Sessions/Playing/Progress",
                &token,
                json!({ "ItemId": film, "PlaySessionId": "ps1", "PositionTicks": ticks }),
            )
            .await;
    }
    let lookup = harness
        .get(&format!("/jellyfin/Items/{film}"), &token)
        .await
        .json();
    assert_eq!(lookup["UserData"]["Played"], true);
    assert_eq!(lookup["UserData"]["PlayCount"], 1);
}

#[tokio::test]
async fn an_item_is_marked_played_and_unplayed() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let film = jellyfin_id("Item", DOODLEBUG);
    let played = harness
        .post(
            &format!("/jellyfin/UserPlayedItems/{film}"),
            &token,
            json!({}),
        )
        .await;
    assert_eq!(played.status, StatusCode::OK);
    let data = played.json();
    assert_eq!(data["Played"], true);
    assert_eq!(data["PlayCount"], 1);
    assert_eq!(data["ItemId"], film);
    let unplayed = harness
        .send(
            Method::DELETE,
            &format!("/jellyfin/UserPlayedItems/{film}"),
            &[("authorization", client_header("device-1", Some(&token)))],
            None,
        )
        .await;
    assert_eq!(unplayed.status, StatusCode::OK);
    assert_eq!(unplayed.json()["Played"], false);
    let hidden = harness
        .post(
            &format!(
                "/jellyfin/UserPlayedItems/{}",
                jellyfin_id("Item", HIDDEN_FILM)
            ),
            &token,
            json!({}),
        )
        .await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn favorites_are_refused_rather_than_forgotten() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let answer = harness
        .post(
            &format!(
                "/jellyfin/UserFavoriteItems/{}",
                jellyfin_id("Item", DOODLEBUG)
            ),
            &token,
            json!({}),
        )
        .await;
    assert_eq!(answer.status, StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn an_audio_track_named_by_its_index_is_the_one_played() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    harness
        .get(
            &format!(
                "/jellyfin/Videos/{}/master.m3u8?MediaSourceId={}&AudioStreamIndex=1",
                jellyfin_id("Item", DOODLEBUG),
                jellyfin_id("Item", DOODLEBUG)
            ),
            &token,
        )
        .await;
    let started = harness.streams.started.lock().unwrap();
    assert_eq!(started[0].source.audio.position, 1);
    // The track the index names, the film's only audio.
    let _ = DOODLEBUG_AUDIO;
}

#[tokio::test]
async fn an_episodes_intro_and_credits_are_its_segments() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let episode = jellyfin_id("Item", EPISODE);
    let answer = harness
        .get(&format!("/jellyfin/MediaSegments/{episode}"), &token)
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    let segments = answer.json();
    assert_eq!(
        shape_differences(&segments, &fixture("media_segments"), &[]),
        Vec::<String>::new()
    );
    assert_eq!(segments["TotalRecordCount"], 2);
    let intro = &segments["Items"][0];
    assert_eq!(intro["Type"], "Intro");
    assert_eq!(intro["ItemId"], episode.as_str());
    assert_eq!(intro["Id"], jellyfin_id("Marker", common::INTRO));
    assert_eq!(intro["StartTicks"], 120_000_000);
    assert_eq!(intro["EndTicks"], 615_000_000);
    assert_eq!(segments["Items"][1]["Type"], "Outro");
    let outros = harness
        .get(
            &format!("/jellyfin/MediaSegments/{episode}?includeSegmentTypes=Outro"),
            &token,
        )
        .await
        .json();
    assert_eq!(outros["TotalRecordCount"], 1);
    let film = harness
        .get(
            &format!("/jellyfin/MediaSegments/{}", jellyfin_id("Item", DOODLEBUG)),
            &token,
        )
        .await
        .json();
    assert_eq!(film, fixture("media_segments"));
}

#[tokio::test]
async fn live_streams_answer_as_where_none_is_ever_opened() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let item = jellyfin_id("Item", DOODLEBUG);
    // As the reference answered: no token is a 400, an unknown one its 500, both one line.
    let without = harness
        .post(
            &format!("/jellyfin/LiveStreams/Open?itemId={item}"),
            &token,
            json!({ "ItemId": item }),
        )
        .await;
    assert_eq!(without.status, StatusCode::BAD_REQUEST);
    assert_eq!(without.content_type(), Some("text/plain"));
    assert_eq!(without.body, b"Error processing request.");
    let unknown = harness
        .post(
            &format!("/jellyfin/LiveStreams/Open?openToken=abc&itemId={item}"),
            &token,
            json!({}),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::INTERNAL_SERVER_ERROR);
    let in_body = harness
        .post(
            "/jellyfin/LiveStreams/Open",
            &token,
            json!({ "OpenToken": "abc" }),
        )
        .await;
    assert_eq!(in_body.status, StatusCode::INTERNAL_SERVER_ERROR);

    let closed = harness
        .bare(
            Method::POST,
            "/jellyfin/LiveStreams/Close?liveStreamId=abc",
            &token,
        )
        .await;
    assert_eq!(closed.status, StatusCode::NO_CONTENT);
    assert!(closed.body.is_empty());
    let unnamed = harness
        .bare(Method::POST, "/jellyfin/LiveStreams/Close", &token)
        .await;
    assert_eq!(unnamed.status, StatusCode::BAD_REQUEST);
    let nobody = harness
        .bare(
            Method::POST,
            "/jellyfin/LiveStreams/Close?liveStreamId=abc",
            "",
        )
        .await;
    assert_eq!(nobody.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn media_sources_never_ask_to_be_opened() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let info = harness
        .post(
            &format!(
                "/jellyfin/Items/{}/PlaybackInfo",
                jellyfin_id("Item", DOODLEBUG)
            ),
            &token,
            browser(120_000_000),
        )
        .await
        .json();
    for source in info["MediaSources"].as_array().unwrap() {
        assert_eq!(source["RequiresOpening"], false);
        assert!(source.get("OpenToken").is_none_or(Value::is_null));
    }
}
