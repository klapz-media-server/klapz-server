//! Source metadata: what metadata providers say about a film or a series, kept apart from the
//! library items that hold it.
//!
//! Two layers. A *source* record is the work as TMDB or TVDB describes it — every language at
//! once, its identifiers, the images on offer — and is shared by every copy of the work in
//! every library. A library item points at one, and shows it in its library's language; what
//! an owner edits on the item stays on the item, under its field locks, and no refresh of the
//! source overwrites it.
//!
//! A source is consulted through [`MetadataSource`], one implementation per provider, which
//! only translates: it searches and fetches. Everything else — which result is taken, how a
//! record lands on items, the identity an item takes — is decided here, once for every
//! provider.

pub mod artwork;
pub mod matching;

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::edit::{derived_title_sort, ItemField};
use crate::metadata::{ItemKind, MetadataItem, RatingKind, TagKind};
use crate::{RatingKey, SectionId};

/// What an item is searched by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchQuery {
    pub kind: ItemKind,
    pub title: String,
    pub year: Option<i32>,
    /// The library's language, `fr-FR`: results are named in it where the provider can.
    pub language: String,
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub provider_id: String,
    /// The names the result goes by in its own right: its title in the query's language, its
    /// original title.
    pub names: Vec<String>,
    /// Names it is also known by — aliases, titles in other languages — which a file may be
    /// named after too, but which another work's title can coincide with.
    pub aliases: Vec<String>,
    pub year: Option<i32>,
}

/// A work as a provider describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceRecord {
    pub kind: ItemKind,
    /// The provider: `tmdb`, `tvdb`.
    pub agent: String,
    pub provider_id: String,
    /// A season's number within its series, an episode's within its season.
    pub position: Option<i32>,
    /// ISO 639-1, `en`.
    pub original_language: Option<String>,
    pub original_title: Option<String>,
    pub translations: Vec<Translation>,
    pub year: Option<i32>,
    /// The release or first air date's midnight UTC, in Unix seconds.
    pub released_on: Option<i64>,
    pub duration_ms: Option<i64>,
    pub studio: Option<String>,
    /// Out of ten.
    pub rating: Option<f64>,
    pub certifications: Vec<Certification>,
    pub external_ids: Vec<ExternalId>,
    pub images: Vec<RemoteImage>,
    pub genres: Vec<Genre>,
    /// Production countries, by their English names as the reference's tags carry them.
    pub countries: Vec<String>,
    /// Cast and crew in billing order.
    pub credits: Vec<Credit>,
    pub ratings: Vec<ProviderRating>,
    /// A series' seasons, a season's episodes.
    pub children: Vec<SourceRecord>,
}

/// A genre, named in every language the provider names it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Genre {
    /// `en` → `Science Fiction`, `fr` → `Science-Fiction`.
    pub names: Vec<(String, String)>,
}

/// Someone credited on a work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
    /// Director, writer, actor or producer; never a genre or a country.
    pub kind: TagKind,
    pub name: String,
    /// The character, for an actor.
    pub role: Option<String>,
    /// Who they are at the provider, `tmdb:819`: what tells two namesakes apart.
    pub person_key: String,
    pub photo: Option<String>,
}

/// A score a provider gives a work, out of ten.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderRating {
    /// `themoviedb`, as Plex names the source.
    pub source: String,
    pub kind: RatingKind,
    pub value: f64,
    /// The badge Plex clients draw, `themoviedb://image.rating`.
    pub image: String,
}

/// A tag as an item will carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedTag {
    pub name: String,
    pub role: Option<String>,
    /// For a credit: the person's provider key and portrait.
    pub person: Option<(String, Option<String>)>,
}

/// A record's texts in one language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translation {
    /// `fr-FR` when the provider names a region, `fr` otherwise.
    pub language: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub tagline: Option<String>,
}

/// An age rating in one country.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certification {
    /// ISO 3166-1 alpha-2, `FR`.
    pub country: String,
    pub rating: String,
}

/// An identifier at a provider: `imdb` / `tt0133093`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExternalId {
    pub source: String,
    pub value: String,
}

/// An image a provider offers, known by its address; nothing is downloaded to know it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteImage {
    pub kind: ImageKind,
    pub url: String,
    /// ISO 639-1; `None` for an image without text, which suits every language.
    pub language: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i32>,
}

/// The kinds of image an item shows, by Plex's names for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ImageKind {
    Poster,
    Background,
    ClearLogo,
    Banner,
    /// An episode's still.
    Thumb,
}

