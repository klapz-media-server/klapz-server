//! Editing items on the official API: `updateMovie`, `updateShow`, `updateEpisode`.
//!
//! Three typed mutations rather than one `updateItem`: each input admits exactly the fields its
//! type publishes — a series has no critics' rating, an episode no studio — so a field an item
//! cannot hold is a schema error the client sees while writing the query, and the payload
//! returns the concrete type with no fragment needed. What they share is in [`update`].
//!
//! An absent input field is left alone, `null` clears it, and so does an empty text. Editing a
//! field does not lock it, as on the reference server; `lock` does, in the same call.

use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Enum, InputObject, MaybeUndefined, Result, SimpleObject, ID};
use open_plex_core::edit::{CalendarDate, EditError, FieldEdit, ItemEdit, ItemField};
use open_plex_core::metadata::{ItemKind, MetadataError, MetadataItem};
use open_plex_core::RatingKey;

use crate::item::{Episode, Movie, Show, ITEM_TYPE};
use crate::loader::{ItemLoader, LockedFieldsLoader};
use crate::node::GlobalId;

/// A field of an item an editor can lock, so that a metadata refresh leaves it as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(name = "ItemField")]
pub enum ItemFieldType {
    /// `title`.
    Title,
    /// `titleSort`.
    TitleSort,
    /// `originalTitle`.
    OriginalTitle,
    /// `studio`.
    Studio,
    /// `rating`, the critics' rating.
    Rating,
    /// `tagline`.
    Tagline,
    /// `summary`.
    Summary,
    /// `contentRating`.
    ContentRating,
    /// The poster.
    Poster,
    /// The background art.
    Background,
    /// `releasedOn`.
    ReleasedOn,
    /// `year`.
    Year,
    /// The genres among `tags`.
    Genres,
    /// The collections the item belongs to.
    Collections,
    /// The directors among `tags`.
    Directors,
    /// The writers among `tags`.
    Writers,
    /// The cast among `tags`.
    Cast,
    /// The producers among `tags`.
    Producers,
    /// The countries among `tags`.
    Countries,
    /// `audienceRating`.
    AudienceRating,
}

impl From<ItemField> for ItemFieldType {
    fn from(field: ItemField) -> Self {
        match field {
            ItemField::Title => Self::Title,
            ItemField::TitleSort => Self::TitleSort,
            ItemField::OriginalTitle => Self::OriginalTitle,
            ItemField::Studio => Self::Studio,
            ItemField::Rating => Self::Rating,
            ItemField::Tagline => Self::Tagline,
            ItemField::Summary => Self::Summary,
            ItemField::ContentRating => Self::ContentRating,
            ItemField::Poster => Self::Poster,
            ItemField::Background => Self::Background,
            ItemField::ReleasedOn => Self::ReleasedOn,
            ItemField::Year => Self::Year,
            ItemField::Genres => Self::Genres,
            ItemField::Collections => Self::Collections,
            ItemField::Directors => Self::Directors,
            ItemField::Writers => Self::Writers,
            ItemField::Cast => Self::Cast,
            ItemField::Producers => Self::Producers,
            ItemField::Countries => Self::Countries,
            ItemField::AudienceRating => Self::AudienceRating,
        }
    }
}

impl From<ItemFieldType> for ItemField {
    fn from(field: ItemFieldType) -> Self {
        match field {
            ItemFieldType::Title => Self::Title,
            ItemFieldType::TitleSort => Self::TitleSort,
            ItemFieldType::OriginalTitle => Self::OriginalTitle,
            ItemFieldType::Studio => Self::Studio,
            ItemFieldType::Rating => Self::Rating,
            ItemFieldType::Tagline => Self::Tagline,
            ItemFieldType::Summary => Self::Summary,
            ItemFieldType::ContentRating => Self::ContentRating,
            ItemFieldType::Poster => Self::Poster,
            ItemFieldType::Background => Self::Background,
            ItemFieldType::ReleasedOn => Self::ReleasedOn,
            ItemFieldType::Year => Self::Year,
            ItemFieldType::Genres => Self::Genres,
            ItemFieldType::Collections => Self::Collections,
            ItemFieldType::Directors => Self::Directors,
            ItemFieldType::Writers => Self::Writers,
            ItemFieldType::Cast => Self::Cast,
            ItemFieldType::Producers => Self::Producers,
            ItemFieldType::Countries => Self::Countries,
            ItemFieldType::AudienceRating => Self::AudienceRating,
        }
    }
}

