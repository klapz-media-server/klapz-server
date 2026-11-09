//! plex.tv, as the account service behind the Plex extension.
//!
//! Three jobs: spending a claim code, announcing where a server is, and asking whose a token
//! is. The URLs carry codes and the requests carry tokens, so an error never keeps its URL and
//! a redirect is never followed.

use std::net::IpAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use open_plex_core::access::{AccessError, AccountDirectory, AccountLink, RemoteAccount};
use reqwest::{redirect, Client, RequestBuilder, Response, StatusCode};
use serde::Deserialize;

const CLAIM_EXCHANGE: &str = "https://plex.tv/api/claim/exchange";
const ACCOUNT: &str = "https://plex.tv/api/v2/user";
const DEVICES: &str = "https://servers.plex.tv/api/v2/devices";
const REGISTRATION: &str = "https://servers.plex.tv/servers.xml";
/// Short enough that a pairing, four calls in a row, stays inside the server's request timeout.
const CALL_TIMEOUT: Duration = Duration::from_secs(7);
/// Presented when no server is concerned, as plex.tv requires some client identifier.
const OWN_CLIENT_IDENTIFIER: &str = "open-plex";

#[derive(Debug, Clone)]
pub struct PlexTv {
    client: Client,
}

impl PlexTv {
    /// # Errors
    ///
    /// When the TLS backend cannot be set up.
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: Client::builder()
                .redirect(redirect::Policy::none())
                .timeout(CALL_TIMEOUT)
                .build()?,
        })
    }
}

/// What every call says about the caller. A claimed device is listed under these.
fn as_server(request: RequestBuilder, client_identifier: &str) -> RequestBuilder {
    request
        .header("X-Plex-Client-Identifier", client_identifier)
        .header("X-Plex-Product", "Plex Media Server")
        .header("X-Plex-Version", crate::general::PLEX_VERSION)
        .header("X-Plex-Device-Name", "open-plex")
        .header("X-Plex-Provides", "server")
        .header("X-Plex-Platform", std::env::consts::OS)
}

fn unreachable_because(error: reqwest::Error) -> AccessError {
    AccessError::Unreachable(Box::new(error.without_url()))
}

async fn send(request: RequestBuilder) -> Result<Response, AccessError> {
    request.send().await.map_err(unreachable_because)
}

/// A refusal is a client error; anything else unexpected says nothing about the request.
fn refused_or_unreachable(status: StatusCode) -> AccessError {
    if status.is_client_error() {
        AccessError::Refused
    } else {
        AccessError::Unreachable(format!("plex.tv answered {status}").into())
    }
}

