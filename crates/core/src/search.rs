//! Turning a title into searchable words.
//!
//! Lives in `core` because what counts as a word is a domain decision, and three places have
//! to agree on it exactly: the importer that writes the index, the query that reads it, and
//! the in-memory repository the tests run against. Two of the three disagreeing would show up
//! as a search that silently misses items.
//!
//! The target behavior is the reference server's `title=` filter, which is a **word-prefix**
//! match and not a column prefix: on 488 films from 2022, `title=le` returns 75 — exactly the
//! items having a word starting with `le` in their title, sort title or original title, with
//! accents folded. A prefix on the sort title alone returns 45 of those 75, so the difference
//! is not academic.

use unicode_normalization::UnicodeNormalization;

/// Folds a string for indexing and for matching: accents removed, lowercased.
///
/// Decomposing and dropping the combining marks is what makes `Légende` findable as `legende`.
/// It deliberately does *not* fold ligatures — `Œ` has no decomposition, so it stays itself,
/// which matches how the reference server buckets it separately under its first letter.
pub fn fold(text: &str) -> String {
    text.nfd()
        .filter(|character| !unicode_normalization::char::is_combining_mark(*character))
        .collect::<String>()
        .to_lowercase()
}

/// The text with its accents removed and its case kept: what the reference stores as a sort
/// title it derives.
///
/// Recomposed afterwards, unlike [`fold`]: this is stored text a client displays, and a
/// Hangul syllable left as its decomposed letters would not read as one.
pub fn without_accents(text: &str) -> String {
    text.nfd()
        .filter(|character| !unicode_normalization::char::is_combining_mark(*character))
        .nfc()
        .collect()
}

/// The searchable words of a title.
///
/// Splits on **whitespace only**, keeping punctuation inside the word. That is not a
/// simplification, it is the reference server's rule, and it is observable in both directions:
///
/// - `title=white` does not find `Black/White`, because the word is `black/white` and `white`
///   is not a prefix of it. `title=black` does find it.
/// - `title=nixon` does find `Frost / Nixon`, because the spaces make `nixon` its own word.
/// - `title=amour` does not find `#Amour et déconnexion`, while `title=%23amour` does.
///
/// Since the match is a *prefix*, only leading punctuation hides a word; trailing punctuation
/// is absorbed, so `Soirée!!` is still found by `soiree`.
pub fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    fold(text)
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>()
        .into_iter()
}

/// Every distinct searchable word of an item, across the fields the reference searches.
///
/// Title, sort title and original title, because that is the set the reference matches: using
/// the title alone returns 74 of the 75 films that `title=le` finds, and the missing one is
/// matched through its original title.
pub fn title_words(
    title: Option<&str>,
    title_sort: Option<&str>,
    original_title: Option<&str>,
) -> Vec<String> {
    let mut collected: Vec<String> = [title, title_sort, original_title]
        .into_iter()
        .flatten()
        .flat_map(words)
        .collect();
    collected.sort();
    collected.dedup();
    collected
}

/// The half-open range of words that begin with `prefix`, or `None` when there is none.
///
/// A range rather than a `LIKE 'prefix%'`: an explicit range is a seek on a plain binary
/// index, while a `LIKE` on a case-insensitive collation refuses the index outright. That
/// lesson was learned the expensive way on the title column — see `metadata_repository`.
///
/// `None` when the prefix is empty, or when its last character has no successor, in which
/// case the caller should use a lower bound alone.
pub fn prefix_range(prefix: &str) -> Option<(String, Option<String>)> {
    let folded = fold(prefix);
    if folded.is_empty() {
        return None;
    }
    Some((folded.clone(), successor(&folded)))
}

