//! Titles ranked by how far they are from a term: the search a voice assistant needs, where
//! the words heard are rarely the words written.
//!
//! The measure is the reference's, worked out on the distances it publishes and agreeing to
//! the unit on the 808 items both servers listed for 63 terms (`docs/plex/spec-gaps.md`): a
//! Levenshtein distance between the term and an item's sort title, both normalized the same
//! way.
//!
//! - **The sort title**, falling back to the title: *La Brea* is 5 from `le bureau` because
//!   it is compared as `Brea`.
//! - **A leading article leaves both**: `the office` is 0 from *The Office*, sorted as
//!   `Office`, and `le seigneur des anneaux` 11 from *La défonce des animaux*, whose sort
//!   title keeps its article. The articles are the reference's default list, and only one is
//!   removed: `the the office` is 4 from *The Office*. A text that is nothing but an article
//!   keeps it.
//! - **Case folds for ASCII only**, and a set of accented letters folds to its base — the
//!   Latin-1 ones, the carons, the lower-case macrons — while the rest stays as it is
//!   written: `boze cialo` is 4 from *Boże Ciało*, two for each of `ż` and `ł`.
//! - **What is left is measured in bytes**: a character that does not fold counts for its
//!   UTF-8 length, `鉄男` is 5 from `아랑`.

use crate::ids::SectionId;
use crate::metadata::{ItemKind, MetadataItem};
use crate::page::{Cursor, CursorKey};

/// The articles a term may open with, the reference's `the,das,der,a,an,el,la`.
pub const ARTICLES: [&str; 7] = ["the", "das", "der", "a", "an", "el", "la"];

/// The longest term measured, in bytes.
///
/// Each title costs a pass over the term, so its length multiplies the work of every
/// comparison. Twice the longest title of the reference library, which no spoken title
/// reaches; the reference itself sets no limit.
pub const MAX_TERM_BYTES: usize = 256;

/// A term or a title as it is compared: folded, each character one unit or its bytes.
///
/// Kept as text rather than as units, because storage indexes it and a reader of the index
/// should be able to read it: `office`, `boże ciało`.
pub fn fold(text: &str) -> String {
    text.chars()
        .filter(|character| !is_combining_mark(*character))
        .map(|character| {
            if character.is_ascii() {
                character.to_ascii_lowercase()
            } else {
                folded_letter(character).unwrap_or(character)
            }
        })
        .collect()
}

/// A term or a title as the two are compared: one leading article removed, then folded.
///
/// Leading spaces are dropped with the article, so ` the  office` is `office`; trailing ones
/// count, as they do on the reference (`the office ` is 1 from *The Office*).
pub fn term_key(term: &str) -> String {
    let term = term.trim_start_matches(' ');
    let without_article = term
        .split_once(' ')
        .filter(|(first, _)| {
            ARTICLES
                .iter()
                .any(|article| first.eq_ignore_ascii_case(article))
        })
        .map(|(_, rest)| rest.trim_start_matches(' '))
        .filter(|rest| !rest.is_empty());
    fold(without_article.unwrap_or(term))
}

/// An item's title as a term is compared with: its sort title, else its title, read as
/// [`term_key`] reads the term.
pub fn title_key(title_sort: Option<&str>, title: Option<&str>) -> String {
    term_key(
        title_sort
            .filter(|sort| !sort.is_empty())
            .or(title)
            .unwrap_or_default(),
    )
}

/// How many units a folded text holds: the length that bounds its distance to another, which
/// storage indexes so that a search reads only the titles of about the term's length.
pub fn key_length(key: &str) -> usize {
    key.chars().map(unit_count).sum()
}

