//! Jellyfin's `BaseItemDto`, shaped from the official API's items.
//!
//! The reference writes a base set of fields on every item and the others when a client asks
//! for them in `Fields`; an item looked up alone carries them all. Absent values are left out,
//! as the reference leaves them out, except `ChannelId`, which it always writes, null.
//!
//! What this server does not hold is not invented: no artwork (empty image tags, which
//! clients answer with their placeholders), no favorites, no external links. Values are the
//! official API's measurements, put in Jellyfin's units: ticks of 100 ns, bits per second,
//! three-letter languages.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::id;
use crate::time;
use crate::track::{in_order, media_stream, Track};

/// What the official API is asked of an item, wherever it is listed.
pub const ITEM_FIELDS: &str = concat!(
    r"
fragment JellyfinVersion on MediaVersion {
    width height container durationMs bitrate
    files { id path filename sizeBytes durationMs streams { ...JellyfinTrack } }
}
fragment JellyfinItem on Item {
    __typename id title titleSort summary addedAt
    library { id }
    watchState { viewCount isWatched offsetMs lastViewedAt }
    ... on Movie {
        originalTitle year releasedOn contentRating audienceRating rating tagline studio
        durationMs
        versions { ...JellyfinVersion }
        tags @include(if: $people) {
            genres(first: 50) { nodes { id name } }
            directors(first: 20) { nodes { id name } }
            writers(first: 20) { nodes { id name } }
            cast(first: 50) { edges { character node { id name } } }
        }
        externalIds(first: 10) @include(if: $people) { nodes { source value } }
    }
    ... on Show {
        originalTitle year releasedOn contentRating audienceRating tagline studio durationMs
        stats { seasonCount episodeCount watchedEpisodeCount }
        tags(includeEpisodes: false) @include(if: $people) {
            genres(first: 50) { nodes { id name } }
            directors(first: 20) { nodes { id name } }
            writers(first: 20) { nodes { id name } }
            cast(first: 50) { edges { character node { id name } } }
        }
        externalIds(first: 10) @include(if: $people) { nodes { source value } }
    }
    ... on Season {
        number year
        show { id title }
        stats { episodeCount watchedEpisodeCount }
    }
    ... on Episode {
        number year releasedOn contentRating audienceRating durationMs originalTitle
        season { id number title }
        show { id title }
        versions { ...JellyfinVersion }
    }
}
",
    crate::track::TRACK_FIELDS!()
);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    #[serde(rename = "__typename")]
    pub type_name: String,
    pub id: String,
    pub title: Option<String>,
    pub title_sort: Option<String>,
    pub summary: Option<String>,
    pub added_at: String,
    pub library: Option<Reference>,
    pub watch_state: Option<WatchState>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub released_on: Option<String>,
    pub content_rating: Option<String>,
    pub audience_rating: Option<f64>,
    pub rating: Option<f64>,
    pub tagline: Option<String>,
    pub studio: Option<String>,
    pub duration_ms: Option<i64>,
    pub number: Option<i32>,
    #[serde(default)]
    pub versions: Vec<Version>,
    pub stats: Option<Stats>,
    pub tags: Option<Tags>,
    pub external_ids: Option<Nodes<ExternalId>>,
    pub show: Option<Reference>,
    pub season: Option<SeasonReference>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Reference {
    pub id: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeasonReference {
    pub id: String,
    pub number: Option<i32>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchState {
    pub view_count: i64,
    pub is_watched: bool,
    pub offset_ms: Option<i64>,
    pub last_viewed_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(
    clippy::struct_field_names,
    reason = "the official API's names, which serde reads"
)]
pub struct Stats {
    pub season_count: Option<i64>,
    pub episode_count: i64,
    pub watched_episode_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Nodes<T> {
    pub nodes: Vec<T>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Named {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Credit {
    pub character: Option<String>,
    pub node: Named,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Credits {
    pub edges: Vec<Credit>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tags {
    pub genres: Nodes<Named>,
    pub directors: Nodes<Named>,
    pub writers: Nodes<Named>,
    pub cast: Credits,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalId {
    pub source: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub container: Option<String>,
    pub duration_ms: Option<i64>,
    pub bitrate: Option<i64>,
    pub files: Vec<File>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub id: String,
    pub path: String,
    pub filename: String,
    pub size_bytes: Option<i64>,
    pub duration_ms: Option<i64>,
    pub streams: Vec<Track>,
}

/// Which of the optional fields a client asked for. An item looked up alone gets them all.
#[derive(Debug, Clone, Default)]
pub struct Fields {
    all: bool,
    named: HashSet<String>,
}

impl Fields {
    pub fn all() -> Self {
        Self {
            all: true,
            named: HashSet::new(),
        }
    }

    /// The `Fields` parameter's values, compared without regard to case.
    pub fn named<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            all: false,
            named: names.into_iter().map(str::to_ascii_lowercase).collect(),
        }
    }

    pub fn has(&self, name: &str) -> bool {
        self.all || self.named.contains(&name.to_ascii_lowercase())
    }

    /// Whether the official API is to be asked for credits and genres.
    pub fn wants_people(&self) -> bool {
        self.has("People") || self.has("Genres") || self.has("ProviderIds")
    }
}

/// What every item of an answer shares.
#[derive(Debug, Clone)]
pub struct Shaping<'a> {
    pub server_id: &'a str,
    pub fields: &'a Fields,
}

fn jellyfin(global: &str) -> String {
    id::of_global(global).unwrap_or_default()
}

/// Jellyfin's item type for one of ours.
pub fn item_type(type_name: &str) -> &'static str {
    match type_name {
        "Movie" => "Movie",
        "Show" => "Series",
        "Season" => "Season",
        _ => "Episode",
    }
}

fn playable_or_series(item: &Item) -> bool {
    matches!(item.type_name.as_str(), "Movie" | "Show")
}

fn is_playable(item: &Item) -> bool {
    matches!(item.type_name.as_str(), "Movie" | "Episode")
}

/// The item's name, as Jellyfin names an untitled season.
fn name(item: &Item) -> String {
    match (&item.title, item.type_name.as_str(), item.number) {
        (Some(title), _, _) => title.clone(),
        (None, "Season", Some(number)) => format!("Season {number}"),
        (None, _, _) => String::new(),
    }
}

/// The user's state of an item. Folders count what is left to watch beneath them.
fn user_data(item: &Item, jellyfin_id: &str) -> Value {
    let state = item.watch_state.clone().unwrap_or_default();
    let mut data = Map::new();
    if let Some(stats) = &item.stats {
        let unplayed = stats.episode_count - stats.watched_episode_count;
        #[allow(
            clippy::cast_precision_loss,
            reason = "episode counts are far below 2^52"
        )]
        let percentage = if stats.episode_count > 0 {
            stats.watched_episode_count as f64 * 100.0 / stats.episode_count as f64
        } else {
            0.0
        };
        data.insert("PlayedPercentage".to_owned(), json!(percentage));
        data.insert("UnplayedItemCount".to_owned(), json!(unplayed));
    }
    data.insert(
        "PlaybackPositionTicks".to_owned(),
        json!(time::ticks(state.offset_ms.unwrap_or(0))),
    );
    data.insert("PlayCount".to_owned(), json!(state.view_count));
    data.insert("IsFavorite".to_owned(), json!(false));
    if let Some(last) = &state.last_viewed_at {
        data.insert("LastPlayedDate".to_owned(), json!(time::date(last)));
    }
    data.insert("Played".to_owned(), json!(state.is_watched));
    data.insert("Key".to_owned(), json!(id::dashed(jellyfin_id)));
    data.insert("ItemId".to_owned(), json!(jellyfin_id));
    Value::Object(data)
}

/// Shapes one item. `parent` is the Jellyfin id of what it was listed under, when that is
/// known; the item's own parent otherwise.
#[allow(
    clippy::too_many_lines,
    reason = "one field after another, in the reference's order"
)]
pub fn item_dto(item: &Item, shaping: &Shaping<'_>) -> Value {
    let fields = shaping.fields;
    let jellyfin_id = jellyfin(&item.id);
    let playable = is_playable(item);
    let primary = item.versions.first();
    let file = primary.and_then(|version| version.files.first());
    let mut dto = Map::new();
    let mut put = |key: &str, value: Value| {
        dto.insert(key.to_owned(), value);
    };
    put("Name", json!(name(item)));
    put("ServerId", json!(shaping.server_id));
    put("Id", json!(jellyfin_id));
    if fields.has("DateCreated") {
        put("DateCreated", json!(time::date(&item.added_at)));
    }
    if fields.has("CanDelete") {
        put("CanDelete", json!(false));
    }
    if fields.has("CanDownload") {
        put("CanDownload", json!(playable));
    }
    if playable {
        let has_subtitles =
            file.is_some_and(|file| file.streams.iter().any(|track| track.kind == "SUBTITLE"));
        if has_subtitles {
            put("HasSubtitles", json!(true));
        }
        if let Some(container) = primary.and_then(|version| version.container.as_deref()) {
            put("Container", json!(container));
        }
    }
    if fields.has("SortName") {
        let sort = item.title_sort.clone().unwrap_or_else(|| name(item));
        put("SortName", json!(sort.to_lowercase()));
    }
    if let Some(released) = &item.released_on {
        put("PremiereDate", json!(time::date(released)));
    }
    if fields.has("ExternalUrls") {
        put("ExternalUrls", json!([]));
    }
    if playable && fields.has("MediaSources") {
        let sources: Vec<Value> = item
            .versions
            .iter()
            .filter_map(|version| media_source(version, item.duration_ms))
            .collect();
        put("MediaSources", Value::Array(sources));
    }
    if fields.has("Path") {
        if let Some(file) = file {
            put("Path", json!(file.path));
        }
    }
    if fields.has("EnableMediaSourceDisplay") {
        put("EnableMediaSourceDisplay", json!(true));
    }
    if fields.has("OriginalTitle") && playable_or_series(item) {
        // The reference writes a title's own as its original when it knows no other.
        let original = item.original_title.clone().unwrap_or_else(|| name(item));
        put("OriginalTitle", json!(original));
    }
    put("ChannelId", Value::Null);
    if fields.has("Overview") {
        if let Some(summary) = item.summary.as_deref().filter(|text| !text.is_empty()) {
            put("Overview", json!(summary));
        }
    }
    if fields.has("Taglines") {
        let taglines: Vec<&String> = item.tagline.iter().collect();
        put("Taglines", json!(taglines));
    }
    let genres: Vec<&Named> = item
        .tags
        .as_ref()
        .map(|tags| tags.genres.nodes.iter().collect())
        .unwrap_or_default();
    if fields.has("Genres") {
        let names: Vec<&str> = genres.iter().map(|genre| genre.name.as_str()).collect();
        put("Genres", json!(names));
    }
    if let Some(rating) = &item.content_rating {
        put("OfficialRating", json!(rating));
    }
    if let Some(rating) = item.audience_rating {
        put("CommunityRating", json!(rating));
    }
    if let Some(rating) = item.rating {
        // Critics' scores are out of a hundred on Jellyfin, out of ten here.
        put("CriticRating", json!((rating * 10.0).round()));
    }
    if let Some(duration) = item.duration_ms {
        put("RunTimeTicks", json!(time::ticks(duration)));
    }
    if fields.has("PlayAccess") {
        put("PlayAccess", json!("Full"));
    }
    if let Some(year) = item.year {
        put("ProductionYear", json!(year));
    }
    if matches!(item.type_name.as_str(), "Season" | "Episode") {
        if let Some(number) = item.number {
            put("IndexNumber", json!(number));
        }
    }
    if let Some(season) = item.season.as_ref().and_then(|season| season.number) {
        put("ParentIndexNumber", json!(season));
    }
    if fields.has("RemoteTrailers") {
        put("RemoteTrailers", json!([]));
    }
    if fields.has("ProviderIds") {
        let ids: Map<String, Value> = item
            .external_ids
            .iter()
            .flat_map(|ids| &ids.nodes)
            .filter_map(|external| {
                provider_name(&external.source).map(|name| (name.to_owned(), json!(external.value)))
            })
            .collect();
        put("ProviderIds", Value::Object(ids));
    }
    put("IsFolder", json!(!playable));
    if fields.has("ParentId") {
        let parent = match item.type_name.as_str() {
            "Episode" => item.season.as_ref().map(|season| jellyfin(&season.id)),
            "Season" => item.show.as_ref().map(|show| jellyfin(&show.id)),
            _ => item.library.as_ref().map(|library| jellyfin(&library.id)),
        };
        if let Some(parent) = parent {
            put("ParentId", json!(parent));
        }
    }
    put("Type", json!(item_type(&item.type_name)));
    if fields.has("People") {
        put("People", json!(people(item.tags.as_ref())));
    }
    if fields.has("Studios") {
        let studios: Vec<Value> = item
            .studio
            .iter()
            .map(|studio| json!({ "Name": studio }))
            .collect();
        put("Studios", Value::Array(studios));
    }
    if fields.has("Genres") {
        let items: Vec<Value> = genres
            .iter()
            .map(|genre| json!({ "Name": genre.name, "Id": jellyfin(&genre.id) }))
            .collect();
        put("GenreItems", Value::Array(items));
    }
    if fields.has("LocalTrailerCount") {
        put("LocalTrailerCount", json!(0));
    }
    put("UserData", user_data(item, &jellyfin_id));
    if let Some(stats) = &item.stats {
        if fields.has("ChildCount") {
            put(
                "ChildCount",
                json!(stats.season_count.unwrap_or(stats.episode_count)),
            );
        }
        if fields.has("RecursiveItemCount") {
            put("RecursiveItemCount", json!(stats.episode_count));
        }
    }
    if fields.has("SpecialFeatureCount") {
        put("SpecialFeatureCount", json!(0));
    }
    if fields.has("DisplayPreferencesId") {
        put("DisplayPreferencesId", json!(jellyfin_id));
    }
    if fields.has("Tags") {
        put("Tags", json!([]));
    }
    if let Some(show) = &item.show {
        put("SeriesName", json!(show.title.clone().unwrap_or_default()));
        put("SeriesId", json!(jellyfin(&show.id)));
    }
    if let Some(season) = &item.season {
        put("SeasonId", json!(jellyfin(&season.id)));
        let name = season
            .title
            .clone()
            .or_else(|| season.number.map(|n| format!("Season {n}")));
        if let Some(name) = name {
            put("SeasonName", json!(name));
        }
    }
    if playable && fields.has("MediaStreams") {
        let streams: Vec<Value> = file
            .map(|file| in_order(&file.streams).map(media_stream).collect())
            .unwrap_or_default();
        put("MediaStreams", Value::Array(streams));
    }
    if playable {
        put("VideoType", json!("VideoFile"));
    }
    put("ImageTags", json!({}));
    put("BackdropImageTags", json!([]));
    put("ImageBlurHashes", json!({}));
    if fields.has("Chapters") {
        put("Chapters", json!([]));
    }
    put(
        "LocationType",
        json!(if item.type_name == "Season" {
            "Virtual"
        } else {
            "FileSystem"
        }),
    );
    put(
        "MediaType",
        json!(if playable { "Video" } else { "Unknown" }),
    );
    if fields.has("LockedFields") {
        put("LockedFields", json!([]));
        put("LockData", json!(false));
    }
    if playable && fields.has("Width") {
        if let Some(version) = primary {
            if let Some(width) = version.width {
                put("Width", json!(width));
            }
            if let Some(height) = version.height {
                put("Height", json!(height));
            }
        }
    }
    Value::Object(dto)
}

