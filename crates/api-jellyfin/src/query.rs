//! Query parameters, read without regard to the case of their names.
//!
//! The reference is an ASP.NET server, whose binding ignores case: Jellyfin Web sends
//! `ParentId`, the newer SDK `parentId`, and both mean the same.

use crate::error::JellyfinError;

#[derive(Debug, Default, Clone)]
pub struct Query {
    /// Names in lower case, in the order they came; a repeated name keeps every value.
    pairs: Vec<(String, String)>,
}

impl Query {
    pub fn parse(raw: Option<&str>) -> Self {
        Self {
            pairs: raw
                .map(|raw| {
                    form_urlencoded::parse(raw.as_bytes())
                        .map(|(name, value)| (name.to_ascii_lowercase(), value.into_owned()))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Sets a parameter, in place of what the request gave it.
    pub fn set(&mut self, name: &str, value: &str) {
        let name = name.to_ascii_lowercase();
        self.pairs.retain(|(key, _)| *key != name);
        self.pairs.push((name, value.to_owned()));
    }

    /// The first non-empty value of a parameter.
    pub fn get(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.pairs
            .iter()
            .find(|(key, value)| *key == name && !value.is_empty())
            .map(|(_, value)| value.as_str())
    }

    /// A comma-separated list, gathered across repetitions, without empty entries.
    pub fn list(&self, name: &str) -> Vec<&str> {
        let name = name.to_ascii_lowercase();
        self.pairs
            .iter()
            .filter(|(key, _)| *key == name)
            .flat_map(|(_, value)| value.split(','))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect()
    }

    /// `true`/`false` in any case, as ASP.NET binds a boolean.
    ///
    /// # Errors
    ///
    /// [`JellyfinError::BadRequest`] for any other value.
    pub fn flag(&self, name: &str) -> Result<Option<bool>, JellyfinError> {
        match self.get(name) {
            None => Ok(None),
            Some(value) if value.eq_ignore_ascii_case("true") => Ok(Some(true)),
            Some(value) if value.eq_ignore_ascii_case("false") => Ok(Some(false)),
            Some(_) => Err(JellyfinError::BadRequest(
                "a flag is neither true nor false",
            )),
        }
    }

    /// A non-negative integer.
    ///
    /// # Errors
    ///
    /// [`JellyfinError::BadRequest`] for what is no such number.
    pub fn count(&self, name: &str) -> Result<Option<u32>, JellyfinError> {
        self.get(name)
            .map(|value| {
                value
                    .parse::<u32>()
                    .map_err(|_| JellyfinError::BadRequest("a count is not a number"))
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_read_without_regard_to_case() {
        let query = Query::parse(Some("ParentId=a&includeItemTypes=Movie&LIMIT=5"));
        assert_eq!(query.get("parentId"), Some("a"));
        assert_eq!(query.get("IncludeItemTypes"), Some("Movie"));
        assert_eq!(query.count("Limit").ok().flatten(), Some(5));
    }

    #[test]
    fn lists_split_on_commas_and_gather_repetitions() {
        let query = Query::parse(Some("Fields=Overview,Path&fields=Genres&SortBy=&Ids=a%2Cb"));
        assert_eq!(query.list("fields"), ["Overview", "Path", "Genres"]);
        assert!(query.list("sortBy").is_empty());
        assert_eq!(query.list("ids"), ["a", "b"]);
    }

    #[test]
    fn an_empty_value_counts_as_absent() {
        let query = Query::parse(Some("ParentId=&ParentId=b"));
        assert_eq!(query.get("parentid"), Some("b"));
        assert_eq!(Query::parse(None).get("x"), None);
    }

    #[test]
    fn flags_are_true_or_false_in_any_case() {
        let query = Query::parse(Some("Recursive=TRUE&IsPlayed=false&Bad=1"));
        assert_eq!(query.flag("recursive").ok().flatten(), Some(true));
        assert_eq!(query.flag("isPlayed").ok().flatten(), Some(false));
        assert_eq!(query.flag("missing").ok().flatten(), None);
        assert!(query.flag("bad").is_err());
    }

    #[test]
    fn a_count_is_a_non_negative_number() {
        let query = Query::parse(Some("StartIndex=-1&Limit=x"));
        assert!(query.count("startIndex").is_err());
        assert!(query.count("limit").is_err());
    }
}
