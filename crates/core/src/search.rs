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
//!
//! Ranking lives here for the same reason: storage orders a search by [`RankedTerm::rung`]
//! over the [`title_keys`] it keeps, the in-memory repository by the function itself, and the
//! score a result carries is [`score`] of that rung.

use unicode_normalization::UnicodeNormalization;

use crate::metadata::ItemKind;

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

/// Every distinct searchable word of a tag's name: a person, a genre.
///
/// The same words as a title's, so `hanks` finds Tom Hanks the way `nixon` finds Frost / Nixon.
pub fn name_words(name: &str) -> Vec<String> {
    let mut collected: Vec<String> = words(name).collect();
    collected.sort();
    collected.dedup();
    collected
}

/// Whether every word of `term` begins one of `words`: the word-prefix match, for what has
/// too few rows to keep an index of words, as the index's query reads it.
pub fn words_match(words: &[String], term: &str) -> bool {
    self::words(term).all(|prefix| {
        prefix_range(&prefix).is_some_and(|(lower, upper)| {
            words
                .iter()
                .any(|word| word >= &lower && upper.as_ref().is_none_or(|bound| word < bound))
        })
    })
}

/// What separates the first word of a title from the rest, for ranking: a space or an
/// apostrophe, so that *Amelie's Miracle* begins with the word `amelie`. The index splits on
/// spaces alone, which is why `amelie` and `amelie's` are one word to match and two to rank.
pub const LEAD_SEPARATORS: [char; 3] = [' ', '\'', '’'];

/// A whole title as a search ranks it: folded, its words joined by single spaces.
///
/// Stored beside the word index, because how well a title answers a term depends on the title
/// as a whole — equal to the term, or opening with it — which the words alone cannot say.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TitleKey {
    pub key: String,
    /// The title or the sort title: what a title equal to the term is read from. *The Batman*
    /// is equal to `batman` through its sort title.
    pub exact: bool,
    /// The title or the original title: what a title opening with the term is read from. An
    /// episode carries its series' as its original title, and ranks by it.
    pub leading: bool,
}

/// The distinct keys of an item, one per text, with what each counts for.
pub fn title_keys(
    title: Option<&str>,
    title_sort: Option<&str>,
    original_title: Option<&str>,
) -> Vec<TitleKey> {
    let mut keys: Vec<TitleKey> = Vec::new();
    for (text, exact, leading) in [
        (title, true, true),
        (title_sort, true, false),
        (original_title, false, true),
    ] {
        let Some(key) = text.map(phrase).filter(|key| !key.is_empty()) else {
            continue;
        };
        match keys.iter_mut().find(|known| known.key == key) {
            Some(known) => {
                known.exact |= exact;
                known.leading |= leading;
            }
            None => keys.push(TitleKey {
                key,
                exact,
                leading,
            }),
        }
    }
    keys.sort();
    keys
}

/// A text folded and its words joined by single spaces, so that two spellings differing in
/// spacing alone compare equal.
fn phrase(text: &str) -> String {
    words(text).collect::<Vec<_>>().join(" ")
}

/// How well a title answers a term, from the least to the best.
///
/// The reference's ladder, measured on `/hubs/search` and `/library/search` over 53 terms:
/// each rung is two tenths of the score, whatever the term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rung {
    /// Matched by some word, nowhere in particular.
    Anywhere,
    /// The title begins with the first word of the term: `Hellboy` for `hell`.
    LeadingPrefix,
    /// The title's first word is the term's first word: `Hell Fest` for `hell`.
    LeadingWord,
    /// The title is the term: `Dune` for `dune`.
    Exact,
}

impl Rung {
    /// Its position on the ladder, from 0.
    pub const fn step(self) -> u8 {
        match self {
            Self::Anywhere => 0,
            Self::LeadingPrefix => 1,
            Self::LeadingWord => 2,
            Self::Exact => 3,
        }
    }
}

/// A search term as ranking reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedTerm {
    phrase: String,
    lead: String,
}

impl RankedTerm {
    /// `None` for a blank term, which matches nothing.
    pub fn new(term: &str) -> Option<Self> {
        let phrase = phrase(term);
        if phrase.is_empty() {
            return None;
        }
        let lead = phrase
            .split(LEAD_SEPARATORS)
            .next()
            .unwrap_or_default()
            .to_owned();
        Some(Self { phrase, lead })
    }

    /// The whole term, as a [`TitleKey`] equal to it is written.
    pub fn phrase(&self) -> &str {
        &self.phrase
    }

    /// The term's first word, split as [`LEAD_SEPARATORS`] say. `None` when the term opens
    /// with a separator, as `'s` does: no title leads with nothing.
    pub fn lead(&self) -> Option<&str> {
        Some(self.lead.as_str()).filter(|lead| !lead.is_empty())
    }