/// The token in a claim exchange's answer, an element of the XML account document.
fn authentication_token(document: &str) -> Option<&str> {
    let (_, rest) = document.split_once("<authentication-token>")?;
    let (token, _) = rest.split_once("</authentication-token>")?;
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// What `localAddresses` can carry: the address's host, when that is an IP address.
fn local_address(connection_uri: &str) -> &str {
    let authority = connection_uri
        .split_once("://")
        .map_or(connection_uri, |(_, authority)| authority);
    let host = authority.split(':').next().unwrap_or(authority);
    if host.parse::<IpAddr>().is_ok() {
        host
    } else {
        ""
    }
}

/// The document the reference posts, minus the libraries it lists after `<Server>`. Every
/// interpolated value is a hexadecimal identifier, a number, or a host `is_connection_uri`
/// reduced to letters, digits, dots and dashes: none can need XML escaping.
fn registration(server_id: &str, local_address: &str, microseconds: u128) -> String {
    format!(
        "<MediaContainer size=\"0\">\n<Server name=\"open-plex\" host=\"\" \
         localAddresses=\"{local_address}\" machineIdentifier=\"{server_id}\" \
         version=\"{version}\" date=\"{microseconds}\" />\n</MediaContainer>\n",
        version = crate::general::PLEX_VERSION,
    )
}

#[derive(Deserialize)]
struct Account {
    id: i64,
    #[serde(default)]
    username: String,
    #[serde(default)]
    title: String,
}

/// A managed user has a title and no username.
fn account_of_document(document: &[u8], token: &str) -> Result<RemoteAccount, serde_json::Error> {
    let account: Account = serde_json::from_slice(document)?;
    Ok(RemoteAccount {
        id: account.id.to_string(),
        name: if account.username.is_empty() {
            account.title
        } else {
            account.username
        },
        token: token.to_owned(),
    })
}

#[async_trait::async_trait]
impl AccountDirectory for PlexTv {
    async fn redeem(&self, server_id: &str, code: &str) -> Result<RemoteAccount, AccessError> {
        let request = self.client.post(CLAIM_EXCHANGE).query(&[("token", code)]);
        let response = send(as_server(request, server_id)).await?;
        if !response.status().is_success() {
            return Err(refused_or_unreachable(response.status()));
        }
        let document = response.text().await.map_err(unreachable_because)?;
        let token = authentication_token(&document)
            .ok_or_else(|| AccessError::Unreachable("plex.tv returned no token".into()))?;
        self.account_of(token).await?.ok_or(AccessError::Refused)
    }

    async fn publish(&self, link: &AccountLink) -> Result<(), AccessError> {
        // The reference also sends `connections[0][prefixLength]`, its LAN's size: an address
        // meant for the Internet has none.
        let https_enabled = if link.connection_uri.starts_with("https://") {
            "1"
        } else {
            "0"
        };
        let request = self
            .client
            .put(format!("{DEVICES}/{}", link.server_id))
            .query(&[
                ("connections[0][uri]", link.connection_uri.as_str()),
                ("httpsEnabled", https_enabled),
                ("httpsRequired", "0"),
                ("dnsRebindingProtection", "0"),
            ])
            .header("X-Plex-Token", &link.account.token);
        let response = send(as_server(request, &link.server_id)).await?;
        if !response.status().is_success() {
            return Err(refused_or_unreachable(response.status()));
        }

        let microseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_micros());
        let request = self
            .client
            .post(REGISTRATION)
            .query(&[("auth_token", link.account.token.as_str())])
            .header("Content-Type", "application/xml")
            .body(registration(
                &link.server_id,
                local_address(&link.connection_uri),
                microseconds,
            ));
        let status = send(as_server(request, &link.server_id)).await?.status();
        // Without a public address this answers 422, to the reference as well, and it is
        // nevertheless what makes plex.tv hand clients a token for the server.
        if status.is_success() || status == StatusCode::UNPROCESSABLE_ENTITY {
            Ok(())
        } else {
            Err(refused_or_unreachable(status))
        }
    }

    async fn account_of(&self, token: &str) -> Result<Option<RemoteAccount>, AccessError> {
        let request = self
            .client
            .get(ACCOUNT)
            .header("Accept", "application/json")
            .header("X-Plex-Token", token);
        let response = send(as_server(request, OWN_CLIENT_IDENTIFIER)).await?;
        match response.status() {
            StatusCode::UNAUTHORIZED => return Ok(None),
            status if !status.is_success() => return Err(refused_or_unreachable(status)),
            _ => {}
        }
        let document = response.bytes().await.map_err(unreachable_because)?;
        account_of_document(&document, token)
            .map(Some)
            .map_err(|error| AccessError::Unreachable(Box::new(error)))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn finds_the_token_among_an_account_documents_elements() {
        // The order of plex.tv's /users/account.xml, values replaced.
        let document = r#"<?xml version="1.0" encoding="UTF-8"?>
<user email="a@example.org" id="7" authenticationToken="abc-DEF_1">
  <subscription active="1"/>
  <username>alice</username>
  <email>a@example.org</email>
  <joined-at type="datetime">2020-01-01 00:00:00 UTC</joined-at>
  <authentication-token>abc-DEF_1</authentication-token>
</user>"#;
        assert_eq!(authentication_token(document), Some("abc-DEF_1"));
    }

    #[test]
    fn a_document_without_a_token_yields_none() {
        assert_eq!(authentication_token("<errors><error/></errors>"), None);
        assert_eq!(
            authentication_token("<authentication-token> </authentication-token>"),
            None
        );
    }

    #[test]
    fn an_account_is_identified_by_its_number() {
        let account = account_of_document(
            br#"{"id":7,"uuid":"u","username":"alice","title":"Alice"}"#,
            "token",
        )
        .unwrap();
        assert_eq!(
            (
                account.id.as_str(),
                account.name.as_str(),
                account.token.as_str()
            ),
            ("7", "alice", "token")
        );
    }

    #[test]
    fn a_managed_user_goes_by_its_title() {
        let account =
            account_of_document(br#"{"id":7,"username":"","title":"Kids"}"#, "token").unwrap();
        assert_eq!(account.name, "Kids");
    }

    #[test]
    fn a_registration_has_the_shape_the_reference_posts() {
        assert_eq!(
            registration("abc123", "192.168.1.30", 1_794_257_565_727_563),
            format!(
                "<MediaContainer size=\"0\">\n<Server name=\"open-plex\" host=\"\" \
                 localAddresses=\"192.168.1.30\" machineIdentifier=\"abc123\" \
                 version=\"{}\" date=\"1794257565727563\" />\n</MediaContainer>\n",
                crate::general::PLEX_VERSION
            )
        );
    }

    #[test]
    fn only_an_ip_address_is_a_local_address() {
        assert_eq!(local_address("http://192.168.1.30:32500"), "192.168.1.30");
        assert_eq!(local_address("https://a.example.org:443"), "");
    }

    #[test]
    fn only_a_client_error_counts_as_a_refusal() {
        assert!(matches!(
            refused_or_unreachable(StatusCode::NOT_FOUND),
            AccessError::Refused
        ));
        assert!(matches!(
            refused_or_unreachable(StatusCode::BAD_GATEWAY),
            AccessError::Unreachable(_)
        ));
    }
}
