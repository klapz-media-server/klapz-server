//! Reading Plex's media analysis out of its packed `extra_data` column.
//!
//! The column holds a JSON object of strings, each key prefixed by the component that wrote
//! it: `ma:` for media analysis, `ln:` for loudness. Every value is a string, numbers
//! included (`"ma:height":"1080"`), and a flag is `"1"` when set — `"0"` appears only for
//! the two keys where absence means "not analysed" (`hasScalingMatrix`, `anamorphic`).

use serde_json::{Map, Value};

/// One row's analysis. Empty when the row has none or it does not parse.
#[derive(Debug, Default)]
pub(crate) struct Analysis(Map<String, Value>);

impl Analysis {
    pub(crate) fn parse(packed: Option<&str>) -> Self {
        let Some(packed) = packed.filter(|text| !text.is_empty()) else {
            return Self::default();
        };
        if let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(packed) {
            Self(fields)
        } else {
            // Logged and skipped: one malformed row is not worth failing an import over, and
            // the track keeps everything its own columns say.
            tracing::warn!("unparseable extra_data, analysis skipped");
            Self::default()
        }
    }

    /// The value as text; an empty string is absent, as everywhere in the source.
    pub(crate) fn text(&self, key: &str) -> Option<String> {
        match self.0.get(key)? {
            Value::String(text) if text.is_empty() => None,
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }

    /// The value as text, an empty string included: a track whose file names it `""` is
    /// labelled differently from one it does not name at all.
    pub(crate) fn text_as_written(&self, key: &str) -> Option<String> {
        match self.0.get(key)? {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }

    pub(crate) fn int<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.text(key)?.parse().ok()
    }

    pub(crate) fn float(&self, key: &str) -> Option<f64> {
        self.text(key)?.parse().ok()
    }

    /// A flag written only when set.
    pub(crate) fn flag(&self, key: &str) -> bool {
        self.tristate(key).unwrap_or(false)
    }

    /// A flag whose absence means "not analysed" rather than false.
    pub(crate) fn tristate(&self, key: &str) -> Option<bool> {
        self.int::<i64>(key).map(|value| value != 0)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn reads_the_typed_values_out_of_strings() {
        let analysis = Analysis::parse(Some(
            r#"{"ma:height":"1080","ma:frameRate":"23.976","ma:profile":"high","ma:hasScalingMatrix":"0","ma:original":"1","ma:title":"","url":"ma%3Aheight=1080"}"#,
        ));
        assert_eq!(analysis.int::<i32>("ma:height"), Some(1080));
        assert_eq!(analysis.float("ma:frameRate"), Some(23.976));
        assert_eq!(analysis.text("ma:profile").as_deref(), Some("high"));
        assert_eq!(analysis.tristate("ma:hasScalingMatrix"), Some(false));
        assert_eq!(analysis.tristate("ma:anamorphic"), None);
        assert!(analysis.flag("ma:original"));
        assert!(!analysis.flag("ma:dub"));
        // An empty title is no title, unless it is asked for as written.
        assert_eq!(analysis.text("ma:title"), None);
        assert_eq!(analysis.text_as_written("ma:title").as_deref(), Some(""));
    }

    #[test]
    fn a_missing_or_malformed_column_is_an_empty_analysis() {
        for packed in [None, Some(""), Some("ma%3Aheight=1080"), Some("[1, 2]")] {
            assert_eq!(
                Analysis::parse(packed).text("ma:height"),
                None,
                "{packed:?}"
            );
        }
    }
}
