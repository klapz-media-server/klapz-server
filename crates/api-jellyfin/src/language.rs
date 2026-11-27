//! Track languages as Jellyfin writes them: ISO 639-2 terminological codes (`fra`, `deu`), as
//! measured on the reference, and their English names in labels.

use klapz_core::language::{Language, LanguageTag};

/// The three-letter code of a language in any spelling, region dropped: `fr`, `fre` and
/// `fr-CA` are `fra`. What names no language the list holds (`qaa`, teletext's `fra,fra`) is
/// published as written.
pub fn three_letters(written: &str) -> &str {
    LanguageTag::parse(written).map_or(written, |tag| code_of(tag).unwrap_or(written))
}

/// The three-letter code of a tag; none for a code the list does not hold.
pub fn code_of(tag: LanguageTag) -> Option<&'static str> {
    let language = tag.language();
    if language == Language::Mis && tag.code() != Language::Mis.code() {
        return None;
    }
    language.terminological_code()
}

/// The English name the reference publishes as a track's `LocalizedLanguage`, for a
/// three-letter code: none for one it does not know (`qaa`, `fra,fra`) or that names no
/// language but `und`.
pub fn localized_name(code: &str) -> Option<&'static str> {
    if code.eq_ignore_ascii_case("und") {
        return Some("Undetermined");
    }
    Language::from_code(code)
        .filter(|language| language.is_language())
        .filter(|language| {
            language
                .terminological_code()
                .is_some_and(|own| own.eq_ignore_ascii_case(code))
        })
        .map(Language::name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_letter_tags_take_their_terminological_code() {
        assert_eq!(three_letters("fr"), "fra");
        assert_eq!(three_letters("de"), "deu");
        assert_eq!(three_letters("en"), "eng");
        assert_eq!(three_letters("ja"), "jpn");
    }

    #[test]
    fn a_region_is_dropped_and_bibliographic_codes_become_terminological() {
        assert_eq!(three_letters("fr-CA"), "fra");
        assert_eq!(three_letters("pt_BR"), "por");
        assert_eq!(three_letters("fre"), "fra");
        assert_eq!(three_letters("GER"), "deu");
        assert_eq!(three_letters("eng"), "eng");
    }

    #[test]
    fn an_unknown_tag_is_published_as_stored() {
        assert_eq!(three_letters("qaa"), "qaa");
        assert_eq!(three_letters("fra,fra"), "fra,fra");
        assert_eq!(three_letters("und"), "und");
    }

    #[test]
    fn undetermined_is_named_and_an_unknown_code_is_not() {
        assert_eq!(localized_name("und"), Some("Undetermined"));
        assert_eq!(localized_name("fra"), Some("French"));
        assert_eq!(localized_name("qaa"), None);
        assert_eq!(localized_name("mul"), None);
        assert_eq!(localized_name("fra,fra"), None);
    }

    #[test]
    fn a_language_is_named_in_english() {
        assert_eq!(localized_name("eng"), Some("English"));
        assert_eq!(localized_name("deu"), Some("German"));
        assert_eq!(localized_name("nob"), Some("Norwegian Bokmål"));
    }
}
