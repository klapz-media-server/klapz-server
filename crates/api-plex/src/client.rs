//! Client identity, carried by the `X-Plex-*` header family.
//!
//! Every value may arrive as a header or as a query parameter of the same name: the bundled
//! web client sends them all in the query string, and playback URLs are replayed verbatim by
//! players that cannot set headers. The lookup rules below were measured against the
//! reference server, through a parameter whose effect is observable (`X-Plex-Platform` on the
//! transcode decision):
//!
//! - a header, **even an empty one**, shadows the query parameter;
//! - header names are case-insensitive, as HTTP requires, but query names are not:
//!   `x-plex-platform=` is ignored;
//! - a repeated query parameter takes its last value;
//! - values are not trimmed.

use std::borrow::Cow;
use std::convert::Infallible;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::HeaderMap;

/// The opaque identifier a client chooses for itself.
///
/// Stable across a client's requests, which is what will key playback sessions and
/// timelines.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientId(String);

impl ClientId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Who is calling, as the client describes itself.
///
/// Every field is optional, the identifier included. The spec marks
/// `X-Plex-Client-Identifier` required on the transcode and timeline operations, but the
/// reference serves them without it; what it does refuse is a decision with no resolvable
/// client profile, which is a transcode concern, not an identity one. None of this is
/// authenticated, so none of it may drive an authorization decision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientIdentity {
    pub identifier: Option<ClientId>,
    pub product: Option<String>,
    pub version: Option<String>,
    pub platform: Option<String>,
    pub platform_version: Option<String>,
    pub device: Option<String>,
    pub model: Option<String>,
    pub device_vendor: Option<String>,
    pub device_name: Option<String>,
    pub marketplace: Option<String>,
    /// The language the client's interface is in, `fr` or `en-US`: the server translates
    /// the strings it writes itself, hub titles among them.
    pub language: Option<String>,
}

impl ClientIdentity {
    fn from_request(headers: &HeaderMap, query: Option<&str>) -> Self {
        let query: Vec<(Cow<'_, str>, Cow<'_, str>)> = query
            .map(|query| form_urlencoded::parse(query.as_bytes()).collect())
            .unwrap_or_default();
        let field = |name: &str| -> Option<String> {
            let value = match headers.get(name) {
                Some(header) => decode_header(header.as_bytes()),
                None => query
                    .iter()
                    .rev()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.clone().into_owned())?,
            };
            (!value.is_empty()).then_some(value)
        };

        Self {
            identifier: field("X-Plex-Client-Identifier").map(ClientId),
            product: field("X-Plex-Product"),
            version: field("X-Plex-Version"),
            platform: field("X-Plex-Platform"),
            platform_version: field("X-Plex-Platform-Version"),
            device: field("X-Plex-Device"),
            model: field("X-Plex-Model"),
            device_vendor: field("X-Plex-Device-Vendor"),
            device_name: field("X-Plex-Device-Name"),
            marketplace: field("X-Plex-Marketplace"),
            language: field("X-Plex-Language"),
        }
    }
}

/// Decodes a header value as UTF-8, falling back to ISO-8859-1.
///
/// HTTP has no standard encoding for non-ASCII header values, and the spec says the server
/// recognizes both. Device names are where it shows: `Salon télé` arrives either way. The
/// fallback cannot fail, since every byte is a Latin-1 code point.
fn decode_header(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().copied().map(char::from).collect(),
    }
}

