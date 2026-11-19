//! What a client shows for a track: `Français (DTS-HD MA 5.1)`, `4K DoVi/HDR10 (HEVC Main 10)`.
//!
//! Every rule here was measured, on 4,473 tracks of the reference library in French,
//! English and German, through a bench that compares these labels with the reference's:
//! 100% of audio labels, 99.9% of video and 99.4% of subtitles agree. The remainder is in
//! `docs/spec-gaps.md`.
//!
//! A label has two forms. `displayTitle` is built from the analysis alone. The extended form
//! starts from the title the file gives the track and adds, in parentheses, whatever of the
//! label that title does not already say: `VFF (Français EAC3 5.1)`, `English : Dolby TrueHD
//! 5.1` with nothing added, `AC3 5.1 (Anglais)`.

use crate::gql_items::GqlStream;
use crate::language::Language;
use crate::language_names::{ALPHA3, CANONICAL, ENDONYMS, LIKELY_REGIONS};

/// A track's language as a client reads it.
#[derive(Debug, PartialEq, Eq)]
pub struct LanguageLabel {
    /// In the client's language: `Anglais (R.-U.)`.
    pub name: String,
    /// The stored tag, unchanged: `en-GB`.
    pub tag: String,
    /// ISO 639-2/T: `eng`. Empty for a code with none, as `qaa` has.
    pub code: String,
}

/// The two labels of a track.
#[derive(Debug, PartialEq, Eq)]
pub struct TrackLabels {
    pub display: String,
    pub extended: String,
}

pub fn language(tag: &str, language: Language) -> LanguageLabel {
    LanguageLabel {
        name: language_name(tag, language),
        tag: tag.to_owned(),
        code: language_code(tag),
    }
}

pub fn labels(stream: &GqlStream, language: Language) -> TrackLabels {
    match stream.kind.as_str() {
        "VIDEO" => video(stream, language),
        "AUDIO" => audio(stream, language),
        _ => subtitle(stream, language),
    }
}

/// The tag's language and region, the region dropped when it is the one the language is
/// assumed to have: `fr-FR` is `fr`, `pt-BR` is `pt`, `fr-CA` stays `fr-CA`.
fn minimal(tag: &str) -> (String, Option<String>) {
    let mut parts = tag.split(['-', '_']);
    let language = canonical(&parts.next().unwrap_or_default().to_ascii_lowercase());
    let region = parts
        .find(|part| {
            (part.len() == 2 && part.chars().all(|c| c.is_ascii_alphabetic()))
                || (part.len() == 3 && part.chars().all(|c| c.is_ascii_digit()))
        })
        .map(str::to_ascii_uppercase)
        .filter(|region| lookup(&LIKELY_REGIONS, &language) != Some(region.as_str()));
    (language, region)
}

fn canonical(code: &str) -> String {
    lookup(&CANONICAL, code).unwrap_or(code).to_owned()
}