impl ImageKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Poster => "poster",
            Self::Background => "background",
            Self::ClearLogo => "clearLogo",
            Self::Banner => "banner",
            Self::Thumb => "thumb",
        }
    }

    pub fn parse(stored: &str) -> Option<Self> {
        [
            Self::Poster,
            Self::Background,
            Self::ClearLogo,
            Self::Banner,
            Self::Thumb,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == stored)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("the provider could not be reached: {0}")]
    Unreachable(String),
    #[error("the provider answered {0}")]
    Status(u16),
    #[error("the provider's answer could not be read: {0}")]
    Unreadable(String),
}

/// A metadata provider, which searches and fetches and decides nothing.
#[async_trait::async_trait]
pub trait MetadataSource: Send + Sync + std::fmt::Debug {
    /// The provider's name, `tmdb`: what records and identities are filed under.
    fn agent(&self) -> &'static str;

    /// Results in the provider's own order, best first.
    async fn search(&self, query: &MatchQuery) -> Result<Vec<Candidate>, AgentError>;

    /// The whole record, every language, and for a series its seasons and episodes.
    async fn fetch(&self, provider_id: &str) -> Result<SourceRecord, AgentError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SourceMetadataError {
    #[error("no library section with id {0}")]
    SectionNotFound(SectionId),
    #[error("the library holds neither films nor series")]
    UnsupportedKind,
    #[error("no metadata provider is configured for this kind of library")]
    NoSource,
    #[error("source metadata storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// An item to match or refresh, with what its edits locked.
#[derive(Debug, Clone)]
pub struct Target {
    pub item: MetadataItem,
    pub locked: BTreeSet<ItemField>,
}

/// A film, or a series with its seasons and their episodes.
#[derive(Debug, Clone)]
pub struct MatchTarget {
    pub target: Target,
    pub seasons: Vec<SeasonTarget>,
    /// The record the item already shows, which a refresh fetches again rather than searching:
    /// the item's title is by then the record's, and searching it could land elsewhere.
    pub linked: Option<Linked>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    pub agent: String,
    pub provider_id: String,
    pub score: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct SeasonTarget {
    pub target: Target,
    pub episodes: Vec<Target>,
}

/// What a library asks to be matched.
#[derive(Debug, Clone)]
pub struct MatchPlan {
    /// What the library's top level holds: films or series.
    pub kind: ItemKind,
    pub language: String,
    pub targets: Vec<MatchTarget>,
}

/// The stored ids of a saved record, mirroring its tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSource {
    pub id: i64,
    pub children: Vec<SavedSource>,
}

/// An item as a record shows it, ready to be written.
#[derive(Debug, Clone)]
pub struct AppliedItem {
    pub item: MetadataItem,
    pub locked: BTreeSet<ItemField>,
    /// The guid the item had, whose viewers' state follows it to the new one.
    pub previous_guid: String,
    pub source_id: i64,
    pub external_ids: Vec<ExternalId>,
    pub score: Option<i32>,
    /// The item's tags of each kind the record speaks for and no lock keeps, which replace its
    /// tags of that kind.
    pub tags: Vec<(TagKind, Vec<AppliedTag>)>,
    /// The record's ratings, which replace the item's, unless its audience rating is locked.
    pub ratings: Option<Vec<ProviderRating>>,
}

/// Storage port for source records and their items.
#[async_trait::async_trait]
pub trait SourceMetadataRepository: Send + Sync + std::fmt::Debug {
    /// A library's language and its films or series to match: those no record describes yet,
    /// or all of them when `force` is set.
    async fn match_plan(
        &self,
        section: SectionId,
        force: bool,
    ) -> Result<MatchPlan, SourceMetadataError>;

    /// Writes a record and its children over any already stored under the same provider id.
    async fn save(
        &self,
        record: &SourceRecord,
        at: i64,
    ) -> Result<SavedSource, SourceMetadataError>;

    /// The guid an imported item of this kind carries for one of these identifiers: copies of
    /// a work keep one identity, the one Plex gave it.
    async fn imported_guid(
        &self,
        kind: ItemKind,
        ids: &[ExternalId],
    ) -> Result<Option<String>, SourceMetadataError>;

    /// Writes items as their records show them, in one transaction.
    async fn apply(&self, items: &[AppliedItem], at: i64) -> Result<(), SourceMetadataError>;

    /// Records the images items show, each replacing the item's image of its kind, and the
    /// colors of an item's backdrop.
    async fn save_images(&self, images: &[ItemImage], at: i64) -> Result<(), SourceMetadataError>;

    /// A film or a series to match on its own, with its library's language: `None` for an
    /// item that is neither, or no item.
    async fn item_target(
        &self,
        item: RatingKey,
    ) -> Result<Option<(String, MatchTarget)>, SourceMetadataError>;

    /// Forgets the record an item and its seasons and episodes show: their links, identifiers
    /// and images. Each takes back the identity a scan gives, `local://{id}`; its texts stay
    /// as they are until something else is matched.
    async fn unmatch(&self, item: RatingKey) -> Result<(), SourceMetadataError>;

    /// The images these items show.
    async fn images_of(&self, items: &[RatingKey]) -> Result<Vec<ShownImage>, SourceMetadataError>;
}

/// An image an item shows, as clients are told of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownImage {
    pub item: RatingKey,
    pub kind: ImageKind,
    /// The artwork store's name for the file.
    pub file: String,
    pub content_type: String,
    pub width: u32,
    pub height: u32,
    pub blur_hash: String,
    /// When the item took it, in Unix seconds: a new image is a new address for a client's
    /// cache.
    pub chosen_at: i64,
}

/// An image an item shows, kept on disk.
#[derive(Debug, Clone)]
pub struct ItemImage {
    pub item: RatingKey,
    pub kind: ImageKind,
    /// Where the provider has it.
    pub url: String,
    pub stored: artwork::StoredImage,
}

/// What matching a library did.
#[derive(Debug, Default, Clone)]
pub struct MatchReport {
    pub matched: Vec<Matched>,
    /// Items left as they were: no result scored [`matching::ACCEPTED_SCORE`].
    pub unmatched: Vec<Unmatched>,
    /// Items a provider or storage failed on, with the reason.
    pub failed: Vec<(RatingKey, String)>,
}

#[derive(Debug, Clone)]
pub struct Matched {
    pub item: RatingKey,
    pub searched: String,
    pub title: Option<String>,
    pub provider_id: String,
    /// `None` for a refresh of a record matched before the score was kept.
    pub score: Option<i32>,
    pub children: usize,
}

#[derive(Debug, Clone)]
pub struct Unmatched {
    pub item: RatingKey,
    pub searched: String,
    /// The best result's name and score, when there was one.
    pub best: Option<(String, i32)>,
}

/// The provider consulted for each kind of library.
#[derive(Debug, Clone, Default)]
pub struct MetadataSources {
    pub films: Option<Arc<dyn MetadataSource>>,
    pub series: Option<Arc<dyn MetadataSource>>,
}

impl MetadataSources {
    fn for_kind(&self, kind: ItemKind) -> Option<&dyn MetadataSource> {
        match kind {
            ItemKind::Movie => self.films.as_deref(),
            ItemKind::Show => self.series.as_deref(),
            ItemKind::Season | ItemKind::Episode => None,
        }
    }
}

/// What matching needs: where records are kept and whom to ask. Shared by the scans, which
/// match what they add, and by the API, which matches on demand.
#[derive(Debug, Clone)]
pub struct Matching {
    pub repository: Arc<dyn SourceMetadataRepository>,
    pub sources: MetadataSources,
    /// Where the images items show are kept; `None` keeps none.
    pub artwork: Option<Arc<dyn artwork::ArtworkFetcher>>,
}

impl Matching {
    /// [`match_library`] with these.
    ///
    /// # Errors
    ///
    /// As [`match_library`].
    pub async fn library(
        &self,
        section: SectionId,
        force: bool,
        now: i64,
    ) -> Result<MatchReport, SourceMetadataError> {
        match_library(
            self.repository.as_ref(),
            &self.sources,
            self.artwork.as_deref(),
            section,
            force,
            now,
        )
        .await
    }
}

impl Matching {
    /// The results a search for an item finds, best first with their scores: by the item's
    /// own name and year, or by those given. `None` when the item is no film or series.
    ///
    /// # Errors
    ///
    /// When storage fails, or the provider: the reason, causes included.
    pub async fn candidates(
        &self,
        item: RatingKey,
        title: Option<&str>,
        year: Option<i32>,
    ) -> Result<Option<Vec<(Candidate, i32)>>, String> {
        let Some((language, target)) = self
            .repository
            .item_target(item)
            .await
            .map_err(|e| with_causes(&e))?
        else {
            return Ok(None);
        };
        let shown = &target.target.item;
        let source = self
            .sources
            .for_kind(shown.kind)
            .ok_or_else(|| SourceMetadataError::NoSource.to_string())?;
        let named = title.map_or_else(|| shown.title.clone().unwrap_or_default(), str::to_owned);
        let year = year.or(shown.year);
        let mut found: Vec<(Candidate, i32)> = Vec::new();
        for (title, year) in matching::queries(&named, year) {
            let query = MatchQuery {
                kind: shown.kind,
                title,
                year,
                language: language.clone(),
            };
            for candidate in source.search(&query).await.map_err(|e| with_causes(&e))? {
                let score = matching::score(&query, &candidate);
                match found
                    .iter_mut()
                    .find(|(seen, _)| seen.provider_id == candidate.provider_id)
                {
                    Some(seen) if seen.1 < score => seen.1 = score,
                    Some(_) => {}
                    None => found.push((candidate, score)),
                }
            }
        }
        // Stable: equal scores keep the provider's order.
        found.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        Ok(Some(found))
    }

