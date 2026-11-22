//! The keys klapz presents to metadata providers.
//!
//! The project's own keys ship in the source, the repo owner's decision, as Jellyfin ships its
//! own: TMDB and TVDB issue them to an application, and every installation of it shares them. A
//! variable set in the environment replaces a built-in key; an empty one is treated as unset, so
//! that a blank line left in a `.env` does not switch a provider off. A provider with no key at
//! all is simply not consulted.

use std::fmt;

/// TMDB's v4 read access token, sent as `Authorization: Bearer`: it serves the whole v3 API, and
/// a header stays out of the URLs that logs and proxies keep.
const TMDB_READ_TOKEN: &str = "eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiI3YzQ2ODUxZDViZDEzMTdjYmQyMGE0YTVjMjg4NjUwMCIsIm5iZiI6MTYxNzYzODYwNi42NTUsInN1YiI6IjYwNmIzNGNlMDljMjRjMDA0MGM5OTJmYSIsInNjb3BlcyI6WyJhcGlfcmVhZCJdLCJ2ZXJzaW9uIjoxfQ.PpMuHWW40HYwV_0NKQLTq9vSER_VONQSjyOXgDzSZjo";
/// TVDB's v4 project key.
const TVDB_API_KEY: &str = "28b4a951-d47b-4fb0-993e-2a7956ea3329";

const TMDB_TOKEN_VARIABLE: &str = "KLAPZ_TMDB_TOKEN";
const TVDB_KEY_VARIABLE: &str = "KLAPZ_TVDB_API_KEY";
/// A subscriber's PIN, which TVDB requires beside a user-supported key. Personal, so it never
/// has a built-in value.
const TVDB_PIN_VARIABLE: &str = "KLAPZ_TVDB_PIN";

/// Where a key came from, so that the server can say so at startup without showing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    BuiltIn,
    Environment,
}

#[derive(Clone)]
pub struct TmdbCredentials {
    read_token: String,
    source: KeySource,
}

impl TmdbCredentials {
    pub fn read_token(&self) -> &str {
        &self.read_token
    }

    pub fn source(&self) -> KeySource {
        self.source
    }
}

#[derive(Clone)]
pub struct TvdbCredentials {
    api_key: String,
    pin: Option<String>,
    source: KeySource,
}

impl TvdbCredentials {
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn pin(&self) -> Option<&str> {
        self.pin.as_deref()
    }

    pub fn source(&self) -> KeySource {
        self.source
    }
}

// Written by hand so that no key reaches a log through `{:?}`.
impl fmt::Debug for TmdbCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TmdbCredentials")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for TvdbCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TvdbCredentials")
            .field("source", &self.source)
            .field("pin", &self.pin.is_some())
            .finish_non_exhaustive()
    }
}

/// Every provider's credentials, `None` for a provider that has no key.
#[derive(Debug, Clone)]
pub struct ProviderCredentials {
    pub tmdb: Option<TmdbCredentials>,
    pub tvdb: Option<TvdbCredentials>,
}

impl ProviderCredentials {
    pub fn from_env() -> Self {
        Self::resolve(
            |name| std::env::var(name).ok(),
            TMDB_READ_TOKEN,
            TVDB_API_KEY,
        )
    }

    fn resolve(
        variable: impl Fn(&str) -> Option<String>,
        built_in_tmdb_token: &str,
        built_in_tvdb_key: &str,
    ) -> Self {
        let set = |name: &str| variable(name).filter(|value| !value.trim().is_empty());
        let key = |name: &str, default: &str| match set(name) {
            Some(value) => Some((value, KeySource::Environment)),
            None => (!default.is_empty()).then(|| (default.to_owned(), KeySource::BuiltIn)),
        };
        Self {
            tmdb: key(TMDB_TOKEN_VARIABLE, built_in_tmdb_token)
                .map(|(read_token, source)| TmdbCredentials { read_token, source }),
            tvdb: key(TVDB_KEY_VARIABLE, built_in_tvdb_key).map(|(api_key, source)| {
                TvdbCredentials {
                    api_key,
                    pin: set(TVDB_PIN_VARIABLE),
                    source,
                }
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::collections::HashMap;

    fn resolve(variables: &[(&str, &str)], token: &str, key: &str) -> ProviderCredentials {
        let variables: HashMap<String, String> = variables
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        ProviderCredentials::resolve(|name| variables.get(name).cloned(), token, key)
    }

    #[test]
    fn built_in_keys_apply_when_the_environment_sets_none() {
        let credentials = resolve(&[], "built-in-token", "built-in-key");
        let tmdb = credentials.tmdb.unwrap();
        assert_eq!(tmdb.read_token(), "built-in-token");
        assert_eq!(tmdb.source(), KeySource::BuiltIn);
        let tv_database = credentials.tvdb.unwrap();
        assert_eq!(tv_database.api_key(), "built-in-key");
        assert_eq!(tv_database.pin(), None);
    }

    #[test]
    fn the_environment_replaces_a_built_in_key() {
        let credentials = resolve(
            &[(TMDB_TOKEN_VARIABLE, "mine"), (TVDB_KEY_VARIABLE, "my-key")],
            "built-in-token",
            "built-in-key",
        );
        let tmdb = credentials.tmdb.unwrap();
        assert_eq!(tmdb.read_token(), "mine");
        assert_eq!(tmdb.source(), KeySource::Environment);
        assert_eq!(credentials.tvdb.unwrap().api_key(), "my-key");
    }

    #[test]
    fn a_blank_variable_keeps_the_built_in_key() {
        let credentials = resolve(&[(TMDB_TOKEN_VARIABLE, "  ")], "built-in-token", "");
        assert_eq!(credentials.tmdb.unwrap().source(), KeySource::BuiltIn);
    }

    #[test]
    fn a_provider_without_any_key_is_left_out() {
        let credentials = resolve(&[(TVDB_PIN_VARIABLE, "1234")], "", "");
        assert!(credentials.tmdb.is_none());
        assert!(credentials.tvdb.is_none());
    }

    #[test]
    fn the_pin_comes_from_the_environment_only() {
        let credentials = resolve(&[(TVDB_PIN_VARIABLE, "1234")], "", "built-in-key");
        assert_eq!(credentials.tvdb.unwrap().pin(), Some("1234"));
    }

    #[test]
    fn debug_output_shows_no_key() {
        let credentials = resolve(&[(TVDB_PIN_VARIABLE, "9876")], "secret-token", "secret-key");
        let shown = format!("{credentials:?}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(!shown.contains("9876"), "{shown}");
    }
}