/// The characters the reference folds to an ASCII letter, as it folds them.
///
/// Measured one by one over U+00C0–U+024F and U+1E00–U+1EFF: every Latin-1 letter with a
/// base letter, `ß` as `s`, the lower-case macrons and the carons. The rest of Latin
/// Extended-A and -B and all of Latin Extended Additional keep their bytes.
const fn folded_letter(character: char) -> Option<char> {
    Some(match character {
        'À'..='Å' | 'à'..='å' | 'ā' | 'Ǎ' | 'ǎ' => 'a',
        'Ç' | 'ç' | 'Č' | 'č' => 'c',
        'Ď' | 'ď' => 'd',
        'È'..='Ë' | 'è'..='ë' | 'ē' | 'Ě' | 'ě' => 'e',
        'Ǧ' | 'ǧ' => 'g',
        'Ì'..='Ï' | 'ì'..='ï' | 'ī' | 'Ǐ' | 'ǐ' => 'i',
        'ǰ' => 'j',
        'Ǩ' | 'ǩ' => 'k',
        'Ľ' | 'ľ' => 'l',
        'Ñ' | 'ñ' | 'Ň' | 'ň' => 'n',
        'Ò'..='Ö' | 'ò'..='ö' | 'ō' | 'Ǒ' | 'ǒ' => 'o',
        'Ř' | 'ř' => 'r',
        'ß' | 'Š' | 'š' => 's',
        'Ť' | 'ť' => 't',
        'Ù'..='Ü' | 'ù'..='ü' | 'ū' | 'Ǔ' | 'ǔ' => 'u',
        'Ý' | 'ý' | 'ÿ' => 'y',
        'Ž' | 'ž' => 'z',
        _ => return None,
    })
}

/// Combining diacritics, which the reference drops: `e` followed by U+0301 is `e`.
const fn is_combining_mark(character: char) -> bool {
    matches!(character, '\u{0300}'..='\u{036F}')
}

/// The Latin-1 letters left after folding — `æ`, `ø`, `þ`, `ð` and their capitals, `×`,
/// `÷` — count as one unit each on the reference, where the rest of the text counts bytes.
const fn is_single_unit(character: char) -> bool {
    matches!(character, '\u{00C0}'..='\u{00FF}')
}

const fn unit_count(character: char) -> usize {
    if is_single_unit(character) {
        1
    } else {
        character.len_utf8()
    }
}

/// Units are bytes, `0..=255`, and the single-unit letters past them, so that `æ` cannot
/// equal the lead byte of a character it shares no meaning with.
const ALPHABET: usize = 256 + 0x40;

fn units(key: &str) -> impl Iterator<Item = usize> + '_ {
    key.chars().flat_map(|character| {
        let single =
            is_single_unit(character).then(|| 256 + (u32::from(character) as usize - 0xC0));
        let mut encoded = [0_u8; 4];
        let bytes = if single.is_some() {
            0
        } else {
            character.encode_utf8(&mut encoded).len()
        };
        single
            .into_iter()
            .chain(encoded.into_iter().take(bytes).map(usize::from))
    })
}

/// The distance from one term to any number of titles.
///
/// Myers' bit-parallel algorithm, in Hyyrö's form for a term longer than a machine word: a
/// title costs one pass over its units and one word operation per 64 units of term, where
/// the textbook table costs the product of both lengths.
#[derive(Debug, Clone)]
pub struct Distance {
    length: usize,
    /// Per block of 64 term units, which positions each unit occupies.
    positions: Vec<Vec<u64>>,
    /// The bit of the term's last unit within its block: where the distance is read.
    last_bit: u64,
}

impl Distance {
    /// Prepares the comparison of `term_key` — already normalized by [`term_key`] — with
    /// titles.
    pub fn new(term_key: &str) -> Self {
        let units: Vec<usize> = units(term_key).collect();
        let blocks = units.len().div_ceil(64);
        let mut positions = vec![vec![0_u64; ALPHABET]; blocks];
        for (index, unit) in units.iter().enumerate() {
            positions[index / 64][*unit] |= 1 << (index % 64);
        }
        Self {
            length: units.len(),
            positions,
            last_bit: 1 << (units.len().saturating_sub(1) % 64),
        }
    }

    /// The term's length in units, the distance to an empty title.
    pub const fn length(&self) -> usize {
        self.length
    }