    /// How well an item whose title keys are `keys` answers the term.
    pub fn rung(&self, keys: &[TitleKey]) -> Rung {
        if keys.iter().any(|key| key.exact && key.key == self.phrase) {
            return Rung::Exact;
        }
        let Some(lead) = self.lead() else {
            return Rung::Anywhere;
        };
        let leading = || keys.iter().filter(|key| key.leading);
        if leading().any(|key| opens_with_word(&key.key, lead)) {
            Rung::LeadingWord
        } else if leading().any(|key| key.key.starts_with(lead)) {
            Rung::LeadingPrefix
        } else {
            Rung::Anywhere
        }
    }
}

fn opens_with_word(key: &str, word: &str) -> bool {
    key.strip_prefix(word)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(LEAD_SEPARATORS))
}

/// The relevance of an item matched by title, between 0 and 1: the reference's own scale, so
/// that results ranked here and there compare.
///
/// The rung decides, two tenths apiece from 0.33 for a film or a series and from 0.31 for an
/// episode; the audience rating breaks ties within a rung, a ten-thousandth per point: *Le
/// Pire voisin au monde*, rated 9.7 and found through an actor, scores 0.33097. Measured on
/// 9,151 scored items over 53 terms: 9,123 exact, the rest a hundred-thousandth apart where
/// the rating ends in 5.
pub fn score(kind: ItemKind, rung: Rung, audience_rating: Option<f64>) -> f64 {
    let rating = audience_rating.unwrap_or(0.0).clamp(0.0, 10.0);
    #[allow(
        clippy::cast_precision_loss,
        reason = "hundredths stay below 100, exact in a float"
    )]
    let hundredths = hundredths(kind, rung) as f64;
    five_decimals(hundredths / 100.0 + rating / 10_000.0)
}

/// The hundredths of a score that the rung and the item's kind make: 33 for a film or a
/// series matched anywhere, 93 for an exact title, two fewer for an episode. What storage
/// orders a search by first, as an integer, so that a cursor resumes on an exact value.
pub fn hundredths(kind: ItemKind, rung: Rung) -> i64 {
    let base = if kind == ItemKind::Episode { 31 } else { 33 };
    base + 20 * i64::from(rung.step())
}

/// The relevance of a tag found by name: 0.32 for a match anywhere, 0.52 for a name starting
/// with the term, 0.92 for the name itself.
pub fn name_score(rung: Rung) -> f64 {
    five_decimals(0.32 + 0.2 * f64::from(rung.step()))
}

/// Rounded to the five decimals the reference writes, so that 0.33 + 0.00097 reads 0.33097
/// rather than its nearest binary neighbour.
fn five_decimals(score: f64) -> f64 {
    (score * 100_000.0).round() / 100_000.0
}

/// How well a name answers a term: the name itself, a name starting with it, or a match
/// anywhere. Unlike a title, a name's rung minds its accents: for `amelie`, Amelie Kiefer
/// starts with the term and Amélie Glenn does not.
pub fn name_rung(name: &str, term: &str) -> Rung {
    let (name, term) = (name.to_lowercase(), term.trim().to_lowercase());
    if name == term {
        Rung::Exact
    } else if name.starts_with(&term) {
        Rung::LeadingPrefix
    } else {
        Rung::Anywhere
    }
}

/// A tag in one library, as a search lists it: a person, a genre or a collection.
#[derive(Debug, Clone, Copy)]
pub struct ListedTag<'a> {
    pub name: &'a str,
    pub library_title: &'a str,
    /// How many of the library's items carry it.
    pub count: u64,
    pub id: i64,
}

