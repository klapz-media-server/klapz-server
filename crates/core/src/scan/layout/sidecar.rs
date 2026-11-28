//! Which subtitle files belong to a media file, and what their names say
//! (`docs/plex/scanner/discovery.md` §3.2).

use super::{classify, split_extension, FileClass, SidecarMatch};
use crate::language::LanguageTag;

/// The legacy list less `txt`, `ass` and `ssa`, which the reference did not attach (§3.2; why
/// is open, §9.5). A `.txt` beside a film is as likely notes as subtitles.
pub(super) const SUBTITLE_EXTENSIONS: [&str; 12] = [
    "utf", "utf8", "utf-8", "srt", "smi", "rt", "aqt", "jss", "idx", "sub", "psb", "vtt",
];

const HEARING_IMPAIRED: [&str; 3] = ["sdh", "cc", "hi"];

pub(super) fn sidecars(media: &str, siblings: &[&str]) -> Vec<SidecarMatch> {
    let (media_stem, _) = split_extension(media);
    let other_stems: Vec<&str> = siblings
        .iter()
        .filter(|name| **name != media && classify(name) == FileClass::Media)
        .map(|name| split_extension(name).0)
        .collect();
    let has = |stem: &str, extension: &str| {
        siblings.iter().any(|name| {
            let (other, other_extension) = split_extension(name);
            other == stem && other_extension.eq_ignore_ascii_case(extension)
        })
    };

    siblings
        .iter()
        .enumerate()
        .filter(|(_, name)| classify(name) == FileClass::Subtitle)
        .filter_map(|(index, name)| {
            let (stem, extension) = split_extension(name);
            let extension = extension.to_ascii_lowercase();
            // A VobSub is an `.idx` and its `.sub`; the pair is one track, named by the `.idx`.
            if (extension == "idx" && !has(stem, "sub")) || (extension == "sub" && has(stem, "idx"))
            {
                return None;
            }
            let suffixes = suffixes_after(stem, media_stem)?;
            // `X.Part2.en.srt` beside `X.mkv` and `X.Part2.mkv` is the second's.
            let closer = other_stems.iter().any(|other| {
                other.len() > media_stem.len() && suffixes_after(stem, other).is_some()
            });
            if closer {
                return None;
            }
            describe(index, &suffixes)
        })
        .collect()
}

/// The dotted words a sidecar's stem adds to its media's: `["en", "forced"]` for
/// `X.en.forced` after `X`.
fn suffixes_after<'a>(stem: &'a str, media_stem: &str) -> Option<Vec<&'a str>> {
    if stem == media_stem {
        return Some(Vec::new());
    }
    let rest = stem.strip_prefix(media_stem)?.strip_prefix('.')?;
    Some(rest.split('.').collect())
}

fn describe(index: usize, suffixes: &[&str]) -> Option<SidecarMatch> {
    let mut sidecar = SidecarMatch {
        index,
        language: None,
        forced: false,
        hearing_impaired: false,
    };
    let mut others = Vec::new();
    for suffix in suffixes {
        let lower = suffix.to_ascii_lowercase();
        if lower == "forced" {
            sidecar.forced = true;
        } else if HEARING_IMPAIRED.contains(&lower.as_str()) {
            sidecar.hearing_impaired = true;
        } else if lower != "default" && lower != "normal" {
            others.push(*suffix);
        }
    }
    // The legacy rule allows two dotted words besides the flags; more is another file's name.
    if others.len() > 2 {
        return None;
    }
    // A full name (`French`) is not read as a language, as on the reference.
    sidecar.language = others
        .last()
        .filter(|code| is_language_code(code))
        .and_then(|code| LanguageTag::parse(code));
    Some(sidecar)
}

/// `[A-Za-z]{2,3}(-[A-Za-z0-9]{2,4})?`: `en`, `eng`, `pt-BR`.
fn is_language_code(code: &str) -> bool {
    let (language, region) = code
        .split_once('-')
        .map_or((code, None), |(l, r)| (l, Some(r)));
    (2..=3).contains(&language.len())
        && language.chars().all(|c| c.is_ascii_alphabetic())
        && region.is_none_or(|region| {
            (2..=4).contains(&region.len()) && region.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(siblings: &[&str]) -> Vec<(String, Option<LanguageTag>, bool, bool)> {
        sidecars("X.mkv", siblings)
            .into_iter()
            .map(|sidecar| {
                (
                    siblings[sidecar.index].to_owned(),
                    sidecar.language,
                    sidecar.forced,
                    sidecar.hearing_impaired,
                )
            })
            .collect()
    }

    fn sidecar(
        name: &str,
        language: Option<&str>,
        forced: bool,
        hi: bool,
    ) -> (String, Option<LanguageTag>, bool, bool) {
        (
            name.to_owned(),
            language.and_then(LanguageTag::parse),
            forced,
            hi,
        )
    }

    #[test]
    fn reads_the_measured_sidecar_names() {
        let siblings = [
            "X.mkv",
            "X.en.srt",
            "X.eng.srt",
            "X.pt-BR.srt",
            "X.French.srt",
            "X.srt",
            "X.en.cc.srt",
            "X.en.hi.srt",
            "X.de.sdh.srt",
            "X.fr.forced.srt",
            "X.pl.sdh.forced.srt",
            "X.es.default.srt",
            "X.ja.vtt",
            "X.ko.smi",
        ];
        assert_eq!(
            found(&siblings),
            [
                sidecar("X.en.srt", Some("en"), false, false),
                sidecar("X.eng.srt", Some("eng"), false, false),
                sidecar("X.pt-BR.srt", Some("pt-BR"), false, false),
                sidecar("X.French.srt", None, false, false),
                sidecar("X.srt", None, false, false),
                sidecar("X.en.cc.srt", Some("en"), false, true),
                sidecar("X.en.hi.srt", Some("en"), false, true),
                sidecar("X.de.sdh.srt", Some("de"), false, true),
                sidecar("X.fr.forced.srt", Some("fr"), true, false),
                sidecar("X.pl.sdh.forced.srt", Some("pl"), true, true),
                sidecar("X.es.default.srt", Some("es"), false, false),
                sidecar("X.ja.vtt", Some("ja"), false, false),
                sidecar("X.ko.smi", Some("ko"), false, false),
            ]
        );
    }

    #[test]
    fn leaves_what_the_reference_did_not_attach() {
        let siblings = [
            "X.mkv",
            "Other Name.de.srt",
            "X.it.forced.ass",
            "X.nl.ssa",
            "X.sv.txt",
        ];
        assert!(found(&siblings).is_empty());
    }

    #[test]
    fn gives_a_sidecar_to_the_media_it_names_most_closely() {
        let siblings = ["X.mkv", "X.Part2.mkv", "X.Part2.en.srt", "X.en.srt"];
        assert_eq!(
            found(&siblings),
            [sidecar("X.en.srt", Some("en"), false, false)]
        );
    }

    #[test]
    fn pairs_an_idx_with_its_sub() {
        assert_eq!(
            found(&["X.mkv", "X.en.idx", "X.en.sub"]),
            [sidecar("X.en.idx", Some("en"), false, false)]
        );
        assert!(found(&["X.mkv", "X.en.idx"]).is_empty());
        assert_eq!(
            found(&["X.mkv", "X.sub"]),
            [sidecar("X.sub", None, false, false)]
        );
    }
}
