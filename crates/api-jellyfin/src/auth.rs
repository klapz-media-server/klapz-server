//! How a Jellyfin client says who it is.
//!
//! Every request carries an `Authorization` header in Jellyfin's own scheme:
//!
//! ```text
//! Authorization: MediaBrowser Client="Jellyfin Web", Device="Firefox", DeviceId="…",
//!                Version="10.11.0", Token="…"
//! ```
//!
//! without the token until the client signed in. The reference (12.4) reads the token from
//! there or from an `ApiKey` query parameter, which is how stream and image URLs carry it.
//! Its legacy forms — `X-Emby-Authorization`, the `Emby` scheme, `X-Emby-Token`,
//! `X-MediaBrowser-Token` and `api_key` — it refuses unless told otherwise, and earlier
//! releases accepted them by default: older clients still send nothing else, so they are
//! accepted here (`docs/jellyfin/spec-gaps.md`).
//!
//! Parsing lives here because it is Jellyfin's protocol; the server's authentication reads
//! the token through [`token`] and resolves it.

use axum::http::{header, HeaderMap};

/// What a client says about itself in its authorization header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientInfo {
    pub client: Option<String>,
    pub device: Option<String>,
    pub device_id: Option<String>,
    pub version: Option<String>,
    pub token: Option<String>,
}

/// The header names the authorization travels under, the current one first.
const AUTHORIZATION_HEADERS: [&str; 2] = ["authorization", "x-emby-authorization"];
/// Header names that carry the token alone.
const TOKEN_HEADERS: [&str; 2] = ["x-emby-token", "x-mediabrowser-token"];
/// Query parameters that carry the token alone, compared without regard to case.
const TOKEN_PARAMETERS: [&str; 2] = ["apikey", "api_key"];

impl ClientInfo {
    /// The client's parameters, from the first authorization header in Jellyfin's scheme.
    pub fn of(headers: &HeaderMap) -> Self {
        AUTHORIZATION_HEADERS
            .iter()
            .flat_map(|name| headers.get_all(*name))
            .filter_map(|value| value.to_str().ok())
            .find_map(parse)
            .unwrap_or_default()
    }
}

/// The parameters of a `MediaBrowser` (or legacy `Emby`) authorization value; `None` for
/// another scheme, such as a session's `Bearer`.
pub fn parse(value: &str) -> Option<ClientInfo> {
    let (scheme, parameters) = value.trim().split_once(' ').unwrap_or((value.trim(), ""));
    if !scheme.eq_ignore_ascii_case("mediabrowser") && !scheme.eq_ignore_ascii_case("emby") {
        return None;
    }
    let mut info = ClientInfo::default();
    for parameter in parameters.split(',') {
        let Some((name, value)) = parameter.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(value);
        // Jellyfin's SDK escapes each value as a URI component.
        let value = percent_decoded(value);
        if value.is_empty() {
            continue;
        }
        let slot = match name.trim().to_ascii_lowercase().as_str() {
            "client" => &mut info.client,
            "device" => &mut info.device,
            "deviceid" => &mut info.device_id,
            "version" => &mut info.version,
            "token" => &mut info.token,
            _ => continue,
        };
        *slot = Some(value);
    }
    Some(info)
}

fn percent_decoded(value: &str) -> String {
    if !value.contains('%') {
        return value.to_owned();
    }
    form_urlencoded::parse(format!("v={}", value.replace('+', "%2B")).as_bytes())
        .next()
        .map_or_else(|| value.to_owned(), |(_, decoded)| decoded.into_owned())
}

/// The token a request presents, wherever it put it: the authorization header first, then a
/// header of its own, then the query string.
pub fn token(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    if let Some(token) = ClientInfo::of(headers).token {
        return Some(token);
    }
    let from_header = TOKEN_HEADERS
        .iter()
        .filter_map(|name| headers.get(*name))
        .filter_map(|value| value.to_str().ok())
        .map(str::trim)
        .find(|value| !value.is_empty());
    if let Some(token) = from_header {
        return Some(token.to_owned());
    }
    form_urlencoded::parse(query?.as_bytes())
        .find(|(name, value)| {
            TOKEN_PARAMETERS
                .iter()
                .any(|known| name.eq_ignore_ascii_case(known))
                && !value.is_empty()
        })
        .map(|(_, value)| value.into_owned())
}

/// Whether a request carries an authorization header in Jellyfin's scheme. The reference
/// refuses a sign-in without one `400`: it is where the device names itself.
pub fn has_client_header(headers: &HeaderMap) -> bool {
    AUTHORIZATION_HEADERS
        .iter()
        .flat_map(|name| headers.get_all(*name))
        .filter_map(|value| value.to_str().ok())
        .any(|value| parse(value).is_some())
}

/// Paths of the Jellyfin surface, below its base, that answer without a token: what a client
/// reads before it signs in, the sign-in itself, and the base, which redirects to Jellyfin Web. Compared without regard to case, as the
/// reference routes.
pub fn is_public(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    let path = path.trim_end_matches('/');
    matches!(
        path,
        "/system/info/public"
            | "/system/ping"
            | "/users/authenticatebyname"
            | "/users/public"
            | "/branding/configuration"
            | "/branding/css"
            | "/branding/css.css"
            | "/quickconnect/enabled"
    ) || path.is_empty()
        || path == "/web"
        || path.starts_with("/web/")
        // Images, as on the reference; the handler decides what a request without a token may
        // have (`images`).
        || crate::images::is_image_path(path)
}

