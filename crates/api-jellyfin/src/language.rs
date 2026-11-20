//! Track languages as Jellyfin writes them: ISO 639-2 terminological codes (`fra`, `deu`), as
//! measured on the reference, and their English names in labels.
//!
//! Our tracks keep the tag the file or the import gave, mostly ISO 639-1 (`fr`) and sometimes
//! already three letters. The table covers the languages of the reference's libraries and the
//! common ones beyond; a tag outside it is published as stored.

/// ISO 639-1, its ISO 639-2/T code, its ISO 639-2/B code where it differs, and the English
/// name .NET gives it.
const LANGUAGES: &[(&str, &str, Option<&str>, &str)] = &[
    ("ar", "ara", None, "Arabic"),
    ("bg", "bul", None, "Bulgarian"),
    ("ca", "cat", None, "Catalan"),
    ("cs", "ces", Some("cze"), "Czech"),
    ("da", "dan", None, "Danish"),
    ("de", "deu", Some("ger"), "German"),
    ("el", "ell", Some("gre"), "Greek"),
    ("en", "eng", None, "English"),
    ("es", "spa", None, "Spanish"),
    ("et", "est", None, "Estonian"),
    ("eu", "eus", Some("baq"), "Basque"),
    ("fa", "fas", Some("per"), "Persian"),
    ("fi", "fin", None, "Finnish"),
    ("fr", "fra", Some("fre"), "French"),
    ("he", "heb", None, "Hebrew"),
    ("hi", "hin", None, "Hindi"),
    ("hr", "hrv", None, "Croatian"),
    ("hu", "hun", None, "Hungarian"),
    ("id", "ind", None, "Indonesian"),
    ("is", "isl", Some("ice"), "Icelandic"),
    ("it", "ita", None, "Italian"),
    ("ja", "jpn", None, "Japanese"),
    ("ko", "kor", None, "Korean"),
    ("lt", "lit", None, "Lithuanian"),
    ("lv", "lav", None, "Latvian"),
    ("ms", "msa", Some("may"), "Malay"),
    ("nb", "nob", None, "Norwegian Bokmål"),
    ("nl", "nld", Some("dut"), "Dutch"),
    ("no", "nor", None, "Norwegian"),
    ("pl", "pol", None, "Polish"),
    ("pt", "por", None, "Portuguese"),
    ("ro", "ron", Some("rum"), "Romanian"),
    ("ru", "rus", None, "Russian"),
    ("sk", "slk", Some("slo"), "Slovak"),
    ("sl", "slv", None, "Slovenian"),
    ("sr", "srp", None, "Serbian"),
    ("sv", "swe", None, "Swedish"),
    ("th", "tha", None, "Thai"),
    ("tr", "tur", None, "Turkish"),
    ("uk", "ukr", None, "Ukrainian"),
    ("vi", "vie", None, "Vietnamese"),
    ("zh", "zho", Some("chi"), "Chinese"),
];

/// The three-letter code of a stored tag, region dropped: `fr-CA` is `fra`. A bibliographic
/// code (`fre`, `ger`) becomes the terminological one the reference writes.
pub fn three_letters(tag: &str) -> &str {
    let primary = tag.split(['-', '_']).next().unwrap_or(tag);
    LANGUAGES
        .iter()
        .find(|(two, three, bibliographic, _)| {
            primary.eq_ignore_ascii_case(two)
                || primary.eq_ignore_ascii_case(three)
                || bibliographic.is_some_and(|code| primary.eq_ignore_ascii_case(code))
        })
        .map_or(tag, |(_, three, _, _)| *three)
}

/// The English name of a three-letter code; a code it does not know, with its first letter
/// raised, as the reference names `qaa` `Qaa`.
pub fn english_name(code: &str) -> String {
    LANGUAGES
        .iter()
        .find(|(_, three, _, _)| code.eq_ignore_ascii_case(three))
        .map_or_else(
            || {
                let mut letters = code.chars();
                letters.next().map_or_else(String::new, |first| {
                    first.to_uppercase().chain(letters).collect()
                })
            },
            |(_, _, _, name)| (*name).to_owned(),
        )
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
    fn an_unknown_tag_is_published_as_stored_and_named_capitalized() {
        assert_eq!(three_letters("qaa"), "qaa");
        assert_eq!(english_name("qaa"), "Qaa");
    }

    #[test]
    fn labels_name_a_language_in_english() {
        assert_eq!(english_name("fra"), "French");
        assert_eq!(english_name("eng"), "English");
    }
}
