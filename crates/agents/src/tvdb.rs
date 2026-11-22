//! TVDB v4, for series: a search, the series with its translations and artwork, and its
//! episodes in the aired order, once in their original language and once per translation.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use klapz_core::edit::CalendarDate;
use klapz_core::metadata::{ItemKind, TagKind};
use klapz_core::source_metadata::{
    AgentError, Candidate, Credit, ExternalId, Genre, ImageKind, MatchQuery, MetadataSource,
    RemoteImage, SourceRecord, Translation,
};
use serde::Deserialize;
use tokio::sync::{Mutex, OnceCell};

use crate::http::{empty_to_none, Http};
use crate::language::{from_tvdb, to_tvdb};
use crate::TvdbCredentials;

const API: &str = "https://api4.thetvdb.com/v4";

/// TVDB's default order, the one it calls `official`: the aired order.
const ORDER: &str = "official";

/// A series past this many pages of episodes per language is cut there rather than walked
/// forever: TVDB pages by 500, so this is 25,000 episodes.
const MAX_PAGES: u32 = 50;

#[derive(Debug)]
pub struct Tvdb {
    http: Http,
    credentials: TvdbCredentials,
    /// A login lasts a month; one is made on first use and again when TVDB refuses it.
    token: Mutex<Option<String>>,
    artwork_kinds: OnceCell<HashMap<u32, ImageKind>>,
}

impl Tvdb {
    /// # Errors
    ///
    /// Fails when no HTTP client can be built.
    pub fn new(credentials: TvdbCredentials) -> Result<Self, AgentError> {
        Ok(Self {
            http: Http::new()?,
            credentials,
            token: Mutex::new(None),
            artwork_kinds: OnceCell::new(),
        })
    }

