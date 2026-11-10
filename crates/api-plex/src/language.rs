//! The language a client reads, for the few strings the server writes itself.
//!
//! The reference names seasons and titles its shelves in the client's `X-Plex-Language`:
//! `Saison 3` and `Récemment ajouté dans VF - Films` for `fr`, `Staffel 3` for `de`, and the
//! English source text with no language at all. The strings come from its own catalogs,
//! extracted into [`crate::translations`].

use crate::client::ClientIdentity;
use crate::language_names::{LocaleNames, LOCALES};
use crate::translations::{Catalog, CATALOGS, ENGLISH};

#[derive(Debug, Clone, Copy)]
pub struct Language(&'static Catalog);

impl Default for Language {
    fn default() -> Self {
        Self(&ENGLISH)
    }
}

impl Language {
    /// The catalog for a client: its exact language when there is one (`pt-BR` is `pt_BR`),
    /// else its primary subtag (`fr-CA` is `fr`, `ja` is `ja_JP`), else English.
    pub fn of(client: &ClientIdentity) -> Self {
        let Some(tag) = client.language.as_deref() else {
            return Self::default();
        };
        let normalized = tag.replace('-', "_");
        let primary = normalized.split('_').next().unwrap_or_default();
        let primary_of = |code: &str| code.split('_').next().unwrap_or_default().to_owned();

        CATALOGS
            .iter()
            .find(|catalog| catalog.code.eq_ignore_ascii_case(&normalized))
            .or_else(|| {
                CATALOGS
                    .iter()
                    .find(|catalog| primary_of(catalog.code).eq_ignore_ascii_case(primary))
            })
            .map_or_else(Self::default, Self)
    }

    pub(crate) const fn catalog(self) -> &'static Catalog {
        self.0
    }

    /// The language and region names this client reads, from the same catalog choice.
    pub(crate) fn names(self) -> &'static LocaleNames {
        LOCALES
            .iter()
            .find(|names| names.catalog == self.0.code)
            // The generator writes English first, and every catalog has its locale.
            .unwrap_or(&LOCALES[0])
    }

    /// A season's display name, when the source stores none: `Saison 3`, or the language's
    /// word for specials for season zero. A stored title wins, as it does for *The Haunting
    /// of Bly Manor*, a season titled as its own series.
    pub fn season_title(self, stored: Option<&str>, number: Option<i32>) -> String {
        match (stored, number) {
            (Some(title), _) => title.to_owned(),
            (None, Some(0)) => self.0.specials.to_owned(),
            (None, Some(number)) => self.0.season.replace("{1}", &number.to_string()),
            (None, None) => String::new(),
        }
    }

    pub const fn continue_watching(self) -> &'static str {
        self.0.continue_watching
    }

    pub fn recently_added_in(self, library: &str) -> String {
        self.0.recently_added_in.replace("{1}", library)
    }

    pub const fn recent_playlists(self) -> &'static str {
        self.0.recent_playlists
    }

    pub const fn related_movies(self) -> &'static str {
        self.0.related_movies
    }

    pub const fn related_shows(self) -> &'static str {
        self.0.related_shows
    }

    pub fn more_with(self, person: &str) -> String {
        self.0.more_with.replace("{1}", person)
    }

    pub fn more_by(self, person: &str) -> String {
        self.0.more_by.replace("{1}", person)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(language: &str) -> ClientIdentity {
        ClientIdentity {
            language: Some(language.to_owned()),
            ..ClientIdentity::default()
        }
    }

    #[test]
    fn names_a_season_in_the_clients_language() {
        // Measured on the reference: the same season reads `Saison 3`, `Staffel 3`,
        // `Temporada 3` and, with no language, `Season 3`.
        assert_eq!(
            Language::of(&client("fr")).season_title(None, Some(3)),
            "Saison 3"
        );
        assert_eq!(
            Language::of(&client("de")).season_title(None, Some(3)),
            "Staffel 3"
        );
        assert_eq!(
            Language::of(&client("es")).season_title(None, Some(3)),
            "Temporada 3"
        );
        assert_eq!(
            Language::of(&ClientIdentity::default()).season_title(None, Some(3)),
            "Season 3"
        );
    }

    #[test]
    fn a_stored_season_title_is_never_translated() {
        assert_eq!(
            Language::of(&client("fr")).season_title(Some("The Haunting of Bly Manor"), Some(1)),
            "The Haunting of Bly Manor"
        );
    }

    #[test]
    fn falls_back_from_region_to_language_to_english() {
        assert_eq!(
            Language::of(&client("fr-CA")).season_title(None, Some(1)),
            "Saison 1"
        );
        assert_eq!(Language::of(&client("pt-BR")).0.code, "pt_BR");
        assert_eq!(Language::of(&client("ja")).0.code, "ja_JP");
        assert_eq!(
            Language::of(&client("xx")).season_title(None, Some(1)),
            "Season 1"
        );
    }

    #[test]
    fn titles_shelves_in_the_clients_language() {
        let french = Language::of(&client("fr"));
        assert_eq!(french.continue_watching(), "Continuer à regarder");
        assert_eq!(
            french.recently_added_in("VF - Films"),
            "Récemment ajouté dans VF - Films"
        );
    }
}