/// The fields locked on an item, in a stable order.
pub(crate) async fn locked_fields_of(
    context: &Context<'_>,
    item: RatingKey,
) -> Result<Vec<ItemFieldType>> {
    Ok(context
        .data::<DataLoader<LockedFieldsLoader>>()?
        .load_one(item)
        .await?
        .unwrap_or_default()
        .into_iter()
        .map(Into::into)
        .collect())
}

/// Changes to a film.
#[derive(Debug, InputObject)]
pub struct UpdateMovieInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The film, as returned in `Movie.id`.
    pub id: ID,
    /// The title. A new one gives a new `titleSort`, the title less a leading article,
    /// unless `titleSort` is locked or given too.
    pub title: MaybeUndefined<String>,
    /// The title to sort by.
    pub title_sort: MaybeUndefined<String>,
    /// The original-language title.
    pub original_title: MaybeUndefined<String>,
    /// The synopsis.
    pub summary: MaybeUndefined<String>,
    /// The tagline.
    pub tagline: MaybeUndefined<String>,
    /// The production studio.
    pub studio: MaybeUndefined<String>,
    /// The age-rating label, such as `fr/12`.
    pub content_rating: MaybeUndefined<String>,
    /// The release year, from 1 to 9999.
    pub year: MaybeUndefined<i32>,
    /// The release date as `YYYY-MM-DD`. Sets `year` to its own unless `year` is given too;
    /// clearing it clears the year.
    pub released_on: MaybeUndefined<String>,
    /// The critics' rating, from 0 to 10.
    pub rating: MaybeUndefined<f64>,
    /// The audience rating, from 0 to 10.
    pub audience_rating: MaybeUndefined<f64>,
    /// Fields to lock, before the values are written.
    #[graphql(default)]
    pub lock: Vec<ItemFieldType>,
    /// Fields to unlock, before the values are written.
    #[graphql(default)]
    pub unlock: Vec<ItemFieldType>,
}

/// Changes to a series.
#[derive(Debug, InputObject)]
pub struct UpdateShowInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The series, as returned in `Show.id`.
    pub id: ID,
    /// The title. A new one gives a new `titleSort`, the title less a leading article,
    /// unless `titleSort` is locked or given too.
    pub title: MaybeUndefined<String>,
    /// The title to sort by.
    pub title_sort: MaybeUndefined<String>,
    /// The original-language title.
    pub original_title: MaybeUndefined<String>,
    /// The synopsis.
    pub summary: MaybeUndefined<String>,
    /// The tagline.
    pub tagline: MaybeUndefined<String>,
    /// The production studio or network.
    pub studio: MaybeUndefined<String>,
    /// The age-rating label, such as `fr/12`.
    pub content_rating: MaybeUndefined<String>,
    /// The year the series first aired, from 1 to 9999.
    pub year: MaybeUndefined<i32>,
    /// The first-air date as `YYYY-MM-DD`. Sets `year` to its own unless `year` is given too;
    /// clearing it clears the year.
    pub released_on: MaybeUndefined<String>,
    /// The audience rating, from 0 to 10.
    pub audience_rating: MaybeUndefined<f64>,
    /// Fields to lock, before the values are written.
    #[graphql(default)]
    pub lock: Vec<ItemFieldType>,
    /// Fields to unlock, before the values are written.
    #[graphql(default)]
    pub unlock: Vec<ItemFieldType>,
}