/// Jellyfin's name for a source of external ids, for the ones it knows.
fn provider_name(source: &str) -> Option<&'static str> {
    match source {
        "imdb" => Some("Imdb"),
        "tmdb" => Some("Tmdb"),
        "tvdb" => Some("Tvdb"),
        _ => None,
    }
}

fn people(tags: Option<&Tags>) -> Vec<Value> {
    let Some(tags) = tags else {
        return Vec::new();
    };
    let crew = |list: &Nodes<Named>, kind: &str| -> Vec<Value> {
        list.nodes
            .iter()
            .map(|person| {
                json!({ "Name": person.name, "Id": jellyfin(&person.id), "Role": kind, "Type": kind })
            })
            .collect()
    };
    let mut people = crew(&tags.directors, "Director");
    people.extend(crew(&tags.writers, "Writer"));
    people.extend(tags.cast.edges.iter().map(|credit| {
        let mut person = json!({ "Name": credit.node.name, "Id": jellyfin(&credit.node.id) });
        if let Some(character) = &credit.character {
            person["Role"] = json!(character);
        }
        person["Type"] = json!("Actor");
        person
    }));
    people
}

/// One version as a Jellyfin media source: its first file, which is what plays.
pub fn media_source(version: &Version, item_duration_ms: Option<i64>) -> Option<Value> {
    let file = version.files.first()?;
    let streams: Vec<Value> = in_order(&file.streams).map(media_stream).collect();
    let mut source = json!({
        "Protocol": "File",
        "Id": jellyfin(&file.id),
        "Path": file.path,
        "Type": "Default",
        "Name": file
            .filename
            .rsplit_once('.')
            .map_or(file.filename.as_str(), |(stem, _)| stem),
        "IsRemote": false,
        "ReadAtNativeFramerate": false,
        "IgnoreDts": false,
        "IgnoreIndex": false,
        "GenPtsInput": false,
        "SupportsTranscoding": true,
        "SupportsDirectStream": true,
        "SupportsDirectPlay": true,
        "IsInfiniteStream": false,
        "UseMostCompatibleTranscodingProfile": false,
        "RequiresOpening": false,
        "RequiresClosing": false,
        "RequiresLooping": false,
        "SupportsProbing": true,
        "VideoType": "VideoFile",
        "MediaStreams": streams,
        "MediaAttachments": [],
        "Formats": [],
        "RequiredHttpHeaders": {},
        "TranscodingSubProtocol": "http",
        "HasSegments": false,
    });
    if let Some(container) = &version.container {
        source["Container"] = json!(container);
    }
    if let Some(size) = file.size_bytes {
        source["Size"] = json!(size);
    }
    if let Some(duration) = file
        .duration_ms
        .or(version.duration_ms)
        .or(item_duration_ms)
    {
        source["RunTimeTicks"] = json!(time::ticks(duration));
    }
    if let Some(bitrate) = version.bitrate {
        source["Bitrate"] = json!(bitrate);
    }
    if let Some(audio) = file
        .streams
        .iter()
        .filter(|track| track.kind == "AUDIO")
        .max_by_key(|track| track.is_default)
    {
        source["DefaultAudioStreamIndex"] = json!(audio.position);
    }
    Some(source)
}