    /// Matches one item: to the record `provider_id` names at its library's provider, else to
    /// the one it shows already, else to the best search result. `None` when the item is no
    /// film or series.
    ///
    /// # Errors
    ///
    /// When storage fails, or the provider: the reason, causes included.
    pub async fn item(
        &self,
        item: RatingKey,
        provider_id: Option<String>,
        now: i64,
    ) -> Result<Option<Outcome>, String> {
        let Some((language, mut target)) = self
            .repository
            .item_target(item)
            .await
            .map_err(|e| with_causes(&e))?
        else {
            return Ok(None);
        };
        let source = self
            .sources
            .for_kind(target.target.item.kind)
            .ok_or_else(|| SourceMetadataError::NoSource.to_string())?;
        if let Some(provider_id) = provider_id {
            target.linked = Some(Linked {
                agent: source.agent().to_owned(),
                provider_id,
                score: None,
            });
        }
        match_one(
            self.repository.as_ref(),
            source,
            self.artwork.as_deref(),
            &target,
            &language,
            now,
        )
        .await
        .map(Some)
    }
}

/// Matches a library's items against its provider and writes what it finds.
///
/// # Errors
///
/// Fails when the library cannot be read; a failure on one item is reported and the others go
/// on.
pub async fn match_library(
    repository: &dyn SourceMetadataRepository,
    sources: &MetadataSources,
    artwork: Option<&dyn artwork::ArtworkFetcher>,
    section: SectionId,
    force: bool,
    now: i64,
) -> Result<MatchReport, SourceMetadataError> {
    let plan = repository.match_plan(section, force).await?;
    let source = sources
        .for_kind(plan.kind)
        .ok_or(SourceMetadataError::NoSource)?;
    let mut report = MatchReport::default();
    for target in plan.targets {
        let key = target.target.item.id;
        match match_one(repository, source, artwork, &target, &plan.language, now).await {
            Ok(Outcome::Matched(matched)) => report.matched.push(matched),
            Ok(Outcome::Unmatched(unmatched)) => report.unmatched.push(unmatched),
            Err(reason) => {
                tracing::warn!(item = key.get(), %reason, "matching failed");
                report.failed.push((key, reason));
            }
        }
    }
    Ok(report)
}

/// What matching one item came to.
#[derive(Debug, Clone)]
pub enum Outcome {
    Matched(Matched),
    Unmatched(Unmatched),
}

async fn match_one(
    repository: &dyn SourceMetadataRepository,
    source: &dyn MetadataSource,
    artwork: Option<&dyn artwork::ArtworkFetcher>,
    target: &MatchTarget,
    language: &str,
    now: i64,
) -> Result<Outcome, String> {
    let item = &target.target.item;
    let named = item.title.clone().unwrap_or_default();
    let (provider_id, score) = match &target.linked {
        Some(linked) if linked.agent == source.agent() => {
            (linked.provider_id.clone(), linked.score)
        }
        _ => match best_candidate(source, item, &named, language).await? {
            Some((candidate, score)) if score >= matching::ACCEPTED_SCORE => {
                (candidate.provider_id, Some(score))
            }
            best => {
                return Ok(Outcome::Unmatched(Unmatched {
                    item: item.id,
                    searched: named,
                    best: best.map(|(candidate, score)| {
                        (candidate.names.first().cloned().unwrap_or_default(), score)
                    }),
                }))
            }
        },
    };

    let record = source
        .fetch(&provider_id)
        .await
        .map_err(|e| with_causes(&e))?;
    let saved = repository
        .save(&record, now)
        .await
        .map_err(|e| with_causes(&e))?;
    let mut records = vec![&record];
    let mut applied =
        vec![applied_item(repository, &target.target, &record, &saved, language, score).await?];
    for season in &target.seasons {
        let Some((season_record, season_saved)) =
            child_at(&record, &saved, season.target.item.position)
        else {
            continue;
        };
        records.push(season_record);
        applied.push(
            applied_item(
                repository,
                &season.target,
                season_record,
                season_saved,
                language,
                None,
            )
            .await?,
        );
        for episode in &season.episodes {
            if let Some((episode_record, episode_saved)) =
                child_at(season_record, season_saved, episode.item.position)
            {
                records.push(episode_record);
                applied.push(
                    applied_item(
                        repository,
                        episode,
                        episode_record,
                        episode_saved,
                        language,
                        None,
                    )
                    .await?,
                );
            }
        }
    }
    repository
        .apply(&applied, now)
        .await
        .map_err(|e| with_causes(&e))?;
    if let Some(artwork) = artwork {
        let images = fetch_images(artwork, &applied, &records, language).await;
        repository
            .save_images(&images, now)
            .await
            .map_err(|e| with_causes(&e))?;
    }
    Ok(Outcome::Matched(Matched {
        item: item.id,
        searched: named,
        title: applied[0].item.title.clone(),
        provider_id: record.provider_id,
        score,
        children: applied.len() - 1,
    }))
}

/// The image of each kind every item shows, fetched; one that cannot be is left out, and the
/// item keeps the one it had.
async fn fetch_images(
    artwork: &dyn artwork::ArtworkFetcher,
    applied: &[AppliedItem],
    records: &[&SourceRecord],
    language: &str,
) -> Vec<ItemImage> {
    let mut images = Vec::new();
    for (item, record) in applied.iter().zip(records) {
        for image in artwork::chosen_images(record, language) {
            let locked = match image.kind {
                ImageKind::Poster => item.locked.contains(&ItemField::Poster),
                ImageKind::Background => item.locked.contains(&ItemField::Background),
                ImageKind::ClearLogo | ImageKind::Banner | ImageKind::Thumb => false,
            };
            if locked {
                continue;
            }
            match artwork.fetch(&image.url).await {
                Ok(stored) => images.push(ItemImage {
                    item: item.item.id,
                    kind: image.kind,
                    url: image.url.clone(),
                    stored,
                }),
                Err(error) => tracing::warn!(
                    item = item.item.id.get(),
                    kind = image.kind.as_str(),
                    reason = with_causes(&error),
                    "an image could not be kept"
                ),
            }
        }
    }
    images
}

/// The best scored result of every search worth making for an item named `named`.
async fn best_candidate(
    source: &dyn MetadataSource,
    item: &MetadataItem,
    named: &str,
    language: &str,
) -> Result<Option<(Candidate, i32)>, String> {
    let mut best: Option<(Candidate, i32)> = None;
    for (title, year) in matching::queries(named, item.year) {
        let query = MatchQuery {
            kind: item.kind,
            title,
            year,
            language: language.to_owned(),
        };
        let candidates = source.search(&query).await.map_err(|e| with_causes(&e))?;
        for candidate in candidates {
            let score = matching::score(&query, &candidate);
            if best.as_ref().is_none_or(|(_, top)| score > *top) {
                best = Some((candidate, score));
            }
        }
    }
    Ok(best)
}

/// An error and every cause under it, `storage failed: UNIQUE constraint failed: …`: what a
/// report needs to be acted on.
fn with_causes(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(inner) = cause {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        cause = inner.source();
    }
    text
}

fn child_at<'a>(
    record: &'a SourceRecord,
    saved: &'a SavedSource,
    position: Option<i32>,
) -> Option<(&'a SourceRecord, &'a SavedSource)> {
    let position = position?;
    record
        .children
        .iter()
        .zip(&saved.children)
        .find(|(child, _)| child.position == Some(position))
}

