//! What talking to a provider shares: one client, a timeout, and how an answer is read.

use std::time::Duration;

use klapz_core::source_metadata::AgentError;

/// Long enough for a series with hundreds of episodes on one page, short enough that a
/// provider that stopped answering does not hold a library's matching for minutes.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A provider's answer above this is not metadata: refused before it is held in memory.
const MAX_BODY: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct Http {
    client: reqwest::Client,
}

impl Http {
    pub(crate) fn new() -> Result<Self, AgentError> {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(concat!("klapz/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| AgentError::Unreachable(e.to_string()))?;
        Ok(Self { client })
    }

    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Sends `request` and reads its JSON body. The error names no URL: a provider's key may
    /// travel in one.
    pub(crate) async fn json<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, AgentError> {
        let response = request
            .send()
            .await
            .map_err(|e| AgentError::Unreachable(e.without_url().to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(AgentError::Status(status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BODY as u64)
        {
            return Err(AgentError::Unreadable("answer too large".to_owned()));
        }
        let body = response
            .bytes()
            .await
            .map_err(|e| AgentError::Unreachable(e.without_url().to_string()))?;
        if body.len() > MAX_BODY {
            return Err(AgentError::Unreadable("answer too large".to_owned()));
        }
        serde_json::from_slice(&body).map_err(|e| AgentError::Unreadable(e.to_string()))
    }
}

/// Providers send `""` for a text they do not have.
pub(crate) fn empty_to_none(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
