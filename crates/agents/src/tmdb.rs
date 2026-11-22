//! TMDB, for films: one search, then one request carrying every translation, age rating,
//! image and identifier the film has.

use std::collections::HashMap;

use klapz_core::edit::CalendarDate;
use klapz_core::metadata::{ItemKind, RatingKind, TagKind};
use klapz_core::source_metadata::{
    AgentError, Candidate, Certification, Credit, ExternalId, Genre, ImageKind, MatchQuery,
    MetadataSource, ProviderRating, RemoteImage, SourceRecord, Translation,
};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::http::{empty_to_none, Http};
use crate::TmdbCredentials;

const API: &str = "https://api.themoviedb.org/3";
/// Originals: the repo owner keeps every image at the size the provider has it.
const IMAGES: &str = "https://image.tmdb.org/t/p/original";

/// Below this many votes TMDB's average says nothing, and Plex's agent leaves the rating out.
const MIN_VOTES: u32 = 4;

/// The crew jobs Plex's agent credits as writers.
const WRITER_JOBS: &[&str] = &["Writer", "Screenplay", "Author"];

/// TMDB's genre names, by language tag and genre id.
type GenreNames = HashMap<String, HashMap<u64, String>>;

#[derive(Debug)]
pub struct Tmdb {
    http: Http,
    credentials: TmdbCredentials,
    /// A film carries its genres in English only; their names in other languages come from a
    /// list per language, asked once each.
    genre_names: Mutex<GenreNames>,
}

impl Tmdb {
    /// # Errors
    ///
    /// Fails when no HTTP client can be built.
    pub fn new(credentials: TmdbCredentials) -> Result<Self, AgentError> {
        Ok(Self {
            http: Http::new()?,
            credentials,
            genre_names: Mutex::new(HashMap::new()),
        })
    }

    /// Fills in the genre names of every one of `languages` not asked for yet.
    async fn learn_genre_names(&self, languages: &[String]) -> Result<(), AgentError> {
        let mut known = self.genre_names.lock().await;
        for language in languages {
            if known.contains_key(language) {
                continue;
            }
            let list: GenreList = self
                .get("/genre/movie/list", &[("language", language.clone())])
                .await?;
            known.insert(
                language.clone(),
                list.genres
                    .into_iter()
                    .filter_map(|genre| Some((genre.id, empty_to_none(&genre.name?)?)))
                    .collect(),
            );
        }
        Ok(())
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, AgentError> {
        let request = self
            .http
            .client()
            .get(format!("{API}{path}"))
            .bearer_auth(self.credentials.read_token())
            .query(query);
        self.http.json(request).await
    }
}

#[async_trait::async_trait]
impl MetadataSource for Tmdb {
    fn agent(&self) -> &'static str {
        "tmdb"
    }

    async fn search(&self, query: &MatchQuery) -> Result<Vec<Candidate>, AgentError> {
        let mut parameters = vec![
            ("query", query.title.clone()),
            ("language", query.language.clone()),
            ("include_adult", "false".to_owned()),
        ];
        if let Some(year) = query.year {
            parameters.push(("year", year.to_string()));
        }
        let found: SearchPage = self.get("/search/movie", &parameters).await?;
        Ok(found
            .results
            .into_iter()
            .map(|movie| Candidate {
                provider_id: movie.id.to_string(),
                year: CalendarDate::parse(&movie.release_date).map(CalendarDate::year),
                names: [movie.title, movie.original_title]
                    .into_iter()
                    .filter(|name| !name.is_empty())
                    .collect(),
                aliases: Vec::new(),
            })
            .collect())
    }

    async fn fetch(&self, provider_id: &str) -> Result<SourceRecord, AgentError> {
        let movie: Movie = self
            .get(
                &format!("/movie/{provider_id}"),
                &[(
                    "append_to_response",
                    "translations,release_dates,images,external_ids,credits".to_owned(),
                )],
            )
            .await?;
        let languages: Vec<String> = movie
            .translations
            .translations
            .iter()
            .map(MovieTranslation::tag)
            .collect();
        self.learn_genre_names(&languages).await?;
        let names = self.genre_names.lock().await;
        Ok(movie.into_record(&names))
    }
}

#[derive(Deserialize)]
struct SearchPage {
    #[serde(default)]
    results: Vec<SearchMovie>,
}