fn lookup<'a>(table: &'a [(&str, &'a str)], key: &str) -> Option<&'a str> {
    table
        .binary_search_by(|(candidate, _)| (*candidate).cmp(key))
        .ok()
        .map(|index| table[index].1)
}

/// ICU's display name of a tag, as the reference computes it: the language's standard name,
/// then the region's short name in parentheses, capitalised for a menu where the locale
/// says to. A code the locale does not name is shown as stored, capitalisation included.
/// The locale is the client's, else the language's own.
fn language_name(tag: &str, language: Language) -> String {
    // Two codes the reference names itself, in English whatever the client's language.
    match tag {
        "mis" => return "Uncoded language".to_owned(),
        "und" => return "Undetermined language".to_owned(),
        _ => {}
    }
    let (code, region) = minimal(tag);
    let names = language.names_for(&code);
    let name = match lookup(names.languages, &code) {
        None => tag.to_owned(),
        Some(name) => match region {
            None => name.to_owned(),
            Some(region) => {
                let region_name = lookup(names.regions, &region).unwrap_or(&region);
                format!("{name} ({region_name})")
            }
        },
    };
    if names.titlecase {
        capitalized(&name)
    } else {
        name
    }
}

fn language_code(tag: &str) -> String {
    let (code, _) = minimal(tag);
    // As the reference reports them: `mis` keeps its code, `und` has none.
    match code.as_str() {
        "mis" => return code,
        "und" => return String::new(),
        _ => {}
    }
    match lookup(&ALPHA3, &code) {
        Some(alpha3) => alpha3.to_owned(),
        None if code.len() == 3
            && lookup(Language::default().names().languages, &code).is_some() =>
        {
            code
        }
        None => String::new(),
    }
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

/// The words of a title, as the reference compares them.
///
/// Split on spaces, hyphens, `@`, `+`, `/`, commas and brackets, but not on `.` or `:`:
/// `BDRemux 1080p AVC` holds `1080p`, `Bluray.1080p.x264` does not, and `DTS:X` is not
/// `DTS`. `AC-3` is read as `AC3` and nothing else is, so `E-AC-3` holds neither `AC3` nor
/// `EAC3`.
fn words(title: &str) -> Vec<String> {
    title
        .to_lowercase()
        .replace("ac-3", "ac3")
        .split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '-' | '[' | ']' | '(' | ')' | '{' | '}' | '/' | ',' | '@' | '+'
                )
        })
        .filter(|word| !word.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Whether a title already says a track's language: its name in itself (`Français`,
/// `English`, but not `French`) or its stored tag (`FR` for `fr`, but not for `fr-FR`).
/// A tag with a region of its own has no single name in itself, so only the tag counts.
fn names_language(title_words: &[String], tag: &str) -> bool {
    let (code, region) = minimal(tag);
    let endonym = region
        .is_none()
        .then(|| lookup(&ENDONYMS, &code))
        .flatten()
        .map(str::to_lowercase);
    endonym.is_some_and(|endonym| title_words.contains(&endonym))
        || title_words.contains(&tag.to_lowercase())
}

/// `title (rest)`, or the title alone when it already says everything.
fn extended(title: &str, rest: &[String]) -> String {
    if rest.is_empty() {
        title.to_owned()
    } else {
        format!("{title} ({})", rest.join(" "))
    }
}

/// The resolution word of a video label, from both dimensions.
///
/// Not the ladder `Media.videoResolution` uses: here height counts as well as width, so
/// 960×720 is `720` and 1424×1072 is `1080`, and there is no `576`. The 1080 threshold lies
/// between 1,041 and 1,072 lines (1424×1040 is `720`), unmeasured closer.
fn resolution(width: i32, height: i32) -> &'static str {
    if width >= 3000 || height >= 1600 {
        "4K"
    } else if width >= 1700 || height >= 1060 {
        "1080"
    } else if width >= 1200 || height >= 720 {
        "720"
    } else if width >= 800 || height >= 440 {
        "480"
    } else {
        "SD"
    }
}

fn video(stream: &GqlStream, language: Language) -> TrackLabels {
    let Some(codec) = stream.codec.as_deref().filter(|codec| !codec.is_empty()) else {
        return unknown(language);
    };
    let analysis = stream.video.as_ref();
    let dimension = |value: Option<i32>| value.unwrap_or(0);
    let resolution = resolution(
        dimension(analysis.and_then(|video| video.width)),
        dimension(analysis.and_then(|video| video.height)),
    );
    let interlaced = analysis.and_then(|video| video.scan.as_deref()) == Some("INTERLACED");
    let scanned = match resolution {
        "4K" | "SD" => resolution.to_owned(),
        _ => format!("{resolution}{}", if interlaced { "i" } else { "p" }),
    };
    let dynamic_range = analysis.and_then(dynamic_range);
    let display = match &dynamic_range {
        Some(range) => format!("{scanned} {range}"),
        None => scanned.clone(),
    };
    let codec_label = video_codec(codec, stream.profile.as_deref());

    let Some(title) = stream.title.as_deref() else {
        return TrackLabels {
            extended: format!("{display} ({codec_label})"),
            display,
        };
    };
    let found = words(title);
    let lowered = resolution.to_lowercase();
    // `1080`, `1080p`, `1080i` and `1920x1080` all say the resolution.
    let says_resolution = found.iter().any(|word| {
        let bare = word
            .split_once('x')
            .filter(|(width, _)| width.chars().all(|c| c.is_ascii_digit()) && !width.is_empty())
            .map_or(word.as_str(), |(_, height)| height);
        bare == lowered || bare.strip_suffix(['p', 'i']) == Some(lowered.as_str())
    });
    let mut shown = Vec::new();
    if !says_resolution {
        shown.push(scanned);
    }
    if let Some(range) = dynamic_range {
        let says_range = ["hdr", "hdr10", "hdr10+", "dv", "dovi", "dolby"]
            .iter()
            .any(|word| found.iter().any(|found| found == word));
        if !says_range {
            shown.push(range);
        }
    }
    let mut rest = Vec::new();
    if !shown.is_empty() {
        rest.push(shown.join(" "));
    }
    let aliases: &[&str] = match codec {
        "h264" => &["h.264", "avc", "h264"],
        "hevc" => &["hevc", "h.265", "h265"],
        other => &[other],
    };
    if !aliases
        .iter()
        .any(|alias| found.iter().any(|word| word == alias))
    {
        rest.push(codec_label);
    }
    TrackLabels {
        extended: extended(title, &rest),
        display,
    }
}

/// `HDR10`, `HDR10+`, `HLG`, and Dolby Vision in front of whichever its base layer is:
/// `DoVi/HDR10`, or `DoVi` alone for a profile 5 track, which has none.
fn dynamic_range(video: &crate::gql_items::GqlVideo) -> Option<String> {
    let base = if video.has_hdr10_plus {
        Some("HDR10+")
    } else {
        match video.color.transfer.as_deref() {
            Some("smpte2084") => Some("HDR10"),
            Some("arib-std-b67") => Some("HLG"),
            _ => None,
        }
    };
    match (video.dolby_vision.is_some(), base) {
        (true, Some(base)) => Some(format!("DoVi/{base}")),
        (true, None) => Some("DoVi".to_owned()),
        (false, base) => base.map(ToOwned::to_owned),
    }
}

/// H.264 names its profile unless it is one of the three everyone has; HEVC always does;
/// every other codec is its name in capitals: `VP9`, `MPEG4`, `MSMPEG4V3`.
fn video_codec(codec: &str, profile: Option<&str>) -> String {
    match (codec, profile) {
        ("h264", None | Some("baseline" | "main" | "high")) => "H.264".to_owned(),
        ("h264", Some(profile)) => format!("H.264 {}", title_case(profile)),
        ("hevc", None) => "HEVC".to_owned(),
        ("hevc", Some(profile)) => format!("HEVC {}", title_case(profile)),
        (other, _) => other.to_uppercase(),
    }
}

fn title_case(text: &str) -> String {
    text.split(' ')
        .map(capitalized)
        .collect::<Vec<_>>()
        .join(" ")
}

fn audio(stream: &GqlStream, language: Language) -> TrackLabels {
    let Some(codec) = stream.codec.as_deref().filter(|codec| !codec.is_empty()) else {
        return unknown(language);
    };
    let catalog = language.catalog();
    let language_name = stream
        .language
        .as_deref()
        .map(|tag| language_name(tag, language));
    let traits: Vec<&str> = if stream.traits.is_visual_impaired {
        vec![catalog.descriptive]
    } else {
        Vec::new()
    };
    let head = std::iter::once(language_name.as_deref().unwrap_or(catalog.unknown))
        .chain(traits.iter().copied())
        .collect::<Vec<_>>()
        .join(" ");
    let codec_label = audio_codec(codec, stream.profile.as_deref());
    let channels = channels(
        stream.channels,
        stream
            .audio
            .as_ref()
            .and_then(|audio| audio.channel_layout.as_deref()),
    );
    let atmos = stream
        .profile
        .as_deref()
        .is_some_and(|profile| profile.contains("atmos"));
    let display = format!(
        "{head} ({codec_label} {channels}{})",
        if atmos { " + Atmos" } else { "" }
    );

    let Some(title) = stream.title.as_deref() else {
        return TrackLabels {
            extended: display.clone(),
            display,
        };
    };
    let found = words(title);
    let lowered = title.to_lowercase();
    let mut rest = Vec::new();
    if let (Some(name), Some(tag)) = (&language_name, stream.language.as_deref()) {
        if !names_language(&found, tag) {
            rest.push(name.clone());
        }
    }
    let aliases: &[&str] = if codec == "dca" {
        &["dts", "dca"]
    } else {
        &[codec]
    };
    if !aliases
        .iter()
        .any(|alias| found.iter().any(|word| word == alias))
    {
        rest.push(codec_label);
    }
    let channel_aliases: Vec<String> = match channels.as_str() {
        "Stereo" => vec!["stereo".to_owned(), "2.0".to_owned()],
        "Mono" => vec!["mono".to_owned(), "1.0".to_owned()],
        other => vec![other.to_lowercase()],
    };
    if !channel_aliases.iter().any(|alias| found.contains(alias)) {
        rest.push(channels);
    }
    // The trait is checked in English whatever the label's language, as the reference does.
    if !traits.is_empty() && !lowered.contains("descriptive") {
        rest.extend(traits.iter().map(|word| (*word).to_owned()));
    }
    if atmos && !lowered.contains("atmos") {
        rest.push("+ Atmos".to_owned());
    }
    TrackLabels {
        extended: extended(title, &rest),
        display,
    }
}

/// A DTS profile names the label whatever the codec says — an `eac3` track of profile `ma`
/// is `DTS-HD MA` on the reference — AAC is named by its profile, and everything else is its
/// name in capitals. A DTS track whose profile the analysis did not name, or named as
/// `ma + dts:x`, is `DCA`, the codec's own name.
fn audio_codec(codec: &str, profile: Option<&str>) -> String {
    match (codec, profile) {
        (_, Some("dts")) => "DTS".to_owned(),
        (_, Some("ma")) => "DTS-HD MA".to_owned(),
        (_, Some("hra")) => "DTS-HD HRA".to_owned(),
        (_, Some("es")) => "DTS-ES".to_owned(),
        ("aac", Some("he-aac")) => "HE-AAC".to_owned(),
        ("aac", Some("he-aacv2")) => "HE-AACv2".to_owned(),
        ("aac", Some("ltp")) => "AAC-LTP".to_owned(),
        ("aac", Some("main")) => "AAC Main".to_owned(),
        ("aac", _) => "AAC".to_owned(),
        (other, _) => other.to_uppercase(),
    }
}

/// The channel layout without its `(side)` or `(back)`, `Stereo` and `Mono` by name, else
/// the channel count's usual layout.
fn channels(count: Option<i32>, layout: Option<&str>) -> String {
    if let Some(layout) = layout {
        let layout = layout.replace("(side)", "").replace("(back)", "");
        return match layout.as_str() {
            "stereo" => "Stereo".to_owned(),
            "mono" => "Mono".to_owned(),
            // Truncated so on the reference, which this reproduces.
            "quad" => "Qua".to_owned(),
            other => title_case(other),
        };
    }
    match count {
        Some(1) => "Mono".to_owned(),
        Some(2) => "Stereo".to_owned(),
        Some(3) => "2.1".to_owned(),
        Some(4) => "4.0".to_owned(),
        Some(5) => "5.0".to_owned(),
        Some(6) => "5.1".to_owned(),
        Some(7) => "6.1".to_owned(),
        Some(8) => "7.1".to_owned(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn subtitle(stream: &GqlStream, language: Language) -> TrackLabels {
    let catalog = language.catalog();
    let language_name = stream
        .language
        .as_deref()
        .map(|tag| language_name(tag, language));
    // Each trait with the English words that, in a title, already say it.
    let mut traits: Vec<(&str, &[&str])> = Vec::new();
    if stream.is_forced {
        traits.push((catalog.forced, &["forced"]));
    }
    if stream.traits.is_hearing_impaired {
        traits.push((catalog.sdh, &["sdh", "cc"]));
    }
    let display = std::iter::once(language_name.as_deref().unwrap_or(catalog.unknown))
        .chain(traits.iter().map(|(word, _)| *word))
        .collect::<Vec<_>>()
        .join(" ");
    let codec = stream.codec.as_deref().unwrap_or_default();
    let format = match codec {
        "eia_608" => "Closed Captions".to_owned(),
        "idx" => "VOBSUB".to_owned(),
        other => other.to_uppercase(),
    };
    let codec_label = if stream.is_external {
        format!("{format} {}", catalog.external)
    } else {
        format.clone()
    };

    let Some(title) = stream.title.as_deref() else {
        return TrackLabels {
            extended: format!("{display} ({codec_label})"),
            display,
        };
    };
    let found = words(title);
    let mut rest = Vec::new();
    if let (Some(name), Some(tag)) = (&language_name, stream.language.as_deref()) {
        if !names_language(&found, tag) {
            rest.push(name.clone());
        }
    }
    for (word, aliases) in &traits {
        if !aliases
            .iter()
            .any(|alias| found.iter().any(|found| found == alias))
        {
            rest.push((*word).to_owned());
        }
    }
    let format_names: Vec<String> = if codec == "ass" {
        vec!["ass".to_owned(), "ssa".to_owned()]
    } else {
        vec![format.to_lowercase()]
    };
    if !format_names.iter().any(|name| found.contains(name)) {
        rest.push(codec_label);
    }
    TrackLabels {
        extended: extended(title, &rest),
        display,
    }
}

fn unknown(language: Language) -> TrackLabels {
    let word = language.catalog().unknown;
    TrackLabels {
        display: word.to_owned(),
        extended: word.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::client::ClientIdentity;
    use crate::gql_items::{GqlAudio, GqlColor, GqlDolbyVision, GqlTraits, GqlVideo};

    fn in_language(code: &str) -> Language {
        Language::of(&ClientIdentity {
            language: Some(code.to_owned()),
            ..ClientIdentity::default()
        })
    }

    fn track(kind: &str, codec: &str, tag: Option<&str>, title: Option<&str>) -> GqlStream {
        GqlStream {
            kind: kind.to_owned(),
            codec: Some(codec.to_owned()),
            language: tag.map(ToOwned::to_owned),
            title: title.map(ToOwned::to_owned),
            traits: GqlTraits::default(),
            ..GqlStream::default()
        }
    }

    fn video_track(width: i32, height: i32, profile: &str, title: Option<&str>) -> GqlStream {
        GqlStream {
            profile: Some(profile.to_owned()),
            video: Some(GqlVideo {
                width: Some(width),
                height: Some(height),
                scan: Some("PROGRESSIVE".to_owned()),
                ..GqlVideo::default()
            }),
            ..track("VIDEO", "h264", Some("en"), title)
        }
    }

    fn audio_track(
        codec: &str,
        profile: Option<&str>,
        channels: i32,
        layout: &str,
        title: Option<&str>,
    ) -> GqlStream {
        GqlStream {
            profile: profile.map(ToOwned::to_owned),
            channels: Some(channels),
            audio: Some(GqlAudio {
                channel_layout: Some(layout.to_owned()),
                ..GqlAudio::default()
            }),
            ..track("AUDIO", codec, Some("fr"), title)
        }
    }

    fn labelled(stream: &GqlStream, code: &str) -> (String, String) {
        let labels = labels(stream, in_language(code));
        (labels.display, labels.extended)
    }

    #[test]
    fn names_a_language_as_icu_does() {
        // Captured: region by its short name, dropped when it is the language's own, and
        // capitalised only where the locale's menus capitalise.
        assert_eq!(language("en-GB", in_language("fr")).name, "Anglais (R.-U.)");
        assert_eq!(language("en-GB", in_language("de")).name, "Englisch (UK)");
        assert_eq!(language("fr-FR", in_language("fr")).name, "Français");
        assert_eq!(language("pt-BR", in_language("fr")).name, "Portugais");
        assert_eq!(
            language("es-419", in_language("en")).name,
            "Spanish (Latin America)"
        );
        assert_eq!(
            language("nb", in_language("de")).name,
            "Norwegisch (Bokmål)"
        );
        // A code no locale names is shown as stored, still capitalised in French.
        assert_eq!(language("qad", in_language("fr")).name, "Qad");
        assert_eq!(language("qad", in_language("en")).name, "qad");
        // Named by the reference itself, never translated.
        assert_eq!(language("mis", in_language("fr")).name, "Uncoded language");
        assert_eq!(language("mis", in_language("fr")).code, "mis");
        assert_eq!(
            language("und", in_language("de")).name,
            "Undetermined language"
        );
        assert_eq!(language("und", in_language("de")).code, "");
    }

    #[test]
    fn names_a_language_in_itself_for_a_client_that_states_none() {
        // Measured on the reference with no `X-Plex-Language`.
        let none = Language::default();
        for (tag, name) in [
            ("ja", "日本語"),
            ("fr", "Français"),
            ("fr-CA", "Français (Canada)"),
            ("fr-BE", "Français (Belgique)"),
            ("en-GB", "English (UK)"),
            ("es", "Español"),
            ("es-419", "Español (Latinoamérica)"),
            ("ru", "Русский"),
            ("pt-BR", "Português"),
            ("nb", "norsk bokmål"),
            ("ar", "العربية"),
            ("ko", "한국어"),
            ("zxx", "Sans contenu linguistique"),
            ("qaa", "Qaa"),
            ("und", "Undetermined language"),
        ] {
            assert_eq!(language(tag, none).name, name, "{tag}");
        }
        // The rest of a label stays the client's, English when it states none.
        let forced = GqlStream {
            is_forced: true,
            ..track("SUBTITLE", "srt", Some("fr"), None)
        };
        assert_eq!(labels(&forced, none).display, "Français Forced");
    }

    #[test]
    fn gives_the_iso_639_2_t_code() {
        assert_eq!(language("fr", Language::default()).code, "fra");
        assert_eq!(language("de", Language::default()).code, "deu");
        assert_eq!(language("ro", Language::default()).code, "ron");
        assert_eq!(language("fra", Language::default()).code, "fra");
        assert_eq!(language("mul", Language::default()).code, "mul");
        assert_eq!(language("qaa", Language::default()).code, "");
        assert_eq!(language("en-US", Language::default()).tag, "en-US");
    }

    #[test]
    fn labels_video_by_resolution_range_and_codec() {
        assert_eq!(
            labelled(&video_track(1920, 800, "high", None), "fr"),
            ("1080p".to_owned(), "1080p (H.264)".to_owned())
        );
        // Height counts as well as width: 960×720 is 720, 1424×1040 is not 1080.
        assert_eq!(
            labelled(&video_track(960, 720, "high", None), "fr").0,
            "720p"
        );
        assert_eq!(
            labelled(&video_track(1424, 1040, "high", None), "fr").0,
            "720p"
        );
        assert_eq!(
            labelled(&video_track(720, 576, "high", None), "fr").0,
            "480p"
        );
        assert_eq!(labelled(&video_track(720, 404, "high", None), "fr").0, "SD");
        assert_eq!(
            labelled(&video_track(1920, 1080, "constrained baseline", None), "fr").1,
            "1080p (H.264 Constrained Baseline)"
        );

        let mut hdr = video_track(3840, 2160, "main 10", None);
        hdr.codec = Some("hevc".to_owned());
        hdr.video = Some(GqlVideo {
            width: Some(3840),
            height: Some(2160),
            color: GqlColor {
                transfer: Some("smpte2084".to_owned()),
                ..GqlColor::default()
            },
            dolby_vision: Some(GqlDolbyVision::default()),
            ..GqlVideo::default()
        });
        assert_eq!(
            labelled(&hdr, "fr"),
            (
                "4K DoVi/HDR10".to_owned(),
                "4K DoVi/HDR10 (HEVC Main 10)".to_owned()
            )
        );
    }

    #[test]
    fn a_video_title_drops_what_it_already_says() {
        // Captured titles: a space-separated `1080p` and `AVC` say it all; a dotted one
        // does not.
        let full = video_track(
            1920,
            1080,
            "high",
            Some("Comme un Cheveu Sur la Soupe (1957) BDRemux 1080p AVC DTS HDMA 2.0 -BxN"),
        );
        assert_eq!(
            labelled(&full, "fr").1,
            "Comme un Cheveu Sur la Soupe (1957) BDRemux 1080p AVC DTS HDMA 2.0 -BxN"
        );
        let dotted = video_track(
            1280,
            720,
            "high",
            Some("NCIS.S08E21.FRENCH.720p.WEB-DL.H264-AUTHORiTY.mkv"),
        );
        assert_eq!(
            labelled(&dotted, "fr").1,
            "NCIS.S08E21.FRENCH.720p.WEB-DL.H264-AUTHORiTY.mkv (720p H.264)"
        );
        // A name written empty is no name, but the label still opens on it.
        let empty = video_track(1920, 1080, "main", Some(""));
        assert_eq!(labelled(&empty, "fr").1, " (1080p H.264)");
        let sized = video_track(1920, 1080, "high", Some("1920x1080"));
        assert_eq!(labelled(&sized, "fr").1, "1920x1080 (H.264)");
    }

    #[test]
    fn labels_audio_by_language_codec_and_channels() {
        assert_eq!(
            labelled(&audio_track("dca", Some("ma"), 6, "5.1(side)", None), "fr").0,
            "Français (DTS-HD MA 5.1)"
        );
        assert_eq!(
            labelled(&audio_track("dca", None, 8, "7.1", None), "fr").0,
            "Français (DCA 7.1)"
        );
        assert_eq!(
            labelled(
                &audio_track("truehd", Some("dolby truehd + dolby atmos"), 8, "7.1", None),
                "en"
            )
            .0,
            "French (TRUEHD 7.1 + Atmos)"
        );
        let mut described = audio_track("eac3", None, 2, "stereo", None);
        described.traits.is_visual_impaired = true;
        assert_eq!(
            labelled(&described, "de").0,
            "Französisch Audiodeskription (EAC3 Stereo)"
        );
        let mut nameless = audio_track("mp3", None, 2, "stereo", None);
        nameless.language = None;
        assert_eq!(labelled(&nameless, "fr").0, "Inconnu (MP3 Stereo)");
    }

    #[test]
    fn an_audio_title_is_matched_word_by_word() {
        let label = |title: &str, codec: &str, profile: Option<&str>| {
            labelled(
                &audio_track(codec, profile, 6, "5.1(side)", Some(title)),
                "fr",
            )
            .1
        };
        // `Français` names the language; `French` does not, nor does a dotted word.
        assert_eq!(label("VFF", "eac3", None), "VFF (Français EAC3 5.1)");
        assert_eq!(label("French", "eac3", None), "French (Français EAC3 5.1)");
        assert_eq!(
            label("Audio français 5.1", "ac3", None),
            "Audio français 5.1 (AC3)"
        );
        // `FR` is the stored tag; `AC-3` is AC3, `5.1@448` holds 5.1.
        assert_eq!(label("FR-DTS 5.1", "dca", Some("dts")), "FR-DTS 5.1");
        assert_eq!(
            label("VFi - AC-3 5.1", "ac3", None),
            "VFi - AC-3 5.1 (Français)"
        );
        assert_eq!(
            label("ac3 5.1@448 kbps", "ac3", None),
            "ac3 5.1@448 kbps (Français)"
        );
        // `E-AC-3` is not EAC3, `LPCM` is not PCM, `DTS:X` is not DTS.
        assert_eq!(
            label("E-AC-3 5.1", "eac3", None),
            "E-AC-3 5.1 (Français EAC3)"
        );
        assert_eq!(
            label("DTS:X-HDMA 5.1", "dca", None),
            "DTS:X-HDMA 5.1 (Français DCA)"
        );
    }

    #[test]
    fn labels_subtitles_with_their_traits() {
        let mut forced = track("SUBTITLE", "srt", Some("fr"), None);
        forced.is_forced = true;
        assert_eq!(
            labelled(&forced, "fr"),
            (
                "Français Forcé".to_owned(),
                "Français Forcé (SRT)".to_owned()
            )
        );
        forced.title = Some("Forced".to_owned());
        assert_eq!(labelled(&forced, "fr").1, "Forced (Français SRT)");

        let mut sdh = track("SUBTITLE", "srt", Some("en"), Some("SDH"));
        sdh.traits.is_hearing_impaired = true;
        assert_eq!(
            labelled(&sdh, "fr"),
            ("Anglais SME".to_owned(), "SDH (Anglais SRT)".to_owned())
        );

        let mut external = track("SUBTITLE", "srt", Some("fr"), None);
        external.is_external = true;
        assert_eq!(labelled(&external, "fr").1, "Français (SRT Externe)");
        assert_eq!(
            labelled(&track("SUBTITLE", "eia_608", Some("en"), None), "fr").1,
            "Anglais (Closed Captions)"
        );
        // A tag with a region is not named by the bare language code.
        assert_eq!(
            labelled(&track("SUBTITLE", "srt", Some("fr-FR"), Some("FR")), "fr").1,
            "FR (Français SRT)"
        );
        assert_eq!(
            labelled(&track("SUBTITLE", "srt", Some("fr"), Some("FR")), "fr").1,
            "FR (SRT)"
        );
    }
}