async fn applied_item(
    repository: &dyn SourceMetadataRepository,
    target: &Target,
    record: &SourceRecord,
    saved: &SavedSource,
    language: &str,
    score: Option<i32>,
) -> Result<AppliedItem, String> {
    let mut external_ids = record.external_ids.clone();
    let own = ExternalId {
        source: record.agent.clone(),
        value: record.provider_id.clone(),
    };
    if !external_ids.contains(&own) {
        external_ids.insert(0, own);
    }
    let guid = match repository
        .imported_guid(record.kind, &external_ids)
        .await
        .map_err(|e| with_causes(&e))?
    {
        Some(guid) => guid,
        None => klapz_guid(record),
    };
    let mut item = project(&target.item, &target.locked, record, language);
    item.guid = guid;
    Ok(AppliedItem {
        item,
        locked: target.locked.clone(),
        previous_guid: target.item.guid.clone(),
        source_id: saved.id,
        external_ids,
        score,
        tags: tags_for(record, &target.locked, language),
        ratings: (!record.ratings.is_empty()
            && !target.locked.contains(&ItemField::AudienceRating))
        .then(|| record.ratings.clone()),
    })
}

/// The identity of a work no imported item knows: its provider and its id there.
pub fn klapz_guid(record: &SourceRecord) -> String {
    let kind = match record.kind {
        ItemKind::Movie => "movie",
        ItemKind::Show => "show",
        ItemKind::Season => "season",
        ItemKind::Episode => "episode",
    };
    format!("klapz://{kind}/{}-{}", record.agent, record.provider_id)
}

