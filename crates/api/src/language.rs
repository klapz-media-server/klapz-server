//! Languages on the official API: the `Language` enum, the same on every field, and `Locale`
//! where a field holds a region or a script as well.

mod values;

use async_graphql::{InputObject, Object};
use klapz_core::language::LanguageTag;
use serde::de::Error;
use serde::{Deserialize, Deserializer};

pub use values::Language;

/// A language, with the script and region a BCP 47 tag may add: `fr-CA`, a dub from Quebec,
/// apart from `fr-FR`, one from France.
#[derive(Debug, Clone, Copy)]
pub struct Locale(pub LanguageTag);

#[Object]
impl Locale {
    /// The language. `MIS` for a code the enum does not hold, which `tag` keeps.
    async fn language(&self) -> Language {
        self.0.language().into()
    }

    /// The script, ISO 15924 in title case: `Hant`.
    async fn script(&self) -> Option<&str> {
        self.0.script()
    }

    /// The region, ISO 3166-1 alpha-2 or UN M.49 in upper case: `CA`, `419`.
    async fn region(&self) -> Option<&str> {
        self.0.region()
    }

    /// The BCP 47 tag, written canonically: `fr-CA`, `zh-Hant`, `es-419`.
    async fn tag(&self) -> String {
        self.0.to_string()
    }

    /// The language's name in English: `French`.
    async fn name(&self) -> &'static str {
        self.0.language().name()
    }

    /// The language's name in itself, `français`, where it is known.
    async fn native_name(&self) -> Option<&'static str> {
        self.0.language().native_name()
    }
}

/// A language, with the script and region a BCP 47 tag may add.
#[derive(Debug, Clone, InputObject)]
pub struct LocaleInput {
    /// The language.
    pub language: Language,
    /// A script, ISO 15924 in four letters: `Hant`.
    pub script: Option<String>,
    /// A region, ISO 3166-1 alpha-2 (`CA`) or UN M.49 (`419`).
    pub region: Option<String>,
}

impl LocaleInput {
    /// The tag this names; `None` when the script or the region is not one.
    #[must_use]
    pub fn tag(&self) -> Option<LanguageTag> {
        let language: klapz_core::language::Language = self.language.into();
        LanguageTag::from(language)
            .with_script(self.script.as_deref())?
            .with_region(self.region.as_deref())
    }
}

/// A `Locale` as an adapter selects it, `{ tag }`.
#[derive(Deserialize)]
struct Selected {
    tag: String,
}

fn read<E: Error>(selected: &Selected) -> Result<LanguageTag, E> {
    LanguageTag::parse(&selected.tag)
        .ok_or_else(|| E::custom(format!("`{}` is not a language tag", selected.tag)))
}

/// Deserializes a `Locale` selected as `{ tag }` from a response of the schema, for the
/// adapters.
///
/// # Errors
///
/// When the value is no such object, or its tag does not read.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<LanguageTag, D::Error> {
    read(&Selected::deserialize(deserializer)?)
}

/// Deserializes a nullable `Locale` selected as `{ tag }`; with `#[serde(default)]`, an absent
/// one too.
///
/// # Errors
///
/// When the value is no such object, or its tag does not read.
pub fn deserialize_optional<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<LanguageTag>, D::Error> {
    Option::<Selected>::deserialize(deserializer)?
        .as_ref()
        .map(read)
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_input_names_a_language_with_its_region() {
        let quebec = LocaleInput {
            language: Language::Fr,
            script: None,
            region: Some("ca".to_owned()),
        };
        assert_eq!(
            quebec.tag().map(|tag| tag.to_string()).as_deref(),
            Some("fr-CA")
        );
        let wrong = LocaleInput {
            region: Some("Canada".to_owned()),
            ..quebec
        };
        assert_eq!(wrong.tag(), None);
    }

    #[test]
    fn an_adapter_reads_a_selected_locale_back() {
        #[derive(Deserialize)]
        struct Track {
            #[serde(default, deserialize_with = "deserialize_optional")]
            language: Option<LanguageTag>,
        }
        let read = |json: &str| {
            serde_json::from_str::<Track>(json)
                .ok()
                .map(|track| track.language.map(|tag| tag.to_string()))
        };
        assert_eq!(
            read(r#"{"language": {"tag": "fr-CA"}}"#),
            Some(Some("fr-CA".to_owned()))
        );
        assert_eq!(read(r#"{"language": null}"#), Some(None));
        assert_eq!(read("{}"), Some(None));
        assert_eq!(read(r#"{"language": {"tag": "fra,fra"}}"#), None);
    }
}