/// Changes to an episode.
#[derive(Debug, InputObject)]
pub struct UpdateEpisodeInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The episode, as returned in `Episode.id`.
    pub id: ID,
    /// The title. A new one gives a new `titleSort`, the title less a leading article,
    /// unless `titleSort` is locked or given too.
    pub title: MaybeUndefined<String>,
    /// The title to sort by.
    pub title_sort: MaybeUndefined<String>,
    /// The original-language title.
    pub original_title: MaybeUndefined<String>,
    /// The synopsis.
    pub summary: MaybeUndefined<String>,
    /// The episode's own age-rating label. Cleared, the episode is rated as its series.
    pub content_rating: MaybeUndefined<String>,
    /// The year the episode first aired, from 1 to 9999.
    pub year: MaybeUndefined<i32>,
    /// The original air date as `YYYY-MM-DD`. Sets `year` to its own unless `year` is given
    /// too; clearing it clears the year.
    pub released_on: MaybeUndefined<String>,
    /// The audience rating, from 0 to 10.
    pub audience_rating: MaybeUndefined<f64>,
    /// Fields to lock, before the values are written.
    #[graphql(default)]
    pub lock: Vec<ItemFieldType>,
    /// Fields to unlock, before the values are written.
    #[graphql(default)]
    pub unlock: Vec<ItemFieldType>,
}

/// Why an edit was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ItemEditErrorCode {
    /// No such item of this type, or not one the caller may edit.
    ItemNotFound,
    /// A value the field cannot hold: a date that is not `YYYY-MM-DD`, a rating outside 0 to
    /// 10, a year outside 1 to 9999.
    InvalidValue,
    /// A field locked or unlocked that this type of item does not hold.
    NotApplicable,
    /// A field both locked and unlocked.
    LockedAndUnlocked,
}

/// A problem with what was asked, as opposed to a failure of the server.
#[derive(Debug, Clone, SimpleObject)]
pub struct ItemEditUserError {
    /// The input field at fault, as a path from the input's root.
    pub field: Vec<String>,
    /// What went wrong, for a person.
    pub message: String,
    /// What went wrong, for a program.
    pub code: ItemEditErrorCode,
}

impl ItemEditUserError {
    fn new(field: &str, code: ItemEditErrorCode) -> Self {
        let message = match code {
            ItemEditErrorCode::ItemNotFound => "no such item",
            ItemEditErrorCode::InvalidValue => "this field cannot hold this value",
            ItemEditErrorCode::NotApplicable => "this type of item has no such field",
            ItemEditErrorCode::LockedAndUnlocked => "a field cannot be locked and unlocked",
        };
        Self {
            field: vec!["input".to_owned(), field.to_owned()],
            message: message.to_owned(),
            code,
        }
    }

    fn of(error: EditError) -> Self {
        match error {
            EditError::OutOfRange(field) | EditError::SetTwice(field) => {
                Self::new(input_name(field), ItemEditErrorCode::InvalidValue)
            }
            EditError::NotApplicable(_) => Self::new("lock", ItemEditErrorCode::NotApplicable),
            EditError::LockedAndUnlocked(_) => {
                Self::new("unlock", ItemEditErrorCode::LockedAndUnlocked)
            }
        }
    }
}

/// The input field a value of this field comes from. Only fields with a value input reach
/// here: the others can only be locked.
const fn input_name(field: ItemField) -> &'static str {
    match field {
        ItemField::TitleSort => "titleSort",
        ItemField::OriginalTitle => "originalTitle",
        ItemField::Studio => "studio",
        ItemField::Rating => "rating",
        ItemField::Tagline => "tagline",
        ItemField::Summary => "summary",
        ItemField::ContentRating => "contentRating",
        ItemField::ReleasedOn => "releasedOn",
        ItemField::Year => "year",
        ItemField::AudienceRating => "audienceRating",
        _ => "title",
    }
}