/// `item` as `record` shows it in `language`, its locked fields untouched.
pub fn project(
    item: &MetadataItem,
    locked: &BTreeSet<ItemField>,
    record: &SourceRecord,
    language: &str,
) -> MetadataItem {
    let mut out = item.clone();
    let open = |field| !locked.contains(&field);
    let text = translation_for(
        &record.translations,
        language,
        record.original_language.as_deref(),
    );
    // A season keeps no title of its own, as on the reference: clients name it by its number.
    if record.kind != ItemKind::Season {
        // In the work's own language its original title is the title, whatever translation
        // the fallback would otherwise reach: a French film TMDB only translated into English.
        let in_original_language = record
            .original_language
            .as_deref()
            .is_some_and(|original| primary_language(original) == primary_language(language));
        let translated = text
            .filter(|text| primary_language(&text.language) == primary_language(language))
            .and_then(|text| text.title.clone());
        let title = translated
            .or_else(|| {
                in_original_language
                    .then(|| record.original_title.clone())
                    .flatten()
            })
            .or_else(|| text.and_then(|text| text.title.clone()))
            .or_else(|| record.original_title.clone());
        if let Some(title) = title {
            if open(ItemField::Title) {
                out.title = Some(title);
                if open(ItemField::TitleSort) {
                    out.title_sort = out.title.as_deref().and_then(derived_title_sort);
                }
            }
        }
    }
    if open(ItemField::OriginalTitle) {
        out.original_title = record
            .original_title
            .clone()
            .filter(|original| out.title.as_deref() != Some(original.as_str()));
    }
    if let Some(summary) = text.and_then(|text| text.summary.clone()) {
        if open(ItemField::Summary) {
            out.summary = Some(summary);
        }
    }
    if let Some(tagline) = text.and_then(|text| text.tagline.clone()) {
        if open(ItemField::Tagline) {
            out.tagline = Some(tagline);
        }
    }
    if record.studio.is_some() && open(ItemField::Studio) {
        out.studio.clone_from(&record.studio);
    }
    if let Some(rating) = content_rating(&record.certifications, language) {
        if open(ItemField::ContentRating) {
            out.content_rating = Some(rating);
        }
    }
    if record.released_on.is_some() && open(ItemField::ReleasedOn) {
        out.released_on = record.released_on;
    }
    if record.year.is_some() && open(ItemField::Year) {
        out.year = record.year;
    }
    if record.rating.is_some() && open(ItemField::Rating) {
        out.rating = record.rating;
    }
    out
}

