//! Deciding which of a provider's search results is the film or series a library item holds.
//!
//! The score is the one Plex's TMDB agent gives (`TheMovieDB.bundle`): 90 less the edit
//! distance between the names, ten more for a year within one, five less per year beyond. Two
//! things are ours, both measured on the 18,332 films of the reference library: names are
//! compared folded (accents, case and punctuation gone) against every name a result carries,
//! and the query is cleaned of what a release name leaves in a scanned title — a bare year,
//! `FRENCH`, `VFF`, `MULTI` — which together took the films a true result would match above 85
//! from 86.1 % to 90.7 %.
//!
//! Plex also takes five points per rank of the provider's ordering. Here the ordering only
//! breaks ties: a point per rank let *Clique*, two letters from `Bloque` and ranked first, tie
//! with *Bloqués*, one letter away and ranked third.

use unicode_normalization::UnicodeNormalization;

use super::{Candidate, MatchQuery};

/// The score from which a result is taken without anyone confirming it: Plex's, under which it
/// falls back to another agent.
pub const ACCEPTED_SCORE: i32 = 85;

/// What answering through an alias costs, in edits: enough for a work whose own title matches
/// to win over one where only an alias or a foreign title does.
const ALIAS_PENALTY: usize = 3;

/// Words of a release name that end the title they follow, compared folded. Everything from
/// the first of them on is dropped, as a release name puts them after the title.
const RELEASE_WORDS: &[&str] = &[
    "french",
    "truefrench",
    "vff",
    "vfq",
    "vfi",
    "vf",
    "vf2",
    "vostfr",
    "vost",
    "vo",
    "multi",
    "fr",
    "en",
    "eng",
    "nf",
    "hdr",
    "dv",
    "dc",
    "vl",
    "bluray",
    "web",
    "webrip",
    "webdl",
    "hdtv",
    "extended",
    "unrated",
    "remastered",
];

/// The searches worth making for an item named `title` with `year`: the name as the scan gave
/// it, and the name cut at a bare year it still carries — which is also tried as the year,
/// since a release name's year is more often the film's than the one a folder adds.
pub fn queries(title: &str, year: Option<i32>) -> Vec<(String, Option<i32>)> {
    let cleaned = without_release_words(title);
    let mut found = vec![(cleaned.clone(), year)];
    if let Some((before, inner_year)) = split_at_bare_year(&cleaned) {
        found.insert(0, (before.clone(), Some(inner_year)));
        if year.is_some_and(|year| year != inner_year) {
            found.push((before, year));
        }
    }
    found.retain(|(title, _)| !title.is_empty());
    found.dedup();
    found
}

/// How well `candidate` answers `query`. Equal scores are left to the provider's ordering.
pub fn score(query: &MatchQuery, candidate: &Candidate) -> i32 {
    let wanted = fold(&query.title);
    let own = candidate
        .names
        .iter()
        .map(|name| levenshtein(&fold(name), &wanted));
    let aliases = candidate
        .aliases
        .iter()
        .map(|name| levenshtein(&fold(name), &wanted) + ALIAS_PENALTY);
    let distance = own.chain(aliases).min().unwrap_or(usize::MAX);
    let mut score = 90 - i32::try_from(distance).unwrap_or(i32::MAX / 2);
    if let (Some(wanted), Some(found)) = (query.year, candidate.year) {
        let apart = (wanted - found).abs();
        score += if apart <= 1 { 10 } else { -5 * apart };
    }
    score
}

