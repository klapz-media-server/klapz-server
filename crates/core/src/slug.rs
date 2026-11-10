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

/// Joins the parts of a name into one slug, skipping parts that give nothing. `None` when no
/// part does.
pub fn slug<'a>(parts: impl IntoIterator<Item = &'a str>) -> Option<String> {
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
    (!slug.is_empty()).then_some(slug)
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

    fn one(text: &str) -> Option<String> {
        slug([text])
    }

    #[test]
    fn folds_accents_and_joins_words_with_one_hyphen() {
        assert_eq!(
            one("L'Épée de Saint Gabriel").as_deref(),
            Some("l-epee-de-saint-gabriel")
        );
        assert_eq!(one("Frost / Nixon").as_deref(), Some("frost-nixon"));
        assert_eq!(one("  Soirée!! ").as_deref(), Some("soiree"));
        assert_eq!(one("007 Spectre").as_deref(), Some("007-spectre"));
        assert_eq!(
            one("#Amour et déconnexion").as_deref(),
            Some("amour-et-deconnexion")
        );
    }

    #[test]
    fn spells_out_the_latin_letters_that_do_not_decompose() {
        assert_eq!(one("Œdipe roi").as_deref(), Some("oedipe-roi"));
        assert_eq!(one("Straße").as_deref(), Some("strasse"));
    }

    #[test]
    fn keeps_letters_of_other_scripts() {
        assert_eq!(one("千と千尋の神隠し").as_deref(), Some("千と千尋の神隠し"));
        assert_eq!(one("Брат 2").as_deref(), Some("брат-2"));
    }

    #[test]
    fn an_episode_is_named_after_its_series_then_itself() {
        assert_eq!(
            slug(["Bull", "Good News About Hell"]).as_deref(),
            Some("bull-good-news-about-hell")
        );
        assert_eq!(slug(["Bull", "!!"]).as_deref(), Some("bull"));
        assert_eq!(slug(["", "?"]), None);
    }
}