/// The tags `record` gives an item of a library in `language`, by kind: genres named in that
/// language where the provider has it, credits as billed. A kind the record has nothing for,
/// or that an edit locked, is left out, so the item keeps what it has.
pub fn tags_for(
    record: &SourceRecord,
    locked: &BTreeSet<ItemField>,
    language: &str,
) -> Vec<(TagKind, Vec<AppliedTag>)> {
    let wanted = primary_language(language);
    let plain = |name: String| AppliedTag {
        name,
        role: None,
        person: None,
    };
    let genres: Vec<AppliedTag> = record
        .genres
        .iter()
        .filter_map(|genre| {
            let named = |code: &str| {
                genre
                    .names
                    .iter()
                    .find(|(language, _)| primary_language(language) == code)
            };
            named(&wanted)
                .or_else(|| named("en"))
                .or_else(|| genre.names.first())
                .map(|(_, name)| plain(name.clone()))
        })
        .collect();
    let mut tags = vec![
        (TagKind::Genre, genres),
        (
            TagKind::Country,
            record.countries.iter().cloned().map(plain).collect(),
        ),
    ];
    for kind in [
        TagKind::Director,
        TagKind::Writer,
        TagKind::Actor,
        TagKind::Producer,
    ] {
        let mut seen = BTreeSet::new();
        let credited = record
            .credits
            .iter()
            .filter(|credit| credit.kind == kind && seen.insert(credit.person_key.as_str()))
            .map(|credit| AppliedTag {
                name: credit.name.clone(),
                role: credit.role.clone(),
                person: Some((credit.person_key.clone(), credit.photo.clone())),
            })
            .collect();
        tags.push((kind, credited));
    }
    tags.retain(|(kind, found)| !found.is_empty() && !locked.contains(&lock_of(*kind)));
    tags
}

