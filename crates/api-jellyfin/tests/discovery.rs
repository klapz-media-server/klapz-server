//! Recognizing the server, signing in and out, against what the reference Jellyfin answered.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use axum::http::{Method, StatusCode};
use common::{client_header, fixture, jellyfin_id, shape_differences, Harness, PASSWORD};
use serde_json::json;

const JSON: &str = "application/json; charset=utf-8";

#[tokio::test]
async fn the_public_info_has_the_reference_shape_and_names_a_jellyfin_server() {
    let harness = Harness::new().await;
    let answer = harness
        .send(
            Method::GET,
            "/jellyfin/System/Info/Public",
            &[("host", "192.168.1.4:32500".to_owned())],
            None,
        )
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.content_type(), Some(JSON));
    let info = answer.json();
    assert_eq!(
        shape_differences(&info, &fixture("system_info_public"), &[]),
        Vec::<String>::new()
    );
    assert_eq!(info["ProductName"], "Jellyfin Server");
    assert_eq!(info["Version"], "12.4.0");
    assert_eq!(info["Id"], Harness::server_id());
    assert_eq!(info["LocalAddress"], "http://192.168.1.4:32500/jellyfin");
}

#[tokio::test]
async fn the_answers_before_sign_in_are_the_references() {
    let harness = Harness::new().await;
    for (path, name) in [
        ("/jellyfin/System/Ping", "system_ping"),
        ("/jellyfin/Branding/Configuration", "branding_configuration"),
        ("/jellyfin/Users/Public", "users_public"),
    ] {
        let answer = harness.send(Method::GET, path, &[], None).await;
        assert_eq!(answer.status, StatusCode::OK, "{path}");
        assert_eq!(answer.content_type(), Some(JSON), "{path}");
        assert_eq!(answer.json(), fixture(name), "{path}");
    }
    // Quick Connect is not built: the one answer that differs from the reference's `true`.
    let quick_connect = harness
        .send(Method::GET, "/jellyfin/QuickConnect/Enabled", &[], None)
        .await;
    assert_eq!(fixture("quick_connect_enabled"), json!(true));
    assert_eq!(quick_connect.json(), json!(false));
}