/// The smallest string greater than every string beginning with `prefix`.
///
/// Increments the last character that can be incremented and drops the rest, which is the
/// standard way to turn a prefix into an upper bound. `None` when every trailing character is
/// already at the top of its range — vanishingly rare, and the caller then has no upper bound
/// rather than a wrong one.
fn successor(prefix: &str) -> Option<String> {
    let mut characters: Vec<char> = prefix.chars().collect();
    while let Some(last) = characters.pop() {
        // Surrogate code points are not valid `char`s, so stepping over the gap keeps the
        // bound a real string rather than silently producing none.
        let next = match u32::from(last) + 1 {
            0xD800 => 0xE000,
            other => other,
        };
        if let Some(next) = char::from_u32(next) {
            let mut bound: String = characters.into_iter().collect();
            bound.push(next);
            return Some(bound);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn folds_accents_but_not_ligatures() {
        assert_eq!(fold("Légende"), "legende");
        assert_eq!(fold("Soirée"), "soiree");
        assert_eq!(fold("ÀÉÎÕÜ"), "aeiou");
        // No decomposition, so it survives — as it does in the reference server's own
        // first-letter buckets, where `Œ` is its own letter.
        assert_eq!(fold("Œuvre"), "œuvre");
    }

    #[test]
    fn splits_on_whitespace_and_keeps_punctuation() {
        assert_eq!(
            words("Spider-Man: No Way Home").collect::<Vec<_>>(),
            ["spider-man:", "no", "way", "home"]
        );
        // Runs of whitespace collapse; the leading `#` stays attached to its word, which is
        // why the reference finds this title by `%23amour` and not by `amour`.
        assert_eq!(
            words("#Amour et   déconnexion").collect::<Vec<_>>(),
            ["#amour", "et", "deconnexion"]
        );
        assert_eq!(words("#JeSuisLà").collect::<Vec<_>>(), ["#jesuisla"]);
        assert_eq!(words("").count(), 0);
        assert_eq!(words("   ").count(), 0);
        // Digits are words: `2 Fast 2 Furious` has to be findable by `2`.
        assert_eq!(
            words("2 Fast 2 Furious").collect::<Vec<_>>(),
            ["2", "fast", "2", "furious"]
        );
    }

    #[test]
    fn a_slash_does_not_separate_words() {
        // The rule that is easiest to get wrong, and the one the reference is unambiguous
        // about: `Black/White` is one word, so it answers to `black` and not to `white`.
        assert_eq!(words("Black/White").collect::<Vec<_>>(), ["black/white"]);
        assert_eq!(
            words("Frost / Nixon").collect::<Vec<_>>(),
            ["frost", "/", "nixon"]
        );
    }

    #[test]
    fn collects_distinct_words_across_the_searched_fields() {
        let collected = title_words(
            Some("Le Nouveau Jouet"),
            Some("[CAM] Le Nouveau Jouet"),
            Some("The New Toy"),
        );
        // `le`, `nouveau` and `jouet` appear in two fields and are stored once. `[cam]`
        // keeps its brackets, so this item does not answer to `cam`.
        assert_eq!(
            collected,
            ["[cam]", "jouet", "le", "new", "nouveau", "the", "toy"]
        );
    }

    #[test]
    fn an_absent_field_contributes_nothing() {
        assert_eq!(title_words(Some("Avatar"), None, None), ["avatar"]);
        assert!(title_words(None, None, None).is_empty());
    }

    #[test]
    fn a_prefix_becomes_a_half_open_range() {
        let (low, high) = prefix_range("Lé").expect("a range");
        assert_eq!(low, "le");
        assert_eq!(high.as_deref(), Some("lf"));
        // The bound has to exclude `lf` itself and include everything between.
        assert!(low.as_str() < "legende" && "legende" < high.as_deref().expect("bound"));
        assert!("lf" >= high.as_deref().expect("bound"));

        assert_eq!(prefix_range(""), None);
        assert_eq!(
            prefix_range("   ").map(|(low, _)| low),
            Some("   ".to_owned())
        );
    }

    #[test]
    fn a_multi_character_prefix_increments_only_the_last_character() {
        let (low, high) = prefix_range("avat").expect("a range");
        assert_eq!((low.as_str(), high.as_deref()), ("avat", Some("avau")));
    }

    #[test]
    fn carries_when_the_last_character_is_maximal() {
        // `\u{10FFFF}` has no successor, so the carry moves left.
        let prefix = format!("a{}", char::MAX);
        assert_eq!(
            prefix_range(&prefix).and_then(|(_, high)| high).as_deref(),
            Some("b")
        );
    }

    #[test]
    fn steps_over_the_surrogate_gap() {
        // `\u{D7FF}` + 1 lands in the surrogate range, which is not a valid `char`.
        let bound = prefix_range("\u{D7FF}")
            .and_then(|(_, high)| high)
            .expect("has a successor");
        assert_eq!(bound, "\u{E000}");
    }

    #[test]
    fn has_no_upper_bound_for_a_maximal_prefix() {
        assert_eq!(
            prefix_range(&char::MAX.to_string()),
            Some((char::MAX.to_string(), None))
        );
    }

    #[test]
    fn a_bound_is_greater_than_anything_with_the_prefix() {
        // The property the whole range technique rests on: without it a page boundary would
        // drop rows.
        let (_, bound) = prefix_range("dune").expect("a range");
        let bound = bound.expect("a bound");
        for candidate in ["dune", "dune: part two", "dunes", "dune\u{10FFFF}"] {
            assert!(
                candidate < bound.as_str(),
                "{candidate} should sort before {bound}"
            );
        }
        assert!("dunf" >= bound.as_str());
    }
}
