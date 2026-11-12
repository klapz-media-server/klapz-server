//! Completing what a user is typing into a filter: titles, people, genres.
//!
//! Measured on the reference's autocompletion, which is not its search: the text is one
//! phrase that has to begin a word, so `first sl` completes *The First Slam Dunk* and `slam d`
//! does too, while `dunk ` completes nothing. A word begins after a space only — `aim` does
//! not complete *Le mal-aimé*, nor `homme` *L'homme qui aimait* — case does not matter and
//! accents do: `aime` completes *Mange, prie, aime* and not *C'est dur d'être aimé*.

/// Whether `text` begins a word of `candidate`, read as one phrase.
///
/// Empty text completes nothing: the reference refuses it, and a match on everything is not
/// a completion.
pub fn completes(candidate: &str, text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let (candidate, text) = (candidate.to_lowercase(), text.to_lowercase());
    candidate.starts_with(&text) || candidate.contains(&format!(" {text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phrase_completes_from_the_start_of_any_word() {
        assert!(completes("The First Slam Dunk", "dun"));
        assert!(completes("The First Slam Dunk", "first sl"));
        assert!(completes("Crocodile Dundee", "DUN"));
        assert!(!completes("Dune", "dun "));
        assert!(!completes("Dune", "une"));
    }

    #[test]
    fn a_word_begins_after_a_space_only() {
        assert!(!completes("Le mal-aimé", "aim"));
        assert!(!completes(
            "L'homme qui aimait les soucoupes volantes",
            "homme"
        ));
        assert!(completes("Confessions d'un homme dangereux", "homme"));
        // A space counts wherever it is, a doubled one included.
        assert!(completes("9  Semaines ½", " "));
    }

    #[test]
    fn minds_accents_and_not_case() {
        assert!(!completes("C'est dur d'être aimé par des cons", "aime"));
        assert!(completes("Plaire, aimer et courir vite", "aime"));
        assert!(completes("À tous les garçons", "à tous"));
        assert!(!completes("Dune", ""));
    }
}
