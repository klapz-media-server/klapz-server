//! Languages: one model for every field that holds one, a track's, a library's, a provider's.
//!
//! A [`Language`] is one of the languages CLDR names, under its shortest ISO 639 code, and a
//! [`LanguageTag`] is a language with the script and region BCP 47 can add: `fr-CA`, `es-419`,
//! `zh-Hant`. Whatever spelling a file or a provider uses reads as the same language — `fre`,
//! `fra`, `fr` and `French` — while a region keeps two dubs apart: a TRUEFRENCH track is
//! `fr-FR`, a VFQ one `fr-CA`.

mod table;

use std::fmt;

pub use table::Language;
use table::{Entry, CODES, ENTRIES, NAMES};

impl Language {
    fn entry(self) -> &'static Entry {
        &ENTRIES[self as usize]
    }

    /// Its shortest code: `fr`, `fil`.
    #[must_use]
    pub fn code(self) -> &'static str {
        self.entry().code
    }

    /// ISO 639-2/T: `fra`. Every language of the table has one but those ISO 639-2 leaves
    /// out and 639-3 names in two letters, which none does.
    #[must_use]
    pub fn terminological_code(self) -> Option<&'static str> {
        self.entry().terminological
    }

    /// ISO 639-2/B, the code Matroska and MP4 write: `fre`, else the terminological one.
    #[must_use]
    pub fn bibliographic_code(self) -> Option<&'static str> {
        self.entry().bibliographic.or(self.entry().terminological)
    }

    /// In English, as CLDR names it: `French`.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.entry().name
    }

    /// In itself, where CLDR has the language's own locale: `français`, `日本語`.
    #[must_use]
    pub fn native_name(self) -> Option<&'static str> {
        self.entry().native_name
    }

    /// Whether it is a language rather than one of ISO 639-2's special codes: undetermined,
    /// multiple, no linguistic content, uncoded.
    #[must_use]
    pub const fn is_language(self) -> bool {
        !matches!(self, Self::Und | Self::Mul | Self::Zxx | Self::Mis)
    }

    /// The language any of its codes names, in any case: `fr`, `fra`, `FRE`, `iw` (`he`).
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        if !(2..=3).contains(&code.len()) {
            return None;
        }
        let code = code.to_ascii_lowercase();
        search(&CODES, &code)
    }
}

fn search(table: &[(&str, Language)], key: &str) -> Option<Language> {
    table
        .binary_search_by(|(candidate, _)| (*candidate).cmp(key))
        .ok()
        .map(|found| table[found].1)
}

/// A few ASCII bytes, so that a tag is `Copy` and compares without allocating.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Subtag<const N: usize> {
    bytes: [u8; N],
    len: u8,
}

impl<const N: usize> Subtag<N> {
    fn new(text: &str) -> Option<Self> {
        let source = text.as_bytes();
        if source.len() > N || !source.is_ascii() {
            return None;
        }
        let mut bytes = [0; N];
        bytes[..source.len()].copy_from_slice(source);
        Some(Self {
            bytes,
            len: u8::try_from(source.len()).ok()?,
        })
    }

    fn as_str(&self) -> &str {
        // Built from a `&str` of ASCII bytes only.
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or_default()
    }
}

impl<const N: usize> fmt::Debug for Subtag<N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A language, with the script and region a BCP 47 tag may add, written canonically: the
/// language in its shortest code and lower case, the script in title case, the region in upper
/// case (`zh-Hant-HK`, `es-419`). Other subtags are dropped.
///
/// A well-formed code the table does not hold (`qaa`, a private-use one) is kept as written;
/// its [`language`](Self::language) is [`Language::Mis`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LanguageTag {
    code: Subtag<3>,
    script: Option<Subtag<4>>,
    region: Option<Subtag<3>>,
}