/// A name reduced to what two spellings of it share: no accents, lower case, words separated by
/// single spaces.
pub fn fold(name: &str) -> String {
    let decomposed: String = name
        .replace('ß', "ss")
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    decomposed.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn without_release_words(title: &str) -> String {
    let words: Vec<&str> = title.split_whitespace().collect();
    let end = words
        .iter()
        .position(|word| RELEASE_WORDS.contains(&fold(word).as_str()))
        .filter(|at| *at > 0)
        .unwrap_or(words.len());
    words[..end].join(" ")
}

/// The title before its last word when that word is a plausible year, and the year.
fn split_at_bare_year(title: &str) -> Option<(String, i32)> {
    let (before, last) = title.trim_end().rsplit_once(' ')?;
    let year: i32 = last.parse().ok().filter(|_| last.len() == 4)?;
    (1900..=2100)
        .contains(&year)
        .then(|| (before.trim_end().to_owned(), year))
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut current = Vec::with_capacity(b.len() + 1);
        current.push(i + 1);
        for (j, cb) in b.iter().enumerate() {
            let replace = previous[j] + usize::from(ca != *cb);
            current.push(replace.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::ItemKind;

    fn query(title: &str, year: Option<i32>) -> MatchQuery {
        MatchQuery {
            kind: ItemKind::Movie,
            title: title.to_owned(),
            year,
            language: "fr-FR".to_owned(),
        }
    }

    fn candidate(names: &[&str], year: Option<i32>) -> Candidate {
        Candidate {
            provider_id: "1".to_owned(),
            names: names.iter().map(|name| (*name).to_owned()).collect(),
            aliases: Vec::new(),
            year,
        }
    }

    #[test]
    fn the_closer_title_wins_whatever_the_providers_order() {
        let wanted = query("Bloque", None);
        assert!(
            score(&wanted, &candidate(&["Bloqués"], Some(2015)))
                > score(&wanted, &candidate(&["Clique"], Some(2017)))
        );
    }

    #[test]
    fn folds_accents_case_and_punctuation() {
        assert_eq!(fold("Mad Max : Fury Road"), "mad max fury road");
        assert_eq!(
            fold("Les Bronzés 3 : Amis pour la vie"),
            "les bronzes 3 amis pour la vie"
        );
        assert_eq!(fold("Ant-Man"), "ant man");
    }

    #[test]
    fn cuts_release_words_and_takes_a_bare_year() {
        assert_eq!(
            queries("Peur Primale 1996 VFF", Some(2021)),
            vec![
                ("Peur Primale".to_owned(), Some(1996)),
                ("Peur Primale 1996".to_owned(), Some(2021)),
                ("Peur Primale".to_owned(), Some(2021)),
            ]
        );
        assert_eq!(
            queries("Cloclo 2012 FRENCH", None),
            vec![
                ("Cloclo".to_owned(), Some(2012)),
                ("Cloclo 2012".to_owned(), None)
            ]
        );
        assert_eq!(
            queries("Doodlebug", Some(1997)),
            vec![("Doodlebug".to_owned(), Some(1997))]
        );
    }

    #[test]
    fn a_title_that_is_a_year_is_kept() {
        assert_eq!(
            queries("1917", Some(2019)),
            vec![("1917".to_owned(), Some(2019))]
        );
    }

    #[test]
    fn scores_as_plex_with_folded_names() {
        let wanted = query("Mad Max Fury Road", Some(2015));
        let found = |year| score(&wanted, &candidate(&["Mad Max : Fury Road"], year));
        assert_eq!(found(Some(2015)), 100);
        assert_eq!(found(Some(2018)), 75);
        assert_eq!(found(None), 90);
    }

    #[test]
    fn an_alias_answers_for_less_than_a_title() {
        let wanted = query("Fringe", None);
        let own = candidate(&["Fringe"], Some(2008));
        let mut aliased = candidate(&["Fringe Nation"], Some(2020));
        aliased.aliases = vec!["Fringe".to_owned()];
        assert_eq!(score(&wanted, &own), 90);
        assert_eq!(score(&wanted, &aliased), 87);
    }

    #[test]
    fn any_name_of_a_result_can_answer() {
        let wanted = query("Mon beau-père, mes parents et moi", Some(2004));
        let found = candidate(
            &["Meet the Fockers", "Mon beau-père, mes parents et moi"],
            Some(2004),
        );
        assert_eq!(score(&wanted, &found), 100);
    }
}