/// What `updateMovie` did.
#[derive(Debug, SimpleObject)]
pub struct UpdateMoviePayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// The film as it now is, unless the edit was refused.
    pub movie: Option<Movie>,
    /// Why the edit was refused. Empty when it was applied.
    pub user_errors: Vec<ItemEditUserError>,
}

/// What `updateShow` did.
#[derive(Debug, SimpleObject)]
pub struct UpdateShowPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// The series as it now is, unless the edit was refused.
    pub show: Option<Show>,
    /// Why the edit was refused. Empty when it was applied.
    pub user_errors: Vec<ItemEditUserError>,
}

/// What `updateEpisode` did.
#[derive(Debug, SimpleObject)]
pub struct UpdateEpisodePayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// The episode as it now is, unless the edit was refused.
    pub episode: Option<Episode>,
    /// Why the edit was refused. Empty when it was applied.
    pub user_errors: Vec<ItemEditUserError>,
}

/// The values an input sets, read off it field by field.
#[derive(Debug, Default)]
struct Values {
    edits: Vec<FieldEdit>,
    errors: Vec<ItemEditUserError>,
}

impl Values {
    fn set<T>(&mut self, value: MaybeUndefined<T>, edit: fn(Option<T>) -> FieldEdit) {
        if let Some(value) = Option::<Option<T>>::from(value) {
            self.edits.push(edit(value));
        }
    }

    fn released_on(&mut self, value: MaybeUndefined<String>) {
        match Option::<Option<String>>::from(value) {
            None => {}
            Some(None) => self.edits.push(FieldEdit::ReleasedOn(None)),
            Some(Some(text)) if text.is_empty() => self.edits.push(FieldEdit::ReleasedOn(None)),
            Some(Some(text)) => match CalendarDate::parse(&text) {
                Some(date) => self.edits.push(FieldEdit::ReleasedOn(Some(date))),
                None => self.errors.push(ItemEditUserError::new(
                    "releasedOn",
                    ItemEditErrorCode::InvalidValue,
                )),
            },
        }
    }
}

/// An input, reduced to what every type's has.
struct Request {
    id: ID,
    values: Values,
    lock: Vec<ItemFieldType>,
    unlock: Vec<ItemFieldType>,
}

/// The film, series or episode an input names, and its kind; edited or refused.
async fn update(
    context: &Context<'_>,
    kind: ItemKind,
    request: Request,
) -> Result<std::result::Result<MetadataItem, ItemEditUserError>> {
    let not_found = || {
        Ok(Err(ItemEditUserError::new(
            "id",
            ItemEditErrorCode::ItemNotFound,
        )))
    };
    let Some(id) = GlobalId::decode(&request.id)
        .ok()
        .filter(|decoded| decoded.type_name == ITEM_TYPE)
        .map(|decoded| RatingKey::new(decoded.key))
    else {
        return not_found();
    };
    // What a caller may not edit answers as if it did not exist, readable or not.
    let Some(item) = crate::schema::readable_item(context, id).await? else {
        return not_found();
    };
    if item.kind != kind || !crate::access::can_edit(context, item.section_id) {
        return not_found();
    }

    let Values { edits, errors } = request.values;
    if let Some(error) = errors.into_iter().next() {
        return Ok(Err(error));
    }
    let edit = match ItemEdit::new(
        kind,
        edits,
        request.lock.into_iter().map(Into::into),
        request.unlock.into_iter().map(Into::into),
    ) {
        Ok(edit) => edit,
        Err(error) => return Ok(Err(ItemEditUserError::of(error))),
    };
    let edited = match crate::schema::metadata(context)?.edit_item(id, &edit).await {
        Ok(edited) => edited,
        // Deleted since it was read.
        Err(MetadataError::ItemNotFound(_)) => return not_found(),
        Err(error) => return Err(error.into()),
    };
    // Either loader may hold the item as it was, if this request read it before.
    context
        .data::<DataLoader<ItemLoader>>()?
        .feed_one(id, edited.item.clone())
        .await;
    context
        .data::<DataLoader<LockedFieldsLoader>>()?
        .feed_one(id, edited.locked)
        .await;
    Ok(Ok(edited.item))
}

