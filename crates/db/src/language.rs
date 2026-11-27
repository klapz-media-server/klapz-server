//! Languages as storage keeps them: the canonical tag [`LanguageTag`] writes, `fr` or `fr-CA`,
//! which a filter compares as text.

use klapz_core::language::LanguageTag;

/// What the reference stored, `eng` or `fr_FR`, as a canonical tag: `en`, `fr-FR`. What names
/// no language, as `unknown` does, is none.
pub(crate) fn canonical(stored: Option<&str>) -> Option<String> {
    stored
        .and_then(LanguageTag::parse)
        .map(|tag| tag.to_string())
}