/// Whether the `Authorization` header is Jellyfin's own, and so not a session's bearer.
pub fn is_jellyfin_authorization(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| parse(value).is_some())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use axum::http::HeaderValue;

    use super::*;

    const WEB: &str = r#"MediaBrowser Client="Jellyfin Web", Device="Firefox", DeviceId="TW96aWxsYQ", Version="10.11.0", Token="abc123""#;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(*name, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn reads_every_parameter_of_a_mediabrowser_header() {
        assert_eq!(
            parse(WEB).unwrap(),
            ClientInfo {
                client: Some("Jellyfin Web".to_owned()),
                device: Some("Firefox".to_owned()),
                device_id: Some("TW96aWxsYQ".to_owned()),
                version: Some("10.11.0".to_owned()),
                token: Some("abc123".to_owned()),
            }
        );
    }

    #[test]
    fn reads_unquoted_reordered_and_escaped_values() {
        let info = parse(
            "mediabrowser token=abc, deviceid=a%2Fb%3D, Device=Living%20room, client=\"Infuse\"",
        )
        .unwrap();
        assert_eq!(info.token.as_deref(), Some("abc"));
        assert_eq!(info.device_id.as_deref(), Some("a/b="));
        assert_eq!(info.device.as_deref(), Some("Living room"));
        assert_eq!(info.client.as_deref(), Some("Infuse"));
        assert_eq!(info.version, None);
    }

    #[test]
    fn a_header_before_sign_in_carries_no_token() {
        let info = parse(r#"MediaBrowser Client="Jellyfin Web", Token="""#).unwrap();
        assert_eq!(info.token, None);
        assert_eq!(info.client.as_deref(), Some("Jellyfin Web"));
    }

    #[test]
    fn another_scheme_is_not_jellyfins() {
        assert_eq!(parse("Bearer eyJhbGciOi"), None);
        assert_eq!(parse("Basic dXNlcjpwYXNz"), None);
        assert!(!is_jellyfin_authorization(&headers(&[(
            "authorization",
            "Bearer x"
        )])));
        assert!(is_jellyfin_authorization(&headers(&[(
            "authorization",
            WEB
        )])));
    }

    #[test]
    fn the_legacy_emby_scheme_and_header_are_read_too() {
        let legacy = headers(&[("x-emby-authorization", r#"Emby Token="old""#)]);
        assert_eq!(token(&legacy, None).as_deref(), Some("old"));
    }

    #[test]
    fn finds_the_token_in_the_header_then_its_own_headers_then_the_query() {
        assert_eq!(
            token(&headers(&[("authorization", WEB)]), Some("ApiKey=other")).as_deref(),
            Some("abc123")
        );
        assert_eq!(
            token(&headers(&[("x-emby-token", "t1")]), Some("ApiKey=other")).as_deref(),
            Some("t1")
        );
        assert_eq!(
            token(&headers(&[("x-mediabrowser-token", "t2")]), None).as_deref(),
            Some("t2")
        );
        for query in [
            "ApiKey=t3",
            "apikey=t3",
            "api_key=t3",
            "static=true&API_KEY=t3",
        ] {
            assert_eq!(
                token(&HeaderMap::new(), Some(query)).as_deref(),
                Some("t3"),
                "{query}"
            );
        }
    }

    #[test]
    fn no_token_anywhere_is_none() {
        assert_eq!(token(&HeaderMap::new(), None), None);
        assert_eq!(token(&HeaderMap::new(), Some("ApiKey=&x=1")), None);
        let anonymous = headers(&[("authorization", r#"MediaBrowser Client="Web""#)]);
        assert_eq!(token(&anonymous, Some("static=true")), None);
        assert_eq!(
            token(&headers(&[("authorization", "Bearer session")]), None),
            None
        );
    }

    #[test]
    fn a_sign_in_needs_the_client_header() {
        assert!(has_client_header(&headers(&[(
            "authorization",
            r#"MediaBrowser Client="Web""#
        )])));
        assert!(!has_client_header(&HeaderMap::new()));
        assert!(!has_client_header(&headers(&[(
            "authorization",
            "Bearer x"
        )])));
    }

    #[test]
    fn the_public_paths_are_what_a_client_reads_before_signing_in() {
        for path in [
            "/System/Info/Public",
            "/system/info/public/",
            "/System/Ping",
            "/Users/AuthenticateByName",
            "/users/authenticatebyname",
            "/Users/Public",
            "/Branding/Configuration",
            "/Branding/Css.css",
            "/QuickConnect/Enabled",
            "/web/index.html",
            "/web",
            "",
            "/",
        ] {
            assert!(is_public(path), "{path}");
        }
        for path in [
            "/System/Info",
            "/Users/Me",
            "/Items",
            "/UserViews",
            "/webhook",
            "/System/Info/Publicity",
        ] {
            assert!(!is_public(path), "{path}");
        }
    }
}