impl LanguageTag {
    /// A tag in any spelling a file, a provider or a client uses: `fr-CA`, `pt_BR`, `fre`,
    /// `French`. `None` for what names no language: `unknown`, teletext's `fra,fra`, a blank.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.len() > 3 {
            if let Some(language) = search(&NAMES, &text.to_lowercase()) {
                return Some(language.into());
            }
        }
        let mut subtags = text.split(['-', '_']);
        let primary = subtags.next()?;
        if !(2..=3).contains(&primary.len()) || !primary.bytes().all(|b| b.is_ascii_alphabetic()) {
            return None;
        }
        let code = match Language::from_code(primary) {
            Some(language) => Subtag::new(language.code())?,
            None => Subtag::new(&primary.to_ascii_lowercase())?,
        };
        let mut tag = Self {
            code,
            script: None,
            region: None,
        };
        let mut next = subtags.next();
        if let Some(script) =
            next.filter(|s| s.len() == 4 && s.bytes().all(|b| b.is_ascii_alphabetic()))
        {
            let mut written = script.to_ascii_lowercase();
            written[..1].make_ascii_uppercase();
            tag.script = Subtag::new(&written);
            next = subtags.next();
        }
        if let Some(region) = next.filter(|region| is_region(region)) {
            tag.region = Subtag::new(&region.to_ascii_uppercase());
        }
        Some(tag)
    }

    /// The tag of a language and a region, the region checked as [`parse`](Self::parse)
    /// checks it.
    #[must_use]
    pub fn with_region(self, region: Option<&str>) -> Option<Self> {
        let region = match region {
            None => None,
            Some(region) if is_region(region) => Subtag::new(&region.to_ascii_uppercase()),
            Some(_) => return None,
        };
        Some(Self { region, ..self })
    }

    /// The tag of a language and a script, the script in four letters.
    #[must_use]
    pub fn with_script(self, script: Option<&str>) -> Option<Self> {
        let script = match script {
            None => None,
            Some(script)
                if script.len() == 4 && script.bytes().all(|b| b.is_ascii_alphabetic()) =>
            {
                let mut written = script.to_ascii_lowercase();
                written[..1].make_ascii_uppercase();
                Subtag::new(&written)
            }
            Some(_) => return None,
        };
        Some(Self { script, ..self })
    }

    #[must_use]
    pub fn language(self) -> Language {
        Language::from_code(self.code.as_str()).unwrap_or(Language::Mis)
    }

    /// The language's code as the tag writes it: `fr`, or a code the table does not hold.
    #[must_use]
    pub fn code(&self) -> &str {
        self.code.as_str()
    }

    /// ISO 15924, in title case: `Hant`.
    #[must_use]
    pub fn script(&self) -> Option<&str> {
        self.script.as_ref().map(Subtag::as_str)
    }

    /// ISO 3166-1 alpha-2 or UN M.49, in upper case: `CA`, `419`.
    #[must_use]
    pub fn region(&self) -> Option<&str> {
        self.region.as_ref().map(Subtag::as_str)
    }
}

fn is_region(region: &str) -> bool {
    (region.len() == 2 && region.bytes().all(|b| b.is_ascii_alphabetic()))
        || (region.len() == 3 && region.bytes().all(|b| b.is_ascii_digit()))
}

impl From<Language> for LanguageTag {
    fn from(language: Language) -> Self {
        Self {
            code: Subtag::new(language.code()).unwrap_or(Subtag {
                bytes: *b"mis",
                len: 3,
            }),
            script: None,
            region: None,
        }
    }
}

