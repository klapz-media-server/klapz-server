//! Which of the images a record offers an item shows, and the port that keeps them on disk.
//!
//! One image per kind, chosen by language first — a poster or a logo carries text, so the
//! library's language wins; a backdrop sits behind text, so one without any wins — then by
//! Plex's measure of quality: 30 % the votes' average, 70 % their number, each against the
//! best on offer.

use super::{ImageKind, RemoteImage, SourceRecord};
use crate::language::LanguageTag;

/// An image kept on this server's disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredImage {
    /// Where the store keeps it, a name it gave out: `provider/ab/abcd….jpg`.
    pub file: String,
    pub content_type: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub blur_hash: String,
    /// A backdrop's corners, each `0xRRGGBB`: top left, top right, bottom right, bottom left.
    pub corner_colors: Option<[u32; 4]>,
}

#[derive(Debug, thiserror::Error)]
#[error("the image could not be fetched or stored")]
pub struct ArtworkError(#[source] pub Box<dyn std::error::Error + Send + Sync>);

/// Keeps provider images on disk: fetched once, served from there.
#[async_trait::async_trait]
pub trait ArtworkFetcher: Send + Sync + std::fmt::Debug {
    async fn fetch(&self, url: &str) -> Result<StoredImage, ArtworkError>;
}

/// Reads stored images back for clients, scaled down into a box when one is given.
#[async_trait::async_trait]
pub trait ArtworkReader: Send + Sync + std::fmt::Debug {
    /// The bytes of `file`, a name the store gave out, and their media type.
    async fn read(
        &self,
        file: &str,
        scale: Option<(u32, u32)>,
    ) -> Result<(Vec<u8>, String), ArtworkError>;
}

/// The image `record` offers for each kind, as an item of a library in `language` shows it.
pub fn chosen_images(record: &SourceRecord, language: LanguageTag) -> Vec<&RemoteImage> {
    [
        ImageKind::Poster,
        ImageKind::Background,
        ImageKind::ClearLogo,
        ImageKind::Banner,
        ImageKind::Thumb,
    ]
    .into_iter()
    .filter_map(|kind| best_of(record, kind, language))
    .collect()
}

fn best_of(record: &SourceRecord, kind: ImageKind, language: LanguageTag) -> Option<&RemoteImage> {
    let offered: Vec<&RemoteImage> = record.images.iter().filter(|i| i.kind == kind).collect();
    let wanted = language.code();
    let original = record.original_language.as_ref().map(LanguageTag::code);
    let tier = |image: &RemoteImage| -> u8 {
        let language = image.language.as_ref().map(LanguageTag::code);
        let is = |code: &str| language == Some(code);
        match kind {
            // Behind text: one without any, then one the viewer reads.
            ImageKind::Background | ImageKind::Thumb => match language {
                None => 0,
                Some(_) if is(wanted) => 1,
                Some(_) if is("en") => 2,
                Some(_) => 3,
            },
            ImageKind::Poster | ImageKind::ClearLogo | ImageKind::Banner => match language {
                Some(_) if is(wanted) => 0,
                Some(_) if original.is_some_and(is) => 1,
                None => 2,
                Some(_) if is("en") => 2,
                Some(_) => 3,
            },
        }
    };
    let best_average = offered
        .iter()
        .map(|i| average(i))
        .fold(f64::MIN_POSITIVE, f64::max);
    let most_votes = offered
        .iter()
        .filter_map(|i| i.vote_count)
        .max()
        .unwrap_or(0)
        .max(1);
    let quality = |image: &RemoteImage| {
        0.3 * average(image) / best_average
            + 0.7 * f64::from(image.vote_count.unwrap_or(0)) / f64::from(most_votes)
    };
    // The first of the best: ties keep the provider's own order.
    offered.into_iter().reduce(|best, image| {
        let better = tier(image) < tier(best)
            || (tier(image) == tier(best) && quality(image) > quality(best));
        if better {
            image
        } else {
            best
        }
    })
}

/// No votes stand for TMDB's prior, a 5 out of 10, as Plex's agent counts them.
fn average(image: &RemoteImage) -> f64 {
    image
        .vote_average
        .filter(|average| *average > 0.0)
        .unwrap_or(5.0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::metadata::ItemKind;

    fn image(kind: ImageKind, url: &str, language: Option<&str>, votes: i32) -> RemoteImage {
        RemoteImage {
            kind,
            url: url.to_owned(),
            language: language.and_then(LanguageTag::parse),
            width: None,
            height: None,
            vote_average: Some(5.5),
            vote_count: Some(votes),
        }
    }

    fn record(images: Vec<RemoteImage>) -> SourceRecord {
        SourceRecord {
            kind: ItemKind::Movie,
            agent: "tmdb".to_owned(),
            provider_id: "603".to_owned(),
            position: None,
            original_language: LanguageTag::parse("en"),
            original_title: None,
            translations: Vec::new(),
            year: None,
            released_on: None,
            duration_ms: None,
            studio: None,
            rating: None,
            certifications: Vec::new(),
            external_ids: Vec::new(),
            images,
            genres: Vec::new(),
            countries: Vec::new(),
            credits: Vec::new(),
            ratings: Vec::new(),
            children: Vec::new(),
        }
    }

    #[test]
    fn a_poster_in_the_library_language_wins_a_backdrop_without_text_wins() {
        let offered = record(vec![
            image(ImageKind::Poster, "en-poster", Some("en"), 40),
            image(ImageKind::Poster, "fr-poster", Some("fr"), 2),
            image(ImageKind::Background, "fr-backdrop", Some("fr"), 30),
            image(ImageKind::Background, "plain-backdrop", None, 1),
        ]);
        let urls: Vec<&str> = chosen_images(&offered, LanguageTag::parse("fr-FR").unwrap())
            .iter()
            .map(|image| image.url.as_str())
            .collect();
        assert_eq!(urls, ["fr-poster", "plain-backdrop"]);
    }

    #[test]
    fn among_equals_the_most_voted_wins() {
        let offered = record(vec![
            image(ImageKind::Poster, "few", Some("de"), 1),
            image(ImageKind::Poster, "many", Some("de"), 50),
        ]);
        assert_eq!(
            chosen_images(&offered, LanguageTag::parse("fr-FR").unwrap())[0].url,
            "many"
        );
    }
}