const fn lock_of(kind: TagKind) -> ItemField {
    match kind {
        TagKind::Genre => ItemField::Genres,
        TagKind::Country => ItemField::Countries,
        TagKind::Director => ItemField::Directors,
        TagKind::Writer => ItemField::Writers,
        TagKind::Actor => ItemField::Cast,
        TagKind::Producer => ItemField::Producers,
    }
}

/// The texts to show in `language` (`fr-FR`): that language and region, else the language in
/// any region, else the work's original language, else English, else whatever there is.
pub fn translation_for<'a>(
    translations: &'a [Translation],
    language: &str,
    original: Option<&str>,
) -> Option<&'a Translation> {
    let primary = primary_language;
    let wanted = primary(language);
    let usable = |t: &&Translation| t.title.is_some() || t.summary.is_some();
    translations
        .iter()
        .filter(usable)
        .find(|t| t.language.eq_ignore_ascii_case(language))
        .or_else(|| {
            translations
                .iter()
                .filter(usable)
                .find(|t| primary(&t.language) == wanted)
        })
        .or_else(|| {
            let original = original.map(primary)?;
            translations
                .iter()
                .filter(usable)
                .find(|t| primary(&t.language) == original)
        })
        .or_else(|| {
            translations
                .iter()
                .filter(usable)
                .find(|t| primary(&t.language) == "en")
        })
        .or_else(|| translations.iter().find(usable))
}

/// `fr` for `fr-FR`.
fn primary_language(tag: &str) -> String {
    tag.split('-').next().unwrap_or(tag).to_ascii_lowercase()
}