impl fmt::Display for LanguageTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())?;
        for subtag in [self.script(), self.region()].into_iter().flatten() {
            write!(formatter, "-{subtag}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn tag(text: &str) -> LanguageTag {
        LanguageTag::parse(text).unwrap()
    }

    #[test]
    fn every_spelling_of_a_language_is_the_same_language() {
        for spelling in ["fr", "fra", "fre", "FRE", "French", "french", " fr "] {
            assert_eq!(tag(spelling), tag("fr"), "{spelling}");
            assert_eq!(tag(spelling).language(), Language::Fr);
        }
        assert_eq!(tag("ger").language(), Language::De);
        assert_eq!(tag("chi").language(), Language::Zh);
        assert_eq!(tag("iw").language(), Language::He);
        assert_eq!(tag("cmn").language(), Language::Zh);
    }

    #[test]
    fn a_region_keeps_two_dubs_apart() {
        assert_ne!(tag("fr-FR"), tag("fr-CA"));
        assert_eq!(tag("fr-FR").language(), tag("fr-CA").language());
    }

    #[test]
    fn writes_bcp_47_canonically() {
        for (written, canonical) in [
            ("fr-FR", "fr-FR"),
            ("pt_br", "pt-BR"),
            ("fre-ca", "fr-CA"),
            ("es-419", "es-419"),
            ("zh-hant", "zh-Hant"),
            ("ZH-HANT-hk", "zh-Hant-HK"),
            ("yue-Hant", "yue-Hant"),
            ("en-US-x-private", "en-US"),
            ("eng", "en"),
        ] {
            assert_eq!(tag(written).to_string(), canonical, "{written}");
            assert_eq!(tag(canonical).to_string(), canonical);
        }
    }

    #[test]
    fn special_codes_are_languages_of_the_table_but_name_none() {
        for (code, language) in [
            ("und", Language::Und),
            ("mul", Language::Mul),
            ("zxx", Language::Zxx),
            ("mis", Language::Mis),
        ] {
            assert_eq!(tag(code).language(), language);
            assert!(!language.is_language());
        }
        assert!(Language::Fr.is_language());
    }

    #[test]
    fn a_code_outside_the_table_is_kept_and_reads_as_uncoded() {
        assert_eq!(tag("qaa").to_string(), "qaa");
        assert_eq!(tag("qaa").language(), Language::Mis);
        assert_ne!(tag("qaa"), tag("qad"));
    }

    #[test]
    fn what_names_no_language_is_refused() {
        for text in [
            "",
            "  ",
            "unknown",
            "fra,fra",
            "f",
            "1a",
            "français-du-nord",
        ] {
            assert_eq!(LanguageTag::parse(text), None, "{text}");
        }
    }

    #[test]
    fn codes_and_names_come_with_each_language() {
        assert_eq!(Language::Fr.code(), "fr");
        assert_eq!(Language::Fr.terminological_code(), Some("fra"));
        assert_eq!(Language::Fr.bibliographic_code(), Some("fre"));
        assert_eq!(Language::En.bibliographic_code(), Some("eng"));
        assert_eq!(Language::Fil.terminological_code(), Some("fil"));
        assert_eq!(Language::Fr.name(), "French");
        assert_eq!(Language::Fr.native_name(), Some("français"));
        assert_eq!(Language::Und.terminological_code(), Some("und"));
    }

    #[test]
    fn regions_and_scripts_are_checked() {
        let french = LanguageTag::from(Language::Fr);
        assert_eq!(french.with_region(Some("ca")).unwrap().to_string(), "fr-CA");
        assert_eq!(french.with_region(Some("C")), None);
        assert_eq!(
            french.with_script(Some("latn")).unwrap().to_string(),
            "fr-Latn"
        );
        assert_eq!(french.with_script(Some("La")), None);
    }

    #[test]
    fn every_language_is_listed_once_under_its_own_code() {
        let languages: std::collections::BTreeSet<Language> =
            CODES.iter().map(|(_, language)| *language).collect();
        assert_eq!(languages.len(), ENTRIES.len());
        for language in languages {
            assert_eq!(Language::from_code(language.code()), Some(language));
        }
    }

    #[test]
    fn the_tables_are_sorted_for_their_search() {
        assert!(CODES.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert!(NAMES.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }
}
