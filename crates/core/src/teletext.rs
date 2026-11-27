//! Which page of a broadcast's teletext a subtitle track is decoded from.
//!
//! A teletext track holds many pages. ffprobe-plex names those that are subtitles, each with a
//! language and its kind (subtitles proper, or for the hearing impaired): the ones its
//! descriptor declares, then the ones it carries undeclared, as Arte sends its German
//! subtitles beside its declared French ones. Each page is a subtitle track of its own.

/// One subtitle page a teletext track declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeletextPage {
    /// From 100 to 899: `888`.
    pub number: u16,
    /// As declared, `fra`; none when the descriptor left it blank.
    pub language: Option<String>,
    pub hearing_impaired: bool,
}

impl TeletextPage {
    /// The page as ffprobe-plex states it, `889:fra:subtitles`: what a track decoded from it
    /// stores, read back by [`pages`].
    #[must_use]
    pub fn stated(&self) -> String {
        format!(
            "{}:{}:{}",
            self.number,
            self.language.as_deref().unwrap_or_default(),
            if self.hearing_impaired {
                "hearing_impaired"
            } else {
                "subtitles"
            }
        )
    }
}

/// The pages as stated, `888:fra:hearing_impaired,889:fra:subtitles`, in their order. An
/// entry that does not read is passed over rather than failing the rest.
#[must_use]
pub fn pages(stated: &str) -> Vec<TeletextPage> {
    stated
        .split(',')
        .filter_map(|entry| {
            let mut fields = entry.split(':');
            let number = fields
                .next()?
                .parse()
                .ok()
                .filter(|number| (100..=899).contains(number))?;
            let language = fields.next()?;
            let hearing_impaired = match fields.next()? {
                "subtitles" => false,
                "hearing_impaired" => true,
                _ => return None,
            };
            fields.next().is_none().then(|| TeletextPage {
                number,
                language: (!language.is_empty()).then(|| language.to_owned()),
                hearing_impaired,
            })
        })
        .collect()
}

/// The page a track's subtitles are decoded from: its own, for a track stored per page; for one
/// stored with all its pages, before tracks were, the first that is subtitles proper, else the
/// first for the hearing impaired. None when it states none, and every subtitle page is then
/// decoded.
///
/// Subtitles proper first, because a broadcaster may declare a page for the hearing impaired it
/// leaves empty: Arte's 888 carries nothing on Peur primale, its 889 1,883 subtitles.
#[must_use]
pub fn subtitle_page(stated: &str) -> Option<TeletextPage> {
    let mut pages = pages(stated);
    let chosen = pages
        .iter()
        .position(|page| !page.hearing_impaired)
        .unwrap_or(0);
    (chosen < pages.len()).then(|| pages.swap_remove(chosen))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_pages_as_ffprobe_plex_states_them() {
        // Peur primale, recorded from Arte.
        assert_eq!(
            pages("888:fra:hearing_impaired,889:fra:subtitles"),
            [
                TeletextPage {
                    number: 888,
                    language: Some("fra".to_owned()),
                    hearing_impaired: true,
                },
                TeletextPage {
                    number: 889,
                    language: Some("fra".to_owned()),
                    hearing_impaired: false,
                },
            ]
        );
        assert_eq!(
            pages("777::subtitles")[0].language,
            None,
            "a blank language"
        );
        // What does not read is passed over.
        assert_eq!(
            pages("88:fra:subtitles,x,999:fra:subtitles,777:fra:other"),
            []
        );
        assert_eq!(pages(""), []);
    }

    #[test]
    fn a_page_states_itself_as_it_is_read() {
        for stated in [
            "889:fra:subtitles",
            "888:fra:hearing_impaired",
            "150::subtitles",
        ] {
            assert_eq!(pages(stated)[0].stated(), stated);
        }
    }

    #[test]
    fn decodes_subtitles_proper_before_those_for_the_hearing_impaired() {
        let number = |stated| subtitle_page(stated).map(|page| page.number);
        assert_eq!(
            number("888:fra:hearing_impaired,889:fra:subtitles"),
            Some(889)
        );
        assert_eq!(number("888:fra:hearing_impaired"), Some(888));
        assert_eq!(number("777:deu:subtitles,888:fra:subtitles"), Some(777));
        assert_eq!(number("nothing"), None);
    }
}