pub(crate) async fn update_movie(
    context: &Context<'_>,
    input: UpdateMovieInput,
) -> Result<UpdateMoviePayload> {
    let mut values = Values::default();
    values.set(input.title, FieldEdit::Title);
    values.set(input.title_sort, FieldEdit::TitleSort);
    values.set(input.original_title, FieldEdit::OriginalTitle);
    values.set(input.summary, FieldEdit::Summary);
    values.set(input.tagline, FieldEdit::Tagline);
    values.set(input.studio, FieldEdit::Studio);
    values.set(input.content_rating, FieldEdit::ContentRating);
    values.set(input.year, FieldEdit::Year);
    values.released_on(input.released_on);
    values.set(input.rating, FieldEdit::Rating);
    values.set(input.audience_rating, FieldEdit::AudienceRating);
    let request = Request {
        id: input.id,
        values,
        lock: input.lock,
        unlock: input.unlock,
    };
    let (movie, user_errors) = match update(context, ItemKind::Movie, request).await? {
        Ok(item) => (Some(Movie(item)), Vec::new()),
        Err(error) => (None, vec![error]),
    };
    Ok(UpdateMoviePayload {
        client_mutation_id: input.client_mutation_id,
        movie,
        user_errors,
    })
}

pub(crate) async fn update_show(
    context: &Context<'_>,
    input: UpdateShowInput,
) -> Result<UpdateShowPayload> {
    let mut values = Values::default();
    values.set(input.title, FieldEdit::Title);
    values.set(input.title_sort, FieldEdit::TitleSort);
    values.set(input.original_title, FieldEdit::OriginalTitle);
    values.set(input.summary, FieldEdit::Summary);
    values.set(input.tagline, FieldEdit::Tagline);
    values.set(input.studio, FieldEdit::Studio);
    values.set(input.content_rating, FieldEdit::ContentRating);
    values.set(input.year, FieldEdit::Year);
    values.released_on(input.released_on);
    values.set(input.audience_rating, FieldEdit::AudienceRating);
    let request = Request {
        id: input.id,
        values,
        lock: input.lock,
        unlock: input.unlock,
    };
    let (show, user_errors) = match update(context, ItemKind::Show, request).await? {
        Ok(item) => (Some(Show(item)), Vec::new()),
        Err(error) => (None, vec![error]),
    };
    Ok(UpdateShowPayload {
        client_mutation_id: input.client_mutation_id,
        show,
        user_errors,
    })
}

pub(crate) async fn update_episode(
    context: &Context<'_>,
    input: UpdateEpisodeInput,
) -> Result<UpdateEpisodePayload> {
    let mut values = Values::default();
    values.set(input.title, FieldEdit::Title);
    values.set(input.title_sort, FieldEdit::TitleSort);
    values.set(input.original_title, FieldEdit::OriginalTitle);
    values.set(input.summary, FieldEdit::Summary);
    values.set(input.content_rating, FieldEdit::ContentRating);
    values.set(input.year, FieldEdit::Year);
    values.released_on(input.released_on);
    values.set(input.audience_rating, FieldEdit::AudienceRating);
    let request = Request {
        id: input.id,
        values,
        lock: input.lock,
        unlock: input.unlock,
    };
    let (episode, user_errors) = match update(context, ItemKind::Episode, request).await? {
        Ok(item) => (Some(Episode(item)), Vec::new()),
        Err(error) => (None, vec![error]),
    };
    Ok(UpdateEpisodePayload {
        client_mutation_id: input.client_mutation_id,
        episode,
        user_errors,
    })
}