#[derive(Deserialize)]
struct SearchMovie {
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    original_title: String,
    #[serde(default)]
    release_date: String,
}

#[derive(Deserialize)]
struct Movie {
    id: u64,
    /// In English: the film is asked for without a language, every other one is a translation.
    #[serde(default)]
    title: String,
    #[serde(default)]
    original_title: String,
    #[serde(default)]
    original_language: String,
    #[serde(default)]
    release_date: String,
    runtime: Option<i64>,
    #[serde(default)]
    vote_average: f64,
    #[serde(default)]
    vote_count: u32,
    #[serde(default)]
    production_companies: Vec<Company>,
    #[serde(default)]
    production_countries: Vec<Country>,
    #[serde(default)]
    genres: Vec<NamedId>,
    #[serde(default)]
    credits: Credits,
    #[serde(default)]
    translations: Translations,
    #[serde(default)]
    release_dates: ReleaseDates,
    #[serde(default)]
    images: Images,
    #[serde(default)]
    external_ids: ExternalIds,
}

#[derive(Deserialize)]
struct Company {
    id: u64,
    name: String,
}

#[derive(Deserialize)]
struct NamedId {
    id: u64,
    /// `null` in the lists of some languages, for a genre nobody named in them.
    name: Option<String>,
}

#[derive(Deserialize)]
struct GenreList {
    #[serde(default)]
    genres: Vec<NamedId>,
}

#[derive(Deserialize)]
struct Country {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize, Default)]
struct Credits {
    #[serde(default)]
    cast: Vec<CastMember>,
    #[serde(default)]
    crew: Vec<CrewMember>,
}

#[derive(Deserialize)]
struct CastMember {
    id: u64,
    name: String,
    character: Option<String>,
    #[serde(default)]
    order: u32,
    profile_path: Option<String>,
}

#[derive(Deserialize)]
struct CrewMember {
    id: u64,
    name: String,
    #[serde(default)]
    job: String,
    profile_path: Option<String>,
}

#[derive(Deserialize, Default)]
struct Translations {
    #[serde(default)]
    translations: Vec<MovieTranslation>,
}

#[derive(Deserialize)]
struct MovieTranslation {
    iso_639_1: String,
    #[serde(default)]
    iso_3166_1: String,
    #[serde(default)]
    data: TranslationData,
}

impl MovieTranslation {
    /// `fr-FR`, or `fr` when TMDB names no region.
    fn tag(&self) -> String {
        if self.iso_3166_1.is_empty() {
            self.iso_639_1.clone()
        } else {
            format!("{}-{}", self.iso_639_1, self.iso_3166_1)
        }
    }
}

#[derive(Deserialize, Default)]
struct TranslationData {
    #[serde(default)]
    title: String,
    #[serde(default)]
    overview: String,
    #[serde(default)]
    tagline: String,
}

#[derive(Deserialize, Default)]
struct ReleaseDates {
    #[serde(default)]
    results: Vec<CountryReleases>,
}

#[derive(Deserialize)]
struct CountryReleases {
    iso_3166_1: String,
    #[serde(default)]
    release_dates: Vec<Release>,
}

#[derive(Deserialize)]
struct Release {
    #[serde(default)]
    certification: String,
}

#[derive(Deserialize, Default)]
struct Images {
    #[serde(default)]
    posters: Vec<Image>,
    #[serde(default)]
    backdrops: Vec<Image>,
    #[serde(default)]
    logos: Vec<Image>,
}

#[derive(Deserialize)]
struct Image {
    file_path: String,
    iso_639_1: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
    vote_average: Option<f64>,
    vote_count: Option<i32>,
}

#[derive(Deserialize, Default)]
struct ExternalIds {
    imdb_id: Option<String>,
}