/// A page of items, as the reference answers a listing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemsResult {
    pub items: Vec<Value>,
    pub total_record_count: i64,
    pub start_index: u32,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn movie() -> Item {
        Item {
            type_name: "Movie".to_owned(),
            id: open_plex_api::GlobalId::encode("Item", 101),
            title: Some("doodlebug".to_owned()),
            title_sort: None,
            summary: Some("A man chases a bug.".to_owned()),
            added_at: "2017-10-07T15:07:33Z".to_owned(),
            library: Some(Reference {
                id: open_plex_api::GlobalId::encode("Library", 1),
                title: None,
            }),
            watch_state: Some(WatchState {
                view_count: 1,
                is_watched: false,
                offset_ms: Some(60_000),
                last_viewed_at: Some("2026-11-01T10:00:00Z".to_owned()),
            }),
            original_title: None,
            year: Some(1997),
            released_on: Some("1997-06-01T00:00:00Z".to_owned()),
            content_rating: None,
            audience_rating: Some(6.5),
            rating: Some(4.7),
            tagline: None,
            studio: None,
            duration_ms: Some(178_301),
            number: None,
            versions: Vec::new(),
            stats: None,
            tags: None,
            external_ids: None,
            show: None,
            season: None,
        }
    }

    fn listing() -> Fields {
        Fields::named([])
    }

    #[test]
    fn a_listed_film_carries_the_base_fields_and_none_it_was_not_asked_for() {
        let fields = listing();
        let dto = item_dto(
            &movie(),
            &Shaping {
                server_id: "s",
                fields: &fields,
            },
        );
        assert_eq!(dto["Name"], "doodlebug");
        assert_eq!(dto["Id"], id::of("Item", 101).unwrap());
        assert_eq!(dto["Type"], "Movie");
        assert_eq!(dto["IsFolder"], false);
        assert_eq!(dto["MediaType"], "Video");
        assert_eq!(dto["ProductionYear"], 1997);
        assert_eq!(dto["RunTimeTicks"], 1_783_010_000_i64);
        assert_eq!(dto["PremiereDate"], "1997-06-01T00:00:00.0000000Z");
        assert_eq!(dto["CommunityRating"], 6.5);
        assert_eq!(dto["CriticRating"], 47.0);
        assert!(dto["ChannelId"].is_null());
        assert_eq!(dto["ImageTags"], json!({}));
        for absent in [
            "Overview",
            "Path",
            "SortName",
            "MediaSources",
            "MediaStreams",
            "People",
            "Genres",
            "DateCreated",
            "ParentId",
            "IndexNumber",
            "SeriesId",
            "OfficialRating",
        ] {
            assert!(dto.get(absent).is_none(), "{absent}");
        }
    }

    #[test]
    fn asked_fields_come_with_the_item() {
        let fields = Fields::named(["overview", "SortName", "DateCreated", "ParentId"]);
        let dto = item_dto(
            &movie(),
            &Shaping {
                server_id: "s",
                fields: &fields,
            },
        );
        assert_eq!(dto["Overview"], "A man chases a bug.");
        assert_eq!(dto["SortName"], "doodlebug");
        assert_eq!(dto["DateCreated"], "2017-10-07T15:07:33.0000000Z");
        assert_eq!(dto["ParentId"], id::of("Library", 1).unwrap());
    }

    #[test]
    fn user_data_carries_the_position_in_ticks_and_the_dashed_key() {
        let fields = listing();
        let dto = item_dto(
            &movie(),
            &Shaping {
                server_id: "s",
                fields: &fields,
            },
        );
        let data = &dto["UserData"];
        assert_eq!(data["PlaybackPositionTicks"], 600_000_000);
        assert_eq!(data["PlayCount"], 1);
        assert_eq!(data["Played"], false);
        assert_eq!(data["IsFavorite"], false);
        assert_eq!(data["LastPlayedDate"], "2026-11-01T10:00:00.0000000Z");
        assert_eq!(data["Key"], id::dashed(&id::of("Item", 101).unwrap()));
        assert!(data.get("UnplayedItemCount").is_none());
    }

    #[test]
    fn an_unwatched_item_has_no_last_played_date() {
        let mut film = movie();
        film.watch_state = None;
        let fields = listing();
        let dto = item_dto(
            &film,
            &Shaping {
                server_id: "s",
                fields: &fields,
            },
        );
        assert_eq!(dto["UserData"]["PlaybackPositionTicks"], 0);
        assert!(dto["UserData"].get("LastPlayedDate").is_none());
    }

    #[test]
    fn a_series_is_a_folder_counting_what_is_left_to_watch() {
        let mut series = movie();
        series.type_name = "Show".to_owned();
        series.stats = Some(Stats {
            season_count: Some(2),
            episode_count: 4,
            watched_episode_count: 1,
        });
        let fields = Fields::named(["ChildCount", "RecursiveItemCount"]);
        let dto = item_dto(
            &series,
            &Shaping {
                server_id: "s",
                fields: &fields,
            },
        );
        assert_eq!(dto["Type"], "Series");
        assert_eq!(dto["IsFolder"], true);
        assert_eq!(dto["MediaType"], "Unknown");
        assert_eq!(dto["ChildCount"], 2);
        assert_eq!(dto["RecursiveItemCount"], 4);
        assert_eq!(dto["UserData"]["UnplayedItemCount"], 3);
        assert_eq!(dto["UserData"]["PlayedPercentage"], 25.0);
        assert!(dto.get("VideoType").is_none());
    }

    #[test]
    fn an_episode_names_its_series_and_season() {
        let mut episode = movie();
        episode.type_name = "Episode".to_owned();
        episode.number = Some(21);
        episode.show = Some(Reference {
            id: open_plex_api::GlobalId::encode("Item", 201),
            title: Some("90210".to_owned()),
        });
        episode.season = Some(SeasonReference {
            id: open_plex_api::GlobalId::encode("Item", 202),
            number: Some(1),
            title: None,
        });
        let fields = Fields::all();
        let dto = item_dto(
            &episode,
            &Shaping {
                server_id: "s",
                fields: &fields,
            },
        );
        assert_eq!(dto["IndexNumber"], 21);
        assert_eq!(dto["ParentIndexNumber"], 1);
        assert_eq!(dto["SeriesName"], "90210");
        assert_eq!(dto["SeriesId"], id::of("Item", 201).unwrap());
        assert_eq!(dto["SeasonId"], id::of("Item", 202).unwrap());
        assert_eq!(dto["SeasonName"], "Season 1");
        assert_eq!(dto["ParentId"], id::of("Item", 202).unwrap());
    }

    #[test]
    fn an_untitled_season_is_named_by_its_number() {
        let mut season = movie();
        season.type_name = "Season".to_owned();
        season.title = None;
        season.number = Some(3);
        let fields = listing();
        let dto = item_dto(
            &season,
            &Shaping {
                server_id: "s",
                fields: &fields,
            },
        );
        assert_eq!(dto["Name"], "Season 3");
        assert_eq!(dto["LocationType"], "Virtual");
        assert_eq!(dto["IndexNumber"], 3);
    }

    #[test]
    fn a_media_source_is_a_versions_first_file() {
        let mut audio = crate::track::tests::track("AUDIO", "opus");
        audio.position = 1;
        audio.is_default = true;
        let version = Version {
            width: Some(720),
            height: Some(480),
            container: Some("mkv".to_owned()),
            duration_ms: Some(178_301),
            bitrate: Some(667_480),
            files: vec![File {
                id: open_plex_api::GlobalId::encode("MediaFile", 2001),
                path: "/media/doodlebug.1997.VO.H264.mkv".to_owned(),
                filename: "doodlebug.1997.VO.H264.mkv".to_owned(),
                size_bytes: Some(14_876_546),
                duration_ms: None,
                streams: vec![crate::track::tests::track("VIDEO", "h264"), audio],
            }],
        };
        let source = media_source(&version, None).unwrap();
        assert_eq!(source["Id"], id::of("MediaFile", 2001).unwrap());
        assert_eq!(source["Name"], "doodlebug.1997.VO.H264");
        assert_eq!(source["Container"], "mkv");
        assert_eq!(source["Size"], 14_876_546);
        assert_eq!(source["Bitrate"], 667_480);
        assert_eq!(source["RunTimeTicks"], 1_783_010_000_i64);
        assert_eq!(source["DefaultAudioStreamIndex"], 1);
        assert_eq!(source["MediaStreams"].as_array().unwrap().len(), 2);
        let empty = Version {
            files: Vec::new(),
            ..version
        };
        assert!(media_source(&empty, None).is_none());
    }
}
