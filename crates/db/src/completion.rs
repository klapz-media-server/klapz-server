//! The completion rule of `core::completion` as SQL: a phrase beginning a word of a column.
//!
//! Two `LIKE`s, one at the start of the text and one after a space, on the lowered column.
//! `SQLite` lowers ASCII alone, so the text is tried in lower case and, when a word of it
//! opens with a letter outside ASCII, with those initials in upper case: `élite` then
//! completes *Élite* and `les évadés` *Les Évadés*, as on the reference. A capital outside
//! ASCII inside a word is beyond it, and titles hardly hold one.

use sea_orm::sea_query::{Expr, Func, LikeExpr, SimpleExpr};
use sea_orm::Condition;

/// Whether `text` begins a word of `column`, read as one phrase. Empty text completes
/// nothing, where a `LIKE '%'` would complete everything.
pub fn completes(column: &SimpleExpr, text: &str) -> Condition {
    if text.is_empty() {
        return Condition::all().add(Expr::val(1).eq(0));
    }
    variants(text)
        .into_iter()
        .fold(Condition::any(), |any, variant| {
            let escaped = escape(&variant);
            let lowered = || Expr::expr(Func::lower(column.clone()));
            any.add(lowered().like(LikeExpr::new(format!("{escaped}%")).escape('\\')))
                .add(lowered().like(LikeExpr::new(format!("% {escaped}%")).escape('\\')))
        })
}

fn variants(text: &str) -> Vec<String> {
    let lowered = text.to_lowercase();
    let mut initial = true;
    let capitalized: String = lowered
        .chars()
        .flat_map(|character| {
            let capital = initial && !character.is_ascii();
            initial = character == ' ';
            let upper = capital
                .then(|| character.to_uppercase())
                .into_iter()
                .flatten();
            upper.chain((!capital).then_some(character))
        })
        .collect();
    if capitalized == lowered {
        vec![lowered]
    } else {
        vec![lowered, capitalized]
    }
}

/// Escapes `LIKE`'s wildcards and its escape character: `%` and `_` complete themselves.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tries_an_accented_initial_in_both_cases() {
        assert_eq!(variants("Dun"), ["dun"]);
        assert_eq!(variants("élite"), ["élite", "Élite"]);
        assert_eq!(variants("À TOUS"), ["à tous", "À tous"]);
        assert_eq!(variants("les évadés"), ["les évadés", "les Évadés"]);
    }

    #[test]
    fn wildcards_match_themselves() {
        assert_eq!(escape("100%_\\"), "100\\%\\_\\\\");
    }
}