impl Movie {
    fn into_record(self, genre_names: &GenreNames) -> SourceRecord {
        let released = CalendarDate::parse(&self.release_date);
        let genres = genres_in_every_language(&self.genres, genre_names);
        let credits = self.credits.into_credits();
        let rating = (self.vote_count >= MIN_VOTES).then_some(self.vote_average);
        let translations = self
            .translations
            .translations
            .into_iter()
            .map(|t| {
                let language = t.tag();
                // A translation leaves the title empty where it keeps the film's own: the
                // English one in English, the original one everywhere else.
                let title = empty_to_none(&t.data.title).or_else(|| {
                    if t.iso_639_1 == "en" {
                        empty_to_none(&self.title)
                    } else {
                        empty_to_none(&self.original_title)
                    }
                });
                Translation {
                    language,
                    title,
                    summary: empty_to_none(&t.data.overview),
                    tagline: empty_to_none(&t.data.tagline),
                }
            })
            .collect();
        let certifications = self.release_dates.certifications();
        let mut external_ids = Vec::new();
        if let Some(imdb) = self.external_ids.imdb_id.filter(|id| id.starts_with("tt")) {
            external_ids.push(ExternalId {
                source: "imdb".to_owned(),
                value: imdb,
            });
        }
        let images = [
            (ImageKind::Poster, self.images.posters),
            (ImageKind::Background, self.images.backdrops),
            (ImageKind::ClearLogo, self.images.logos),
        ]
        .into_iter()
        .flat_map(|(kind, images)| {
            images
                .into_iter()
                .map(move |image| remote_image(kind, image))
        })
        .collect();
        SourceRecord {
            kind: ItemKind::Movie,
            agent: "tmdb".to_owned(),
            provider_id: self.id.to_string(),
            position: None,
            original_language: empty_to_none(&self.original_language),
            original_title: empty_to_none(&self.original_title),
            translations,
            year: released.map(CalendarDate::year),
            released_on: released.map(CalendarDate::midnight),
            duration_ms: self
                .runtime
                .filter(|minutes| *minutes > 0)
                .map(|m| m * 60_000),
            // Plex's agent credits the company TMDB numbered first, its oldest record.
            studio: self
                .production_companies
                .into_iter()
                .min_by_key(|company| company.id)
                .map(|company| company.name.trim().to_owned()),
            rating,
            certifications,
            external_ids,
            images,
            genres,
            countries: self
                .production_countries
                .into_iter()
                .map(|country| country.name)
                .filter(|name| !name.is_empty())
                .collect(),
            credits,
            ratings: rating
                .map(|value| ProviderRating {
                    source: "themoviedb".to_owned(),
                    kind: RatingKind::Audience,
                    value,
                    image: "themoviedb://image.rating".to_owned(),
                })
                .into_iter()
                .collect(),
            children: Vec::new(),
        }
    }
}

impl ReleaseDates {
    /// Each country's first age rating that is not empty.
    fn certifications(self) -> Vec<Certification> {
        self.results
            .into_iter()
            .filter_map(|country| {
                let rating = country
                    .release_dates
                    .into_iter()
                    .map(|release| release.certification)
                    .find(|certification| !certification.is_empty())?;
                Some(Certification {
                    country: country.iso_3166_1,
                    rating,
                })
            })
            .collect()
    }
}

impl Credits {
    /// Directors, writers and producers in TMDB's order, then the cast in billing order.
    fn into_credits(mut self) -> Vec<Credit> {
        let photo = |path: Option<String>| path.map(|path| format!("{IMAGES}{path}"));
        let mut credits: Vec<Credit> = self
            .crew
            .into_iter()
            .filter_map(|member| {
                let kind = match member.job.as_str() {
                    "Director" => TagKind::Director,
                    "Producer" => TagKind::Producer,
                    job if WRITER_JOBS.contains(&job) => TagKind::Writer,
                    _ => return None,
                };
                Some(Credit {
                    kind,
                    name: member.name,
                    role: None,
                    person_key: format!("tmdb:{}", member.id),
                    photo: photo(member.profile_path),
                })
            })
            .collect();
        self.cast.sort_by_key(|member| member.order);
        credits.extend(self.cast.into_iter().map(|member| Credit {
            kind: TagKind::Actor,
            name: member.name,
            role: member.character.and_then(|role| empty_to_none(&role)),
            person_key: format!("tmdb:{}", member.id),
            photo: photo(member.profile_path),
        }));
        credits
    }
}

/// A film's genres, named in English as the film carries them and in every language whose
/// list names them.
fn genres_in_every_language(genres: &[NamedId], genre_names: &GenreNames) -> Vec<Genre> {
    genres
        .iter()
        .filter_map(|genre| {
            let mut names = vec![("en".to_owned(), genre.name.clone()?)];
            names.extend(genre_names.iter().filter_map(|(language, named)| {
                Some((language.clone(), named.get(&genre.id)?.clone()))
            }));
            Some(Genre { names })
        })
        .collect()
}