/// The age rating for the library's country, as Plex writes it: bare for the United States,
/// prefixed by the country elsewhere (`fr/12`).
fn content_rating(certifications: &[Certification], language: &str) -> Option<String> {
    let country = language.split_once('-')?.1;
    let found = certifications
        .iter()
        .find(|c| c.country.eq_ignore_ascii_case(country) && !c.rating.is_empty())?;
    Some(if country.eq_ignore_ascii_case("US") {
        found.rating.clone()
    } else {
        format!("{}/{}", country.to_ascii_lowercase(), found.rating)
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn translation(language: &str, title: &str) -> Translation {
        Translation {
            language: language.to_owned(),
            title: Some(title.to_owned()),
            summary: None,
            tagline: None,
        }
    }

    fn film() -> MetadataItem {
        MetadataItem {
            id: RatingKey::new(7),
            section_id: SectionId::new(2),
            parent_id: None,
            kind: ItemKind::Movie,
            guid: "local://7".to_owned(),
            title: Some("Matrix 1999 VFF".to_owned()),
            title_sort: None,
            original_title: None,
            summary: None,
            tagline: None,
            studio: None,
            content_rating: None,
            content_rating_age: None,
            position: Some(1),
            year: Some(1999),
            released_on: None,
            duration_ms: None,
            rating: None,
            audience_rating: None,
            added_at: 0,
            updated_at: 0,
            refreshed_at: None,
            chapter_source: None,
        }
    }

    fn record() -> SourceRecord {
        SourceRecord {
            kind: ItemKind::Movie,
            agent: "tmdb".to_owned(),
            provider_id: "603".to_owned(),
            position: None,
            original_language: Some("en".to_owned()),
            original_title: Some("The Matrix".to_owned()),
            translations: vec![
                translation("en-US", "The Matrix"),
                translation("fr-FR", "Matrix"),
            ],
            year: Some(1999),
            released_on: Some(922_838_400),
            duration_ms: None,
            studio: Some("Warner Bros. Pictures".to_owned()),
            rating: Some(8.2),
            certifications: vec![
                Certification {
                    country: "US".to_owned(),
                    rating: "R".to_owned(),
                },
                Certification {
                    country: "FR".to_owned(),
                    rating: "12".to_owned(),
                },
            ],
            external_ids: Vec::new(),
            images: Vec::new(),
            genres: vec![Genre {
                names: vec![
                    ("en".to_owned(), "Science Fiction".to_owned()),
                    ("fr".to_owned(), "Science-Fiction".to_owned()),
                ],
            }],
            countries: vec!["United States of America".to_owned()],
            credits: vec![
                Credit {
                    kind: TagKind::Actor,
                    name: "Keanu Reeves".to_owned(),
                    role: Some("Neo".to_owned()),
                    person_key: "tmdb:6384".to_owned(),
                    photo: None,
                },
                Credit {
                    kind: TagKind::Director,
                    name: "Lana Wachowski".to_owned(),
                    role: None,
                    person_key: "tmdb:9340".to_owned(),
                    photo: None,
                },
            ],
            ratings: Vec::new(),
            children: Vec::new(),
        }
    }

    #[test]
    fn tags_name_genres_in_the_library_language_and_skip_locked_kinds() {
        let locked = BTreeSet::from([ItemField::Directors]);
        let tags = tags_for(&record(), &locked, "fr-FR");
        let kinds: Vec<TagKind> = tags.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(kinds, [TagKind::Genre, TagKind::Country, TagKind::Actor]);
        assert_eq!(tags[0].1[0].name, "Science-Fiction");
        assert_eq!(tags[2].1[0].role.as_deref(), Some("Neo"));
        let german = tags_for(&record(), &BTreeSet::new(), "de-DE");
        assert_eq!(german[0].1[0].name, "Science Fiction");
    }

    #[test]
    fn picks_the_library_language_then_falls_back() {
        let all = [
            translation("en-US", "The Matrix"),
            translation("fr-CA", "La Matrice"),
        ];
        assert_eq!(
            translation_for(&all, "fr-FR", Some("en"))
                .unwrap()
                .title
                .as_deref(),
            Some("La Matrice")
        );
        assert_eq!(
            translation_for(&all, "de-DE", Some("en"))
                .unwrap()
                .title
                .as_deref(),
            Some("The Matrix")
        );
        let japanese = [
            translation("ja-JP", "マトリックス"),
            translation("en-US", "The Matrix"),
        ];
        assert_eq!(
            translation_for(&japanese, "de-DE", Some("ja"))
                .unwrap()
                .title
                .as_deref(),
            Some("マトリックス")
        );
    }

    #[test]
    fn shows_the_record_in_the_library_language() {
        let shown = project(&film(), &BTreeSet::new(), &record(), "fr-FR");
        assert_eq!(shown.title.as_deref(), Some("Matrix"));
        assert_eq!(shown.original_title.as_deref(), Some("The Matrix"));
        assert_eq!(shown.content_rating.as_deref(), Some("fr/12"));
        assert_eq!(shown.studio.as_deref(), Some("Warner Bros. Pictures"));
        let american = project(&film(), &BTreeSet::new(), &record(), "en-US");
        assert_eq!(american.title.as_deref(), Some("The Matrix"));
        assert_eq!(american.original_title, None);
        assert_eq!(american.content_rating.as_deref(), Some("R"));
    }

    #[test]
    fn a_work_in_the_library_language_keeps_its_original_title() {
        let mut french = record();
        french.original_language = Some("fr".to_owned());
        french.original_title = Some("Le mal aimé".to_owned());
        french.translations = vec![translation("en-US", "Unloved")];
        let shown = project(&film(), &BTreeSet::new(), &french, "fr-FR");
        assert_eq!(shown.title.as_deref(), Some("Le mal aimé"));
        assert_eq!(shown.original_title, None);
        let american = project(&film(), &BTreeSet::new(), &french, "en-US");
        assert_eq!(american.title.as_deref(), Some("Unloved"));
    }

    #[test]
    fn a_locked_field_keeps_its_value() {
        let mut edited = film();
        edited.title = Some("Mon Matrix".to_owned());
        let locked = BTreeSet::from([ItemField::Title, ItemField::Studio]);
        let shown = project(&edited, &locked, &record(), "fr-FR");
        assert_eq!(shown.title.as_deref(), Some("Mon Matrix"));
        assert_eq!(shown.studio, None);
        assert_eq!(shown.year, Some(1999));
    }

    #[test]
    fn a_new_work_is_named_after_its_provider() {
        assert_eq!(klapz_guid(&record()), "klapz://movie/tmdb-603");
    }
}
