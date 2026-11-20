//! Where a converted image comes from.
//!
//! The URL is the client's choice and the server is the one fetching it, which makes this an
//! SSRF surface: the reference will fetch from its own loopback when asked. So only Plex's
//! hosts are reachable, over TLS, on every hop of a redirect. Those are what Plex Web
//! actually needs: an account avatar lives on `plex.tv` and redirects to `assets.plex.tv`.

use std::time::Duration;

use reqwest::{redirect, Client, Url};

use crate::TranscodeError;

const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const PLEX_DOMAIN: &str = "plex.tv";

/// The URL to fetch, once the client's value has passed the policy.
pub(crate) fn remote_url(requested: &str) -> Result<Url, TranscodeError> {
    let url = Url::parse(requested).map_err(|_| TranscodeError::InvalidSource)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(TranscodeError::InvalidSource);
    }
    if !may_fetch(&url) {
        tracing::warn!("refused an image source outside Plex's hosts");
        return Err(TranscodeError::ForbiddenSource);
    }
    Ok(url)
}

/// `Url` has already resolved the tricks a string test would fall for: userinfo
/// (`https://plex.tv@evil.example/`), letter case, percent-encoded hosts.
fn may_fetch(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && url.host_str().is_some_and(|host| {
            host == PLEX_DOMAIN
                || host
                    .strip_suffix(PLEX_DOMAIN)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
}

pub(crate) fn client() -> Result<Client, TranscodeError> {
    Client::builder()
        .user_agent("klapz")
        .timeout(FETCH_TIMEOUT)
        // Stopping hands back the 3xx itself, which `fetch` treats as no image.
        .redirect(redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < MAX_REDIRECTS && may_fetch(attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|error| TranscodeError::Client(error.without_url()))
}

/// The cause of a failed fetch stays out of the logs: a `reqwest` error quotes its URL, and
/// this one may hold a token.
pub(crate) async fn fetch(client: &Client, url: Url) -> Result<Vec<u8>, TranscodeError> {
    let mut response = client
        .get(url)
        .send()
        .await
        .ok()
        .filter(|response| response.status().is_success())
        .ok_or(TranscodeError::UnavailableSource)?;

    let announced = response.content_length().unwrap_or(0);
    if usize::try_from(announced).map_or(true, |length| length > MAX_SOURCE_BYTES) {
        return Err(TranscodeError::SourceTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| TranscodeError::UnavailableSource)?
    {
        // Checked as it arrives: the announced length is only the sender's word.
        if body.len() + chunk.len() > MAX_SOURCE_BYTES {
            return Err(TranscodeError::SourceTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn allowed(url: &str) -> bool {
        may_fetch(&Url::parse(url).unwrap())
    }

    #[test]
    fn fetches_plex_hosts_over_tls() {
        assert!(allowed("https://plex.tv/users/abc/avatar?c=1&size=60"));
        assert!(allowed("https://assets.plex.tv/avatars/abc"));
        assert!(allowed("https://PLEX.TV/users/abc/avatar"));
    }

    #[test]
    fn refuses_everything_else() {
        for url in [
            "http://plex.tv/users/abc/avatar",
            "https://plex.tv:8443/x",
            "https://notplex.tv/x",
            "https://plex.tv.evil.example/x",
            "https://plex.tv@evil.example/x",
            "https://127.0.0.1/x",
            "https://[::1]/x",
            "https://169.254.169.254/latest/meta-data",
        ] {
            assert!(!allowed(url), "{url}");
        }
    }

    #[test]
    fn sorts_a_source_into_the_right_refusal() {
        assert!(matches!(
            remote_url("file:///C:/Windows/win.ini"),
            Err(TranscodeError::InvalidSource)
        ));
        assert!(matches!(
            remote_url("not a url"),
            Err(TranscodeError::InvalidSource)
        ));
        assert!(matches!(
            remote_url("http://127.0.0.1:32400/identity"),
            Err(TranscodeError::ForbiddenSource)
        ));
        assert!(remote_url("https://plex.tv/users/abc/avatar").is_ok());
    }
}
