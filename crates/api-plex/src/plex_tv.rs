//! plex.tv, as the account service behind the Plex extension.
//!
//! Three calls: spending a claim code, announcing where a server is, and asking whose a token
//! is. The URLs carry codes and the requests carry tokens, so an error never keeps its URL and
//! a redirect is never followed.

use std::time::Duration;

use open_plex_core::access::{AccessError, AccountDirectory, AccountLink, RemoteAccount};
use reqwest::{redirect, Client, RequestBuilder, Response, StatusCode};
use serde::Deserialize;

const CLAIM_EXCHANGE: &str = "https://plex.tv/api/claim/exchange";
const ACCOUNT: &str = "https://plex.tv/api/v2/user";
const DEVICES: &str = "https://servers.plex.tv/api/v2/devices";
/// Short enough that a pairing, two calls in a row, stays inside the server's request timeout.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
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
        if response.status().is_success() {
            Ok(())
        } else {
            Err(refused_or_unreachable(response.status()))
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