fn remote_image(kind: ImageKind, image: Image) -> RemoteImage {
    RemoteImage {
        kind,
        url: format!("{IMAGES}{}", image.file_path),
        // `xx` is TMDB's "no language": an image without text.
        language: image
            .iso_639_1
            .filter(|language| language != "xx" && !language.is_empty()),
        width: image.width,
        height: image.height,
        vote_average: image.vote_average,
        vote_count: image.vote_count,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn reads_a_film_with_every_language_and_its_images() {
        let movie: Movie = serde_json::from_str(
            r#"{
              "id": 603, "title": "The Matrix", "original_title": "The Matrix",
              "original_language": "en", "release_date": "1999-03-30", "runtime": 136,
              "vote_average": 8.2, "vote_count": 25000,
              "production_companies": [{"id": 79, "name": "Village Roadshow"}, {"id": 174, "name": "Warner Bros."}],
              "translations": {"translations": [
                {"iso_639_1": "en", "iso_3166_1": "US", "data": {"title": "", "overview": "Set in the 22nd century", "tagline": "Welcome to the Real World."}},
                {"iso_639_1": "fr", "iso_3166_1": "FR", "data": {"title": "Matrix", "overview": "Programmeur anonyme", "tagline": ""}}
              ]},
              "release_dates": {"results": [
                {"iso_3166_1": "FR", "release_dates": [{"certification": ""}, {"certification": "12"}]}
              ]},
              "images": {"posters": [{"file_path": "/a.jpg", "iso_639_1": "fr", "width": 2000, "height": 3000, "vote_average": 5.3, "vote_count": 4}],
                         "backdrops": [{"file_path": "/b.jpg", "iso_639_1": null}], "logos": []},
              "external_ids": {"imdb_id": "tt0133093"},
              "genres": [{"id": 878, "name": "Science Fiction"}],
              "production_countries": [{"iso_3166_1": "US", "name": "United States of America"}],
              "credits": {
                "cast": [{"id": 2, "name": "Laurence Fishburne", "character": "Morpheus", "order": 1},
                         {"id": 1, "name": "Keanu Reeves", "character": "Neo", "order": 0}],
                "crew": [{"id": 9, "name": "Lana Wachowski", "job": "Director"},
                         {"id": 9, "name": "Lana Wachowski", "job": "Screenplay"},
                         {"id": 7, "name": "Joel Silver", "job": "Producer"},
                         {"id": 8, "name": "Bill Pope", "job": "Director of Photography"}]
              }
            }"#,
        )
        .unwrap();
        let names = HashMap::from([(
            "fr-FR".to_owned(),
            HashMap::from([(878, "Science-Fiction".to_owned())]),
        )]);
        let record = movie.into_record(&names);
        assert_eq!(record.provider_id, "603");
        assert_eq!(record.year, Some(1999));
        assert_eq!(record.duration_ms, Some(8_160_000));
        assert_eq!(record.studio.as_deref(), Some("Village Roadshow"));
        assert_eq!(record.translations[0].title.as_deref(), Some("The Matrix"));
        assert_eq!(record.translations[1].language, "fr-FR");
        assert_eq!(record.translations[1].tagline, None);
        assert_eq!(record.certifications[0].rating, "12");
        assert_eq!(record.external_ids[0].value, "tt0133093");
        assert_eq!(
            record.images[0].url,
            "https://image.tmdb.org/t/p/original/a.jpg"
        );
        assert_eq!(record.images[1].language, None);
        assert_eq!(record.genres[0].names.len(), 2);
        assert_eq!(record.countries, ["United States of America"]);
        let credited: Vec<_> = record
            .credits
            .iter()
            .map(|c| (c.kind, c.name.as_str()))
            .collect();
        assert_eq!(
            credited,
            [
                (TagKind::Director, "Lana Wachowski"),
                (TagKind::Writer, "Lana Wachowski"),
                (TagKind::Producer, "Joel Silver"),
                (TagKind::Actor, "Keanu Reeves"),
                (TagKind::Actor, "Laurence Fishburne"),
            ]
        );
        assert_eq!(record.ratings[0].image, "themoviedb://image.rating");
    }
}