/// The order a search lists tags in: by how well the name answers `term` ([`name_rung`]),
/// then library by library in title order, the most carried first, then the newest tag.
/// The reference's order entry for entry on the people of six terms.
pub fn tag_order(term: &str, left: &ListedTag<'_>, right: &ListedTag<'_>) -> std::cmp::Ordering {
    name_rung(right.name, term)
        .cmp(&name_rung(left.name, term))
        .then_with(|| {
            left.library_title
                .to_lowercase()
                .cmp(&right.library_title.to_lowercase())
        })
        .then_with(|| right.count.cmp(&left.count))
        .then_with(|| right.id.cmp(&left.id))
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
    fn a_name_has_each_of_its_words_once() {
        assert_eq!(name_words("Jean-Pierre Léaud"), ["jean-pierre", "leaud"]);
        assert_eq!(name_words("Bob Bob"), ["bob"]);
        assert!(name_words("").is_empty());
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

    fn rung(title: &str, sort: Option<&str>, original: Option<&str>, term: &str) -> Rung {
        RankedTerm::new(term)
            .expect("a term")
            .rung(&title_keys(Some(title), sort, original))
    }

    #[test]
    fn keys_a_title_once_with_what_each_text_counts_for() {
        let keys = title_keys(Some("The  Batman"), Some("Batman"), Some("The Batman"));
        assert_eq!(
            keys,
            [
                TitleKey {
                    key: "batman".to_owned(),
                    exact: true,
                    leading: false
                },
                TitleKey {
                    key: "the batman".to_owned(),
                    exact: true,
                    leading: true
                },
            ]
        );
        assert!(title_keys(None, Some("  "), None).is_empty());
    }

    #[test]
    fn ranks_titles_on_the_references_ladder() {
        // Scores measured for `dune`, `batman`, `amélie`, `hell` and `breaking b`.
        assert_eq!(rung("Dune", None, None, "dune"), Rung::Exact);
        assert_eq!(rung("DUNE", None, None, " Dune "), Rung::Exact);
        assert_eq!(
            rung("The Batman", Some("Batman"), None, "batman"),
            Rung::Exact
        );
        assert_eq!(
            rung(
                "Dune : Deuxième partie",
                None,
                Some("Dune: Part Two"),
                "dune"
            ),
            Rung::LeadingWord
        );
        assert_eq!(
            rung("Amelie's Miracle", None, Some("Amelie rennt"), "amélie"),
            Rung::LeadingWord
        );
        // The original title counts for a leading word, not for an exact title.
        assert_eq!(
            rung("Le Fléau", None, Some("Batman Beyond"), "batman"),
            Rung::LeadingWord
        );
        assert_eq!(
            rung("L'Enfer", None, Some("Hell"), "hell"),
            Rung::LeadingWord
        );
        assert_eq!(
            rung("Dune: Part Two", None, None, "dune"),
            Rung::LeadingPrefix
        );
        assert_eq!(rung("Hellboy", None, None, "hell"), Rung::LeadingPrefix);
        assert_eq!(
            rung("Breaking Bad", None, None, "breaking b"),
            Rung::LeadingWord
        );
        assert_eq!(
            rung("Les Nouvelles aventures de Batman", None, None, "batman"),
            Rung::Anywhere
        );
        // The sort title does not lead.
        assert_eq!(
            rung("Le Batman", Some("Batman, Le"), None, "batman"),
            Rung::Anywhere
        );
        assert_eq!(rung("Legend", None, None, "le"), Rung::LeadingPrefix);
    }

    #[test]
    fn a_blank_term_has_no_rank_and_a_separator_no_lead() {
        assert_eq!(RankedTerm::new("  "), None);
        let term = RankedTerm::new("'s").expect("a term");
        assert_eq!(term.lead(), None);
        assert_eq!(
            term.rung(&title_keys(Some("'s Avond"), None, None)),
            Rung::Anywhere
        );
        assert_eq!(term.rung(&title_keys(Some("'s"), None, None)), Rung::Exact);
    }

    #[test]
    fn scores_two_tenths_a_rung_and_a_ten_thousandth_a_rating_point() {
        let close = |left: f64, right: f64| (left - right).abs() < 1e-12;
        assert!(close(
            score(ItemKind::Movie, Rung::Exact, Some(6.5)),
            0.93065
        ));
        assert!(close(
            score(ItemKind::Movie, Rung::Anywhere, Some(9.7)),
            0.33097
        ));
        assert!(close(score(ItemKind::Show, Rung::LeadingWord, None), 0.73));
        assert!(close(
            score(ItemKind::Episode, Rung::LeadingPrefix, Some(6.9)),
            0.51069
        ));
        assert!(close(
            score(ItemKind::Episode, Rung::Exact, Some(10.0)),
            0.911
        ));
        // Five decimals and no binary residue, as the reference writes it.
        assert_eq!(
            score(ItemKind::Movie, Rung::Anywhere, Some(9.7)).to_string(),
            "0.33097"
        );
    }

    #[test]
    fn ranks_names_minding_their_accents() {
        assert_eq!(name_rung("Tom Hanks", "tom hanks"), Rung::Exact);
        assert_eq!(name_rung("Tom Hanks", "tom"), Rung::LeadingPrefix);
        assert_eq!(name_rung("Tommy Lee Jones", "tom"), Rung::LeadingPrefix);
        assert_eq!(name_rung("Cécile de France", "france"), Rung::Anywhere);
        assert_eq!(name_rung("Amelie Kiefer", "amelie"), Rung::LeadingPrefix);
        assert_eq!(name_rung("Amélie Glenn", "amelie"), Rung::Anywhere);
        assert_eq!(name_score(Rung::Exact).to_string(), "0.92");
        assert_eq!(name_score(Rung::LeadingPrefix).to_string(), "0.52");
        assert_eq!(name_score(Rung::Anywhere).to_string(), "0.32");
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
