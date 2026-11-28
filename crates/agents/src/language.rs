//! TVDB names languages by ISO 639-2/T (`fra`), with two codes of its own.

use klapz_core::language::LanguageTag;

/// `fra` → `fr`. TVDB's `zhtw` is Taiwan's Chinese and its `pt` Brazil's Portuguese,
/// Portugal's being `por`.
pub(crate) fn from_tvdb(code: &str) -> Option<LanguageTag> {
    match code.to_ascii_lowercase().as_str() {
        "zhtw" => LanguageTag::parse("zh-TW"),
        "pt" => LanguageTag::parse("pt-BR"),
        code => LanguageTag::parse(code),
    }
}

/// `fr-FR` → `fra`: how TVDB names the language of a tag.
pub(crate) fn to_tvdb(tag: LanguageTag) -> Option<&'static str> {
    tag.language().terminological_code()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(code: &str) -> Option<String> {
        from_tvdb(code).map(|tag| tag.to_string())
    }

    #[test]
    fn maps_tvdb_codes() {
        assert_eq!(written("fra").as_deref(), Some("fr"));
        assert_eq!(written("eng").as_deref(), Some("en"));
        assert_eq!(written("jpn").as_deref(), Some("ja"));
        assert_eq!(written("zhtw").as_deref(), Some("zh-TW"));
        assert_eq!(written("pt").as_deref(), Some("pt-BR"));
        assert_eq!(written("yue").as_deref(), Some("yue"));
        assert_eq!(LanguageTag::parse("fr-FR").and_then(to_tvdb), Some("fra"));
    }
}