    async fn login(&self) -> Result<String, AgentError> {
        let mut body = serde_json::json!({ "apikey": self.credentials.api_key() });
        if let Some(pin) = self.credentials.pin() {
            body["pin"] = serde_json::Value::from(pin);
        }
        let request = self
            .http
            .client()
            .post(format!("{API}/login"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string());
        let answer: Envelope<Login> = self.http.json(request).await?;
        Ok(answer.data.token)
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, AgentError> {
        for attempt in 0..2 {
            let token = {
                let mut held = self.token.lock().await;
                if held.is_none() {
                    *held = Some(self.login().await?);
                }
                held.clone().unwrap_or_default()
            };
            let request = self
                .http
                .client()
                .get(format!("{API}{path}"))
                .bearer_auth(token)
                .query(query);
            match self.http.json::<Envelope<T>>(request).await {
                Err(AgentError::Status(401)) if attempt == 0 => *self.token.lock().await = None,
                answer => return answer.map(|envelope| envelope.data),
            }
        }
        Err(AgentError::Status(401))
    }

    /// Which of TVDB's artwork types are which of ours, for series. Asked once: TVDB numbers
    /// them in a table of its own.
    async fn artwork_kinds(&self) -> Result<&HashMap<u32, ImageKind>, AgentError> {
        self.artwork_kinds
            .get_or_try_init(|| async {
                let types: Vec<ArtworkType> = self.get("/artwork/types", &[]).await?;
                Ok(types
                    .into_iter()
                    .filter(|t| t.record_type == "series")
                    .filter_map(|t| {
                        let kind = match t.slug.as_str() {
                            "posters" => ImageKind::Poster,
                            "backgrounds" | "fanart" => ImageKind::Background,
                            "banners" => ImageKind::Banner,
                            "clearlogo" => ImageKind::ClearLogo,
                            _ => return None,
                        };
                        Some((t.id, kind))
                    })
                    .collect())
            })
            .await
    }

    async fn episodes(
        &self,
        series: &str,
        language: Option<&str>,
    ) -> Result<Vec<Episode>, AgentError> {
        let path = match language {
            Some(language) => format!("/series/{series}/episodes/{ORDER}/{language}"),
            None => format!("/series/{series}/episodes/{ORDER}"),
        };
        let mut all = Vec::new();
        for page in 0..MAX_PAGES {
            let found: EpisodePage = self.get(&path, &[("page", page.to_string())]).await?;
            let last = found.episodes.is_empty();
            all.extend(found.episodes);
            if last {
                break;
            }
        }
        Ok(all)
    }
}

#[async_trait::async_trait]
impl MetadataSource for Tvdb {
    fn agent(&self) -> &'static str {
        "tvdb"
    }

    async fn search(&self, query: &MatchQuery) -> Result<Vec<Candidate>, AgentError> {
        let mut parameters = vec![
            ("query", query.title.clone()),
            ("type", "series".to_owned()),
        ];
        if let Some(year) = query.year {
            parameters.push(("year", year.to_string()));
        }
        let found: Vec<SearchSeries> = self.get("/search", &parameters).await?;
        let wanted = to_tvdb(&query.language);
        Ok(found
            .into_iter()
            .filter_map(|series| {
                let id = series.tvdb_id.filter(|id| !id.is_empty())?;
                let (own, others): (Vec<_>, Vec<_>) = series
                    .translations
                    .into_iter()
                    .partition(|(language, _)| Some(language) == wanted.as_ref());
                let mut names = vec![series.name];
                names.extend(own.into_iter().map(|(_, name)| name));
                names.retain(|name| !name.is_empty());
                let mut aliases: Vec<String> = others.into_iter().map(|(_, name)| name).collect();
                aliases.extend(series.aliases);
                aliases.retain(|name| !name.is_empty());
                Some(Candidate {
                    provider_id: id,
                    names,
                    aliases,
                    year: series.year.and_then(|year| year.parse().ok()),
                })
            })
            .collect())
    }

    async fn fetch(&self, provider_id: &str) -> Result<SourceRecord, AgentError> {
        let series: Series = self
            .get(
                &format!("/series/{provider_id}/extended"),
                &[("meta", "translations".to_owned())],
            )
            .await?;
        let kinds = self.artwork_kinds().await?;
        let original = self.episodes(provider_id, None).await?;
        let mut translated: BTreeMap<String, Vec<Episode>> = BTreeMap::new();
        for language in series.translated_languages() {
            translated.insert(
                language.clone(),
                self.episodes(provider_id, Some(&language)).await?,
            );
        }
        Ok(series.into_record(kinds, &original, &translated))
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Deserialize)]
struct Login {
    token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtworkType {
    id: u32,
    #[serde(default)]
    record_type: String,
    #[serde(default)]
    slug: String,
}

#[derive(Deserialize)]
struct SearchSeries {
    tvdb_id: Option<String>,
    #[serde(default)]
    name: String,
    year: Option<String>,
    #[serde(default)]
    translations: BTreeMap<String, String>,
    #[serde(default)]
    aliases: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Series {
    id: u64,
    #[serde(default)]
    name: String,
    first_aired: Option<String>,
    original_language: Option<String>,
    average_runtime: Option<i64>,
    #[serde(default)]
    artworks: Vec<Artwork>,
    #[serde(default)]
    remote_ids: Vec<RemoteId>,
    #[serde(default)]
    seasons: Vec<Season>,
    #[serde(default)]
    translations: SeriesTranslations,
    /// In English only: TVDB translates no genre.
    #[serde(default)]
    genres: Vec<SeriesGenre>,
    #[serde(default)]
    characters: Vec<Character>,
}

#[derive(Deserialize)]
struct SeriesGenre {
    #[serde(default)]
    name: String,
}

/// A credit on a series: TVDB calls every one a character, an actor's or not.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Character {
    /// The character's name, for an actor.
    name: Option<String>,
    people_id: Option<u64>,
    person_name: Option<String>,
    #[serde(rename = "personImgURL")]
    person_img_url: Option<String>,
    #[serde(default)]
    people_type: String,
    #[serde(default)]
    sort: i64,
}

#[derive(Deserialize)]
struct Artwork {
    image: String,
    language: Option<String>,
    #[serde(rename = "type")]
    kind: u32,
    width: Option<i32>,
    height: Option<i32>,
    score: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteId {
    id: String,
    #[serde(default)]
    source_name: String,
}

#[derive(Deserialize)]
struct Season {
    id: u64,
    number: i32,
    #[serde(rename = "type", default)]
    order: SeasonType,
}

#[derive(Deserialize, Default)]
struct SeasonType {
    #[serde(rename = "type", default)]
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SeriesTranslations {
    #[serde(default)]
    name_translations: Vec<NameTranslation>,
    #[serde(default)]
    overview_translations: Vec<OverviewTranslation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameTranslation {
    language: String,
    #[serde(default)]
    name: String,
    /// TVDB lists a series' other names among its translations, marked so: `Mini Wakfu` is
    /// one for *Wakfu* in French, and is no title.
    is_alias: Option<bool>,
}

#[derive(Deserialize)]
struct OverviewTranslation {
    language: String,
    #[serde(default)]
    overview: String,
}

#[derive(Deserialize)]
struct EpisodePage {
    #[serde(default)]
    episodes: Vec<Episode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Episode {
    id: u64,
    season_number: i32,
    number: i32,
    name: Option<String>,
    overview: Option<String>,
    aired: Option<String>,
    runtime: Option<i64>,
    image: Option<String>,
}

impl Series {
    /// The languages the series is translated into besides its own, as TVDB names them.
    fn translated_languages(&self) -> BTreeSet<String> {
        let original = self.original_language.as_deref().unwrap_or_default();
        self.translations
            .name_translations
            .iter()
            .map(|t| t.language.clone())
            .chain(
                self.translations
                    .overview_translations
                    .iter()
                    .map(|t| t.language.clone()),
            )
            .filter(|language| language != original)
            .collect()
    }

    /// The series' name and overview in each language, aliases left out, and its own name in
    /// its original language where no translation gives one.
    fn texts(&self, original_language: Option<&str>) -> Vec<Translation> {
        let mut texts: BTreeMap<String, Translation> = BTreeMap::new();
        for name in self
            .translations
            .name_translations
            .iter()
            .filter(|name| name.is_alias != Some(true))
        {
            let language = from_tvdb(&name.language);
            let entry = texts
                .entry(language.clone())
                .or_insert_with(|| empty(language));
            entry.title = empty_to_none(&name.name);
        }
        for overview in &self.translations.overview_translations {
            let language = from_tvdb(&overview.language);
            let entry = texts
                .entry(language.clone())
                .or_insert_with(|| empty(language));
            entry.summary = empty_to_none(&overview.overview);
        }
        if let Some(language) = original_language {
            let entry = texts
                .entry(language.to_owned())
                .or_insert_with(|| empty(language.to_owned()));
            if entry.title.is_none() {
                entry.title = empty_to_none(&self.name);
            }
        }
        texts.into_values().collect()
    }

    fn into_record(
        self,
        kinds: &HashMap<u32, ImageKind>,
        original: &[Episode],
        translated: &BTreeMap<String, Vec<Episode>>,
    ) -> SourceRecord {
        let first_aired = self.first_aired.as_deref().and_then(CalendarDate::parse);
        let original_language = self.original_language.as_deref().map(from_tvdb);
        let translations = self.texts(original_language.as_deref());
        let images = self
            .artworks
            .into_iter()
            .filter_map(|artwork| {
                Some(RemoteImage {
                    kind: *kinds.get(&artwork.kind)?,
                    url: artwork.image,
                    language: artwork.language.as_deref().map(from_tvdb),
                    width: artwork.width,
                    height: artwork.height,
                    vote_average: artwork.score,
                    vote_count: None,
                })
            })
            .collect();
        let external_ids = self
            .remote_ids
            .into_iter()
            .filter_map(|remote| {
                let source = match remote.source_name.as_str() {
                    "IMDB" => "imdb",
                    "TheMovieDB.com" => "tmdb",
                    _ => return None,
                };
                Some(ExternalId {
                    source: source.to_owned(),
                    value: remote.id,
                })
            })
            .collect();
        let seasons = self
            .seasons
            .iter()
            .filter(|season| season.order.name == ORDER)
            .map(|season| season_record(season, original, original_language.as_deref(), translated))
            .collect();

        SourceRecord {
            kind: ItemKind::Show,
            agent: "tvdb".to_owned(),
            provider_id: self.id.to_string(),
            position: None,
            original_title: empty_to_none(&self.name),
            original_language,
            translations,
            year: first_aired.map(CalendarDate::year),
            released_on: first_aired.map(CalendarDate::midnight),
            duration_ms: self.average_runtime.filter(|m| *m > 0).map(|m| m * 60_000),
            studio: None,
            rating: None,
            certifications: Vec::new(),
            external_ids,
            images,
            genres: self
                .genres
                .into_iter()
                .filter(|genre| !genre.name.is_empty())
                .map(|genre| Genre {
                    names: vec![("en".to_owned(), genre.name)],
                })
                .collect(),
            countries: Vec::new(),
            credits: credits(self.characters),
            ratings: Vec::new(),
            children: seasons,
        }
    }
}

fn season_record(
    season: &Season,
    episodes: &[Episode],
    original_language: Option<&str>,
    translated: &BTreeMap<String, Vec<Episode>>,
) -> SourceRecord {
    SourceRecord {
        kind: ItemKind::Season,
        agent: "tvdb".to_owned(),
        provider_id: season.id.to_string(),
        position: Some(season.number),
        original_language: original_language.map(str::to_owned),
        original_title: None,
        translations: Vec::new(),
        year: None,
        released_on: None,
        duration_ms: None,
        studio: None,
        rating: None,
        certifications: Vec::new(),
        external_ids: vec![ExternalId {
            source: "tvdb".to_owned(),
            value: season.id.to_string(),
        }],
        images: Vec::new(),
        genres: Vec::new(),
        countries: Vec::new(),
        credits: Vec::new(),
        ratings: Vec::new(),
        children: episodes
            .iter()
            .filter(|episode| episode.season_number == season.number)
            .map(|episode| episode_record(episode, original_language, translated))
            .collect(),
    }
}

fn empty(language: String) -> Translation {
    Translation {
        language,
        title: None,
        summary: None,
        tagline: None,
    }
}

fn episode_record(
    episode: &Episode,
    original_language: Option<&str>,
    translated: &BTreeMap<String, Vec<Episode>>,
) -> SourceRecord {
    let aired = episode.aired.as_deref().and_then(CalendarDate::parse);
    let mut translations: Vec<Translation> = translated
        .iter()
        .filter_map(|(language, episodes)| {
            let same = episodes.iter().find(|other| other.id == episode.id)?;
            let translation = Translation {
                language: from_tvdb(language),
                title: same.name.as_deref().and_then(empty_to_none),
                summary: same.overview.as_deref().and_then(empty_to_none),
                tagline: None,
            };
            (translation.title.is_some() || translation.summary.is_some()).then_some(translation)
        })
        .collect();
    if let Some(language) = original_language {
        translations.push(Translation {
            language: language.to_owned(),
            title: episode.name.as_deref().and_then(empty_to_none),
            summary: episode.overview.as_deref().and_then(empty_to_none),
            tagline: None,
        });
    }
    SourceRecord {
        kind: ItemKind::Episode,
        agent: "tvdb".to_owned(),
        provider_id: episode.id.to_string(),
        position: Some(episode.number),
        original_language: original_language.map(str::to_owned),
        original_title: None,
        translations,
        year: aired.map(CalendarDate::year),
        released_on: aired.map(CalendarDate::midnight),
        duration_ms: episode.runtime.filter(|m| *m > 0).map(|m| m * 60_000),
        studio: None,
        rating: None,
        certifications: Vec::new(),
        external_ids: vec![ExternalId {
            source: "tvdb".to_owned(),
            value: episode.id.to_string(),
        }],
        images: episode
            .image
            .as_deref()
            .and_then(empty_to_none)
            .map(|url| RemoteImage {
                kind: ImageKind::Thumb,
                url,
                language: None,
                width: None,
                height: None,
                vote_average: None,
                vote_count: None,
            })
            .into_iter()
            .collect(),
        genres: Vec::new(),
        countries: Vec::new(),
        credits: Vec::new(),
        ratings: Vec::new(),
        children: Vec::new(),
    }
}

/// The directors, writers, actors and producers among a series' characters, in TVDB's order.
fn credits(mut characters: Vec<Character>) -> Vec<Credit> {
    characters.sort_by_key(|character| character.sort);
    characters
        .into_iter()
        .filter_map(|character| {
            let kind = match character.people_type.as_str() {
                "Director" => TagKind::Director,
                "Writer" => TagKind::Writer,
                "Actor" => TagKind::Actor,
                "Producer" => TagKind::Producer,
                _ => return None,
            };
            Some(Credit {
                kind,
                name: character.person_name.as_deref().and_then(empty_to_none)?,
                role: if kind == TagKind::Actor {
                    character.name.as_deref().and_then(empty_to_none)
                } else {
                    None
                },
                person_key: format!("tvdb:{}", character.people_id?),
                photo: character.person_img_url.as_deref().and_then(empty_to_none),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn reads_a_series_its_seasons_and_translated_episodes() {
        let series: Series = serde_json::from_str(
            r#"{
              "id": 121361, "name": "Game of Thrones", "firstAired": "2011-04-17",
              "originalLanguage": "eng", "averageRuntime": 55,
              "artworks": [{"image": "https://artworks.thetvdb.com/p.jpg", "language": "fra", "type": 2, "width": 680, "height": 1000, "score": 100}],
              "remoteIds": [{"id": "tt0944947", "sourceName": "IMDB"}, {"id": "1399", "sourceName": "TheMovieDB.com"}],
              "seasons": [{"id": 364731, "number": 1, "type": {"type": "official"}}, {"id": 9, "number": 1, "type": {"type": "dvd"}}],
              "genres": [{"name": "Drama"}],
              "characters": [
                {"name": "Tyrion Lannister", "peopleId": 7, "personName": "Peter Dinklage", "peopleType": "Actor", "sort": 2},
                {"name": "Daenerys", "peopleId": 8, "personName": "Emilia Clarke", "peopleType": "Actor", "sort": 1},
                {"name": null, "peopleId": 9, "personName": "Someone", "peopleType": "Crew", "sort": 0}
              ],
              "translations": {
                "nameTranslations": [{"language": "fra", "name": "Le Trône de fer"}],
                "overviewTranslations": [{"language": "eng", "overview": "Seven noble families"}]
              }
            }"#,
        )
        .unwrap();
        let episode = |name: &str| Episode {
            id: 3_254_641,
            season_number: 1,
            number: 1,
            name: Some(name.to_owned()),
            overview: None,
            aired: Some("2011-04-17".to_owned()),
            runtime: Some(62),
            image: None,
        };
        let translated = BTreeMap::from([("fra".to_owned(), vec![episode("L'hiver vient")])]);
        let kinds = HashMap::from([(2, ImageKind::Poster)]);
        let record = series.into_record(&kinds, &[episode("Winter Is Coming")], &translated);

        assert_eq!(record.year, Some(2011));
        let french = record
            .translations
            .iter()
            .find(|t| t.language == "fr")
            .unwrap();
        assert_eq!(french.title.as_deref(), Some("Le Trône de fer"));
        let english = record
            .translations
            .iter()
            .find(|t| t.language == "en")
            .unwrap();
        assert_eq!(english.title.as_deref(), Some("Game of Thrones"));
        assert_eq!(record.external_ids.len(), 2);
        assert_eq!(record.images[0].language.as_deref(), Some("fr"));
        assert_eq!(
            record.children.len(),
            1,
            "the DVD order's season is left out"
        );
        assert_eq!(record.genres[0].names[0].1, "Drama");
        let cast: Vec<_> = record
            .credits
            .iter()
            .map(|c| c.role.as_deref().unwrap())
            .collect();
        assert_eq!(cast, ["Daenerys", "Tyrion Lannister"]);
        let pilot = &record.children[0].children[0];
        assert_eq!(pilot.position, Some(1));
        let titles: Vec<_> = pilot
            .translations
            .iter()
            .map(|t| t.title.clone().unwrap())
            .collect();
        assert_eq!(titles, ["L'hiver vient", "Winter Is Coming"]);
    }
}