    /// The edit distance from the term to a title normalized by [`title_key`].
    pub fn to(&self, key: &str) -> u32 {
        let blocks = self.positions.len();
        if blocks == 0 {
            return u32::try_from(key_length(key)).unwrap_or(u32::MAX);
        }
        // Vertical deltas: every cell of the first column is one more than the one above.
        let mut plus = vec![u64::MAX; blocks];
        let mut minus = vec![0_u64; blocks];
        let mut distance = i64::try_from(self.length).unwrap_or(i64::MAX);
        for unit in units(key) {
            // The first row grows by one per title unit: the cost of inserting it.
            let mut carry = 1;
            for block in 0..blocks {
                let high = if block + 1 == blocks {
                    self.last_bit
                } else {
                    1 << 63
                };
                carry = advance(
                    &mut plus[block],
                    &mut minus[block],
                    self.positions[block][unit],
                    carry,
                    high,
                );
            }
            distance += carry;
        }
        u32::try_from(distance).unwrap_or(u32::MAX)
    }
}

/// One column of one block, given the horizontal delta entering it from the block above;
/// returns the delta leaving it at `high`. Bits above `high` are padding: carries and shifts
/// only move upward, so they never reach the bits that count.
fn advance(plus: &mut u64, minus: &mut u64, matches: u64, carry_in: i64, high: u64) -> i64 {
    let (vertical_plus, vertical_minus) = (*plus, *minus);
    let entering_minus = u64::from(carry_in < 0);
    let crossed = matches | vertical_minus;
    let matches = matches | entering_minus;
    let horizontal =
        (((matches & vertical_plus).wrapping_add(vertical_plus)) ^ vertical_plus) | matches;
    let mut horizontal_plus = vertical_minus | !(horizontal | vertical_plus);
    let mut horizontal_minus = vertical_plus & horizontal;
    let carry_out =
        i64::from(horizontal_plus & high != 0) - i64::from(horizontal_minus & high != 0);
    horizontal_plus <<= 1;
    horizontal_minus <<= 1;
    horizontal_minus |= entering_minus;
    horizontal_plus |= u64::from(carry_in > 0);
    *plus = horizontal_minus | !(crossed | horizontal_plus);
    *minus = horizontal_plus & crossed;
    carry_out
}

/// Titles closest to a term, across the libraries a caller may see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosestTitles {
    /// The term as spoken. Normalized by storage with [`term_key`].
    pub term: String,
    /// The kinds of item to rank. Empty ranks nothing.
    pub kinds: Vec<ItemKind>,
    /// The libraries searched, the ones a caller may see. `None` searches them all.
    pub sections: Option<Vec<SectionId>>,
}

/// An item and its distance to the term.
#[derive(Debug, Clone)]
pub struct TitleMatch {
    pub item: MetadataItem,
    pub distance: u32,
    /// The title as it was compared, which orders matches at the same distance.
    pub key: String,
}

