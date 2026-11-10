//! The name an item goes by in a URL.
//!
//! Lowercase, accents folded as search folds them, every run of anything that is not a letter
//! or a digit turned into one hyphen. Letters outside ASCII that have no decomposition stay
//! themselves (`日本`, `привет`): a browser shows them as they are, and dropping them would leave
//! a Japanese film with no slug at all. The few Latin letters a reader expects spelled out are.
//!
//! A slug is a name, not an identity: two films of the same title share one. Whoever routes on
//! it keeps the id beside it.

use crate::search::fold;

/// What an item is called when nothing in its name survives: no title, or only punctuation.
pub const UNKNOWN: &str = "unknown";

/// Joins the parts of a name into one slug, skipping parts that give nothing, and falls back
/// on [`UNKNOWN`] when no part gives anything.
pub fn slug<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut slug = String::new();
    for part in parts {
        let piece = slug_of(part);
        if piece.is_empty() {
            continue;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&piece);
    }
    if slug.is_empty() {
        return UNKNOWN.to_owned();
    }
    slug
}

fn slug_of(text: &str) -> String {
    let mut slug = String::new();
    let mut pending_hyphen = false;
    for character in fold(text).chars() {
        let spelled = match character {
            'œ' => "oe",
            'æ' => "ae",
            'ß' => "ss",
            'ø' => "o",
            'ł' => "l",
            'đ' | 'ð' => "d",
            'þ' => "th",
            _ if character.is_alphanumeric() => {
                if pending_hyphen && !slug.is_empty() {
                    slug.push('-');
                }
                pending_hyphen = false;
                slug.push(character);
                continue;
            }
            _ => {
                pending_hyphen = true;
                continue;
            }
        };
        if pending_hyphen && !slug.is_empty() {
            slug.push('-');
        }
        pending_hyphen = false;
        slug.push_str(spelled);
    }
    slug
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> String {
        slug([text])
    }

    #[test]
    fn folds_accents_and_joins_words_with_one_hyphen() {
        assert_eq!(one("L'Épée de Saint Gabriel"), "l-epee-de-saint-gabriel");
        assert_eq!(one("Frost / Nixon"), "frost-nixon");
        assert_eq!(one("  Soirée!! "), "soiree");
        assert_eq!(one("007 Spectre"), "007-spectre");
        assert_eq!(one("#Amour et déconnexion"), "amour-et-deconnexion");
    }

    #[test]
    fn spells_out_the_latin_letters_that_do_not_decompose() {
        assert_eq!(one("Œdipe roi"), "oedipe-roi");
        assert_eq!(one("Straße"), "strasse");
    }

    #[test]
    fn keeps_letters_of_other_scripts() {
        assert_eq!(one("千と千尋の神隠し"), "千と千尋の神隠し");
        assert_eq!(one("Брат 2"), "брат-2");
    }

    #[test]
    fn an_episode_is_named_after_its_series_then_itself() {
        assert_eq!(
            slug(["Bull", "Good News About Hell"]),
            "bull-good-news-about-hell"
        );
        assert_eq!(slug(["Bull", "!!"]), "bull");
    }

    #[test]
    fn a_name_with_nothing_left_is_unknown() {
        assert_eq!(slug(["", "?"]), "unknown");
        assert_eq!(slug(std::iter::empty()), "unknown");
        assert_eq!(one("« … »"), "unknown");
    }
}