#[tokio::test]
async fn paths_are_read_without_regard_to_case() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    for path in [
        "/jellyfin/Users/Me",
        "/jellyfin/users/me",
        "/jellyfin/USERS/ME",
    ] {
        assert_eq!(
            harness.get(path, &token).await.status,
            StatusCode::OK,
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_sign_in_answers_the_reference_shape_with_a_token_of_its_own() {
    let harness = Harness::new().await;
    let answer = harness.sign_in_as("Alice", "device-1").await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.content_type(), Some(JSON));
    let result = answer.json();
    // Dates of a last sign-in and activity are not kept; the cast receiver is the
    // reference's own setting; a request with no connection behind it has no peer.
    assert_eq!(
        shape_differences(
            &result,
            &fixture("authenticate_by_name"),
            &[
                "LastLoginDate",
                "LastActivityDate",
                "CastReceiverId",
                "RemoteEndPoint",
            ],
        ),
        Vec::<String>::new()
    );
    assert_eq!(result["User"]["Name"], "alice");
    assert_eq!(
        result["User"]["Id"],
        jellyfin_id("User", harness.alice.get())
    );
    assert_eq!(result["ServerId"], Harness::server_id());
    assert_eq!(result["User"]["ServerId"], Harness::server_id());
    assert_eq!(result["SessionInfo"]["DeviceId"], "device-1");
    assert_eq!(result["SessionInfo"]["Client"], "Jellyfin Web");
    assert_eq!(result["User"]["Policy"]["IsAdministrator"], false);
    assert!(result["AccessToken"].as_str().unwrap().len() >= 32);
}

#[tokio::test]
async fn refuses_a_user_without_a_jellyfin_extension() {
    let harness = Harness::new().await;
    // bob is connected through Plex only.
    let answer = harness.sign_in_as("bob", "device-1").await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert!(answer.body.is_empty());
}

#[tokio::test]
async fn refuses_a_user_whose_jellyfin_extension_is_switched_off() {
    let harness = Harness::new().await;
    assert_eq!(
        harness.sign_in_as("carol", "device-1").await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn refuses_a_wrong_password_and_an_unknown_user_alike() {
    let harness = Harness::new().await;
    for (username, password) in [("alice", "wrong horse"), ("nobody", PASSWORD)] {
        let answer = harness
            .send(
                Method::POST,
                "/jellyfin/Users/AuthenticateByName",
                &[("authorization", client_header("device-1", None))],
                Some(json!({ "Username": username, "Pw": password })),
            )
            .await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{username}");
    }
}

#[tokio::test]
async fn a_sign_in_without_the_client_header_or_credentials_is_a_bad_request() {
    let harness = Harness::new().await;
    let anonymous = harness
        .send(
            Method::POST,
            "/jellyfin/Users/AuthenticateByName",
            &[],
            Some(json!({ "Username": "alice", "Pw": PASSWORD })),
        )
        .await;
    assert_eq!(anonymous.status, StatusCode::BAD_REQUEST);
    let garbled = harness
        .send(
            Method::POST,
            "/jellyfin/Users/AuthenticateByName",
            &[("authorization", client_header("device-1", None))],
            Some(json!({ "Name": 3 })),
        )
        .await;
    assert_eq!(garbled.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn no_caller_gets_a_bare_401_past_the_public_paths() {
    let harness = Harness::new().await;
    for path in [
        "/jellyfin/System/Info",
        "/jellyfin/Users/Me",
        "/jellyfin/UserViews",
    ] {
        let answer = harness.send(Method::GET, path, &[], None).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{path}");
        assert!(answer.body.is_empty(), "{path}");
    }
}

#[tokio::test]
async fn the_signed_in_user_reads_itself_with_the_reference_shape() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let me = harness.get("/jellyfin/Users/Me", &token).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(
        shape_differences(
            &me.json(),
            &fixture("users_me"),
            &["LastLoginDate", "LastActivityDate", "CastReceiverId"],
        ),
        Vec::<String>::new()
    );
    let own = harness
        .get(
            &format!(
                "/jellyfin/Users/{}",
                jellyfin_id("User", harness.alice.get())
            ),
            &token,
        )
        .await;
    assert_eq!(own.json(), me.json());
}

#[tokio::test]
async fn another_user_is_not_found() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let other = jellyfin_id("User", harness.alice.get() + 1);
    let answer = harness
        .get(&format!("/jellyfin/Users/{other}"), &token)
        .await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND);
    assert_eq!(answer.json()["status"], 404);
    let garbled = harness.get("/jellyfin/Users/zzz", &token).await;
    assert_eq!(garbled.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_system_info_has_the_reference_shape_once_signed_in() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let answer = harness.get("/jellyfin/System/Info", &token).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(
        shape_differences(&answer.json(), &fixture("system_info"), &[]),
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn signing_out_ends_the_devices_token_only() {
    let harness = Harness::new().await;
    let first = harness.token().await;
    let other = harness.sign_in_as("alice", "device-2").await.json()["AccessToken"]
        .as_str()
        .unwrap()
        .to_owned();
    let answer = harness
        .send(
            Method::POST,
            "/jellyfin/Sessions/Logout",
            &[("authorization", client_header("device-1", Some(&first)))],
            None,
        )
        .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert_eq!(
        harness.get("/jellyfin/Users/Me", &first).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        harness.get("/jellyfin/Users/Me", &other).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_token_travels_in_the_query_string_and_legacy_headers_too() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    for (uri, headers) in [
        (format!("/jellyfin/Users/Me?ApiKey={token}"), vec![]),
        (format!("/jellyfin/Users/Me?api_key={token}"), vec![]),
        (
            "/jellyfin/Users/Me".to_owned(),
            vec![("x-emby-token", token.clone())],
        ),
    ] {
        let answer = harness.send(Method::GET, &uri, &headers, None).await;
        assert_eq!(answer.status, StatusCode::OK, "{uri}");
    }
}

#[tokio::test]
async fn display_preferences_are_the_references_defaults() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let answer = harness
        .get(
            "/jellyfin/DisplayPreferences/usersettings?client=emby",
            &token,
        )
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    let preferences = answer.json();
    assert_eq!(
        shape_differences(
            &preferences,
            &fixture("display_preferences"),
            &[
                "chromecastVersion",
                "skipForwardLength",
                "skipBackLength",
                "enableNextVideoInfoOverlay",
                "tvhome",
                "dashboardTheme",
            ]
        ),
        Vec::<String>::new()
    );
    assert_eq!(preferences["Client"], "emby");
    let kept = harness
        .post(
            "/jellyfin/DisplayPreferences/usersettings",
            &token,
            json!({}),
        )
        .await;
    assert_eq!(kept.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn an_unknown_route_is_not_found() {
    let harness = Harness::new().await;
    let token = harness.token().await;
    let answer = harness.get("/jellyfin/Nothing/Here", &token).await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND);
}