impl TitleMatch {
    /// The position of this match: closest first, then by compared title, then by id.
    pub fn cursor(&self) -> Cursor {
        Cursor {
            keys: vec![
                CursorKey::Int(i64::from(self.distance)),
                CursorKey::Text(self.key.clone()),
            ],
            id: self.item.id.get(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// The textbook table, over the same units.
    fn reference_distance(term: &str, title: &str) -> u32 {
        let term: Vec<usize> = units(term).collect();
        let title: Vec<usize> = units(title).collect();
        let mut previous: Vec<usize> = (0..=title.len()).collect();
        for (row, left) in term.iter().enumerate() {
            let mut current = vec![row + 1];
            for (column, right) in title.iter().enumerate() {
                let substitution = previous[column] + usize::from(left != right);
                current.push(
                    substitution
                        .min(previous[column + 1] + 1)
                        .min(current[column] + 1),
                );
            }
            previous = current;
        }
        u32::try_from(previous[title.len()]).unwrap()
    }

    fn distance(term: &str, title_sort: &str) -> u32 {
        Distance::new(&term_key(term)).to(&title_key(Some(title_sort), None))
    }

    #[test]
    fn measures_what_the_reference_published() {
        assert_eq!(distance("dune", "Dune"), 0);
        assert_eq!(distance("dune", "Cube"), 2);
        assert_eq!(distance("tom hanks", "Four Hands"), 4);
        assert_eq!(distance("kaamelott", "Charlotte"), 5);
        assert_eq!(distance("the", "Them"), 1);
        // Compared as sort titles.
        assert_eq!(distance("le bureau", "Brea"), 5);
        assert_eq!(distance("kaamelott", "Angela"), 6);
        // The article leaves a sort title that kept it.
        assert_eq!(
            distance("le seigneur des anneaux", "La défonce des animaux"),
            11
        );
        assert_eq!(distance("un homme", "La Momie"), 5);
    }

    #[test]
    fn drops_one_leading_article() {
        assert_eq!(term_key("the office"), "office");
        assert_eq!(term_key("THE OFFICE"), "office");
        assert_eq!(term_key("  the  office"), "office");
        assert_eq!(term_key("das boot"), "boot");
        assert_eq!(term_key("la meute"), "meute");
        assert_eq!(term_key("the the office"), "the office");
        // Not on the list, and not followed by a space.
        assert_eq!(term_key("le bureau"), "le bureau");
        assert_eq!(term_key("the\toffice"), "the\toffice");
        assert_eq!(term_key("theoffice"), "theoffice");
        // An article alone stays, as does the space after a word.
        assert_eq!(term_key("the"), "the");
        assert_eq!(term_key("the office "), "office ");
    }

    #[test]
    fn folds_the_letters_the_reference_folds() {
        assert_eq!(fold("Le mal-aimé"), "le mal-aime");
        assert_eq!(fold("ÉTÉ"), "ete");
        assert_eq!(fold("Muße"), "muse");
        assert_eq!(fold("Čeština ūnī"), "cestina uni");
        // Upper-case macrons do not fold, and case only folds for ASCII.
        assert_eq!(fold("Āa"), "Āa");
        assert_eq!(fold("Boże Ciało"), "boże ciało");
        // A combining mark is dropped, whatever it follows.
        assert_eq!(fold("e\u{0301}"), "e");
    }

    #[test]
    fn counts_what_does_not_fold_in_bytes() {
        assert_eq!(distance("boze cialo", "Boże Ciało"), 4);
        assert_eq!(distance("鉄男", "아랑"), 5);
        assert_eq!(distance("wall·e", "Wall-E"), 2);
        assert_eq!(distance("iklimler", "İklimler"), 2);
        // The Latin-1 letters without a base count once, and equal only themselves.
        assert_eq!(distance("dunæ", "Dune"), 1);
        assert_eq!(distance("dunæ", "Dunæ"), 0);
        assert_eq!(key_length("æøß½"), 5);
    }

    #[test]
    fn an_empty_term_is_as_far_as_the_title_is_long() {
        assert_eq!(distance(" ", "M"), 1);
        assert_eq!(Distance::new("").to("abc"), 3);
        assert_eq!(Distance::new("abc").to(""), 3);
    }

    #[test]
    fn agrees_with_the_table_on_terms_of_every_block_count() {
        // A small linear congruential generator: deterministic, and enough to reach every
        // branch of the block carries.
        let mut state: u64 = 0x5DEE_CE66;
        let mut next = |bound: u64| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            (state >> 33) % bound
        };
        let alphabet = ['a', 'b', 'c', ' ', 'é', 'æ', 'ł', '鉄'];
        let mut text = |length: u64| -> String {
            (0..length)
                .map(|_| alphabet[usize::try_from(next(alphabet.len() as u64)).unwrap()])
                .collect()
        };
        for length in [0, 1, 5, 63, 64, 65, 127, 128, 129, 200] {
            for _ in 0..20 {
                let term = text(length);
                let title = text(length / 2 + 7);
                let (term, title) = (fold(&term), fold(&title));
                assert_eq!(
                    Distance::new(&term).to(&title),
                    reference_distance(&term, &title),
                    "{term:?} / {title:?}"
                );
            }
        }
    }
}