impl<S: Sync> FromRequestParts<S> for ClientIdentity {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self::from_request(&parts.headers, parts.uri.query()))
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use axum::body::Body;
    use axum::http::{HeaderValue, Request};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    use super::*;

    fn identity(headers: &[(&str, &[u8])], query: Option<&str>) -> ClientIdentity {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_bytes(value).unwrap(),
            );
        }
        ClientIdentity::from_request(&map, query)
    }

    #[test]
    fn reads_every_field_from_the_query_string() {
        let identity = identity(
            &[],
            Some(
                "X-Plex-Client-Identifier=abc123&X-Plex-Product=Plex%20Web\
                 &X-Plex-Version=4.1&X-Plex-Platform=Chrome&X-Plex-Platform-Version=120.0\
                 &X-Plex-Device=Windows&X-Plex-Model=hosted&X-Plex-Device-Vendor=Microsoft\
                 &X-Plex-Device-Name=Living%20Room&X-Plex-Marketplace=googlePlay\
                 &X-Plex-Language=fr",
            ),
        );
        assert_eq!(
            identity,
            ClientIdentity {
                identifier: Some(ClientId("abc123".to_owned())),
                product: Some("Plex Web".to_owned()),
                version: Some("4.1".to_owned()),
                platform: Some("Chrome".to_owned()),
                platform_version: Some("120.0".to_owned()),
                device: Some("Windows".to_owned()),
                model: Some("hosted".to_owned()),
                device_vendor: Some("Microsoft".to_owned()),
                device_name: Some("Living Room".to_owned()),
                marketplace: Some("googlePlay".to_owned()),
                language: Some("fr".to_owned()),
            }
        );
    }

    #[test]
    fn requires_nothing() {
        // The reference serves even the operations the spec marks as requiring an
        // identifier without one.
        assert_eq!(identity(&[], None), ClientIdentity::default());
    }

    #[test]
    fn prefers_the_header_over_the_query_string() {
        let identity = identity(
            &[("X-Plex-Platform", b"Chrome")],
            Some("X-Plex-Platform=Safari"),
        );
        assert_eq!(identity.platform.as_deref(), Some("Chrome"));
    }

    #[test]
    fn lets_an_empty_header_shadow_the_query_string() {
        // Measured: an empty `X-Plex-Platform` header beside `?X-Plex-Platform=Chrome` is
        // refused by the reference exactly as if no platform had been sent.
        let identity = identity(&[("X-Plex-Platform", b"")], Some("X-Plex-Platform=Chrome"));
        assert_eq!(identity.platform, None);
    }

    #[test]
    fn matches_header_names_in_any_case() {
        let identity = identity(&[("x-plex-client-identifier", b"abc")], None);
        assert_eq!(
            identity.identifier.as_ref().map(ClientId::as_str),
            Some("abc")
        );
    }

    #[test]
    fn matches_query_names_case_sensitively() {
        let identity = identity(&[], Some("x-plex-platform=Chrome"));
        assert_eq!(identity.platform, None);
    }

    #[test]
    fn takes_the_last_of_a_repeated_query_parameter() {
        let identity = identity(&[], Some("X-Plex-Platform=Safari&X-Plex-Platform=Chrome"));
        assert_eq!(identity.platform.as_deref(), Some("Chrome"));
    }

    #[test]
    fn treats_an_empty_value_as_absent() {
        let identity = identity(&[], Some("X-Plex-Client-Identifier="));
        assert_eq!(identity.identifier, None);
    }

    #[test]
    fn keeps_surrounding_whitespace() {
        // The reference does not trim: ` Chrome ` is not a platform it knows.
        let identity = identity(&[], Some("X-Plex-Platform=%20Chrome%20"));
        assert_eq!(identity.platform.as_deref(), Some(" Chrome "));
    }

    #[test]
    fn decodes_a_header_sent_as_utf8() {
        let identity = identity(&[("X-Plex-Device-Name", "Salon télé".as_bytes())], None);
        assert_eq!(identity.device_name.as_deref(), Some("Salon télé"));
    }

    #[test]
    fn decodes_a_header_sent_as_latin1() {
        let identity = identity(&[("X-Plex-Device-Name", b"Salon t\xe9l\xe9")], None);
        assert_eq!(identity.device_name.as_deref(), Some("Salon télé"));
    }

    #[tokio::test]
    async fn extracts_from_a_real_request() {
        async fn echo(identity: ClientIdentity) -> String {
            format!(
                "{}|{}",
                identity.identifier.as_ref().map_or("-", ClientId::as_str),
                identity.product.as_deref().unwrap_or("-"),
            )
        }

        let response = Router::new()
            .route("/probe", get(echo))
            .oneshot(
                Request::builder()
                    .uri("/probe?X-Plex-Product=Plex%20Web")
                    .header("X-Plex-Client-Identifier", "abc123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(body, "abc123|Plex Web");
    }
}
