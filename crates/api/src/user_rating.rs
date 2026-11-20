//! The caller's own rating of a work, and the mutation that changes it.
//!
//! Like watch state, a rating is the caller's and the work's: every copy of a film shares one,
//! and a request with no caller has none. The rules are in `core::user_rating`.

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{Context, Enum, InputObject, Object, Result, SimpleObject, ID};
use klapz_core::user_rating::{self, RateOutcome, RatingError, RatingRecord, UserRating};
use klapz_core::watch::{WatchError, WatchRepository};
use klapz_core::{now_epoch, RatingKey, UserId};

use crate::item::{Item, ITEM_TYPE};
use crate::node::GlobalId;
use crate::schema::ApiContext;
use crate::DateTime;

/// The caller's rating of a work, out of ten.
#[derive(Debug, Clone, Copy)]
pub struct UserRatingObject {
    rating: UserRating,
    rated_at: i64,
}

#[Object(name = "UserRating")]
impl UserRatingObject {
    /// From 0 to 10, at the precision it was given. Plex's stars are halves of the scale:
    /// three and a half stars is 7.
    async fn value(&self) -> f64 {
        self.rating.get()
    }

    /// When the caller rated it: when the rating reached the server, unless the client dated
    /// it.
    async fn rated_at(&self) -> DateTime {
        self.rated_at.into()
    }
}

impl UserRatingObject {
    /// The rating a record holds, none once cleared.
    fn of(record: RatingRecord) -> Option<Self> {
        record.rating.map(|rating| Self {
            rating,
            rated_at: record.at,
        })
    }
}

/// Loads the caller's rating of several works in one call per viewer.
#[derive(Debug, Clone)]
pub struct UserRatingLoader {
    repository: Arc<dyn WatchRepository>,
}

impl UserRatingLoader {
    pub const fn new(repository: Arc<dyn WatchRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<(UserId, String)> for UserRatingLoader {
    type Value = RatingRecord;
    type Error = Arc<WatchError>;

    async fn load(
        &self,
        keys: &[(UserId, String)],
    ) -> Result<HashMap<(UserId, String), Self::Value>, Self::Error> {
        let mut by_viewer: HashMap<UserId, Vec<String>> = HashMap::new();
        for (viewer, guid) in keys {
            by_viewer.entry(*viewer).or_default().push(guid.clone());
        }
        let mut found = HashMap::new();
        for (viewer, guids) in by_viewer {
            for (guid, record) in self
                .repository
                .ratings(viewer, &guids)
                .await
                .map_err(Arc::new)?
            {
                found.insert((viewer, guid), record);
            }
        }
        Ok(found)
    }
}

/// The caller's rating of a work. Null when it has none, and for a request with no caller.
pub(crate) async fn rating_of(
    context: &Context<'_>,
    guid: &str,
) -> Result<Option<UserRatingObject>> {
    let Some(viewer) = crate::access::viewer_id(context) else {
        return Ok(None);
    };
    Ok(context
        .data::<DataLoader<UserRatingLoader>>()?
        .load_one((viewer, guid.to_owned()))
        .await?
        .and_then(UserRatingObject::of))
}

/// Why `rateItem` refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum RateItemErrorCode {
    /// No item the caller may read has this id.
    ItemNotFound,
    /// The rating is not from 0 to 10.
    OffTheScale,
    /// The change is dated after the present.
    InTheFuture,
}

/// A request `rateItem` refused, and where in the input the reason lies.
#[derive(Debug, Clone, SimpleObject)]
pub struct RateItemUserError {
    /// The path to the input field at fault.
    pub field: Vec<String>,
    /// What went wrong, for a person to read.
    pub message: String,
    /// What went wrong, for a program to branch on.
    pub code: RateItemErrorCode,
}

impl RateItemUserError {
    fn new(field: &str, code: RateItemErrorCode) -> Self {
        let message = match code {
            RateItemErrorCode::ItemNotFound => "no such item",
            RateItemErrorCode::OffTheScale => "a rating is from 0 to 10",
            RateItemErrorCode::InTheFuture => "a rating cannot be dated in the future",
        };
        Self {
            field: vec!["input".to_owned(), field.to_owned()],
            message: message.to_owned(),
            code,
        }
    }
}

/// A rating to give a film, a series, a season or an episode, or to clear.
#[derive(Debug, InputObject)]
pub struct RateItemInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The item, as returned in its `id`. The rating is the work's: every copy of it shares
    /// it.
    pub item_id: ID,
    /// From 0 to 10. Null, or absent, clears the caller's rating.
    pub rating: Option<f64>,
    /// When the change was made, for a client that rated while it could not reach the server.
    /// Now when absent. A change dated before the caller's last one is ignored, so that a late
    /// one does not undo a newer rating or clearing.
    pub rated_at: Option<DateTime>,
}

/// What `rateItem` did.
#[derive(Debug, SimpleObject)]
pub struct RateItemPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The item rated, unless the request was refused.
    pub item: Option<Item>,
    /// The caller's rating after the change: null when cleared, or when the request was
    /// refused.
    pub user_rating: Option<UserRatingObject>,
    /// Whether the change was made. False when it was older than the caller's last one, when
    /// there was no rating to clear, and when the request was refused.
    pub applied: bool,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<RateItemUserError>,
}

impl RateItemPayload {
    fn refused(client_mutation_id: Option<String>, error: RateItemUserError) -> Self {
        Self {
            client_mutation_id,
            item: None,
            user_rating: None,
            applied: false,
            user_errors: vec![error],
        }
    }
}

pub(crate) async fn rate_item(
    context: &Context<'_>,
    input: RateItemInput,
) -> Result<RateItemPayload> {
    let refused = |field, code| {
        Ok(RateItemPayload::refused(
            input.client_mutation_id.clone(),
            RateItemUserError::new(field, code),
        ))
    };
    // The value and the date are checked before the item, in the reference's order: a bad
    // value on an item that does not exist is a bad value.
    let Ok(rating) = input.rating.map(UserRating::new).transpose() else {
        return refused("rating", RateItemErrorCode::OffTheScale);
    };
    let now = now_epoch();
    let at = input.rated_at.map_or(now, DateTime::unix_seconds);
    if user_rating::check_time(at, now).is_err() {
        return refused("ratedAt", RateItemErrorCode::InTheFuture);
    }
    let Some(viewer) = crate::access::viewer_id(context) else {
        return refused("itemId", RateItemErrorCode::ItemNotFound);
    };
    let key = GlobalId::decode(&input.item_id)
        .ok()
        .filter(|decoded| decoded.type_name == ITEM_TYPE)
        .map(|decoded| RatingKey::new(decoded.key));
    let item = match key {
        Some(key) => crate::schema::readable_item(context, key).await?,
        None => None,
    };
    let Some(item) = item else {
        return refused("itemId", RateItemErrorCode::ItemNotFound);
    };

    let watch = context.data::<ApiContext>()?.watch.as_ref();
    let outcome = match user_rating::rate(watch, viewer, &item.guid, rating, at, now).await {
        Ok(outcome) => outcome,
        Err(RatingError::Storage(error)) => return Err(error.into()),
        // The value was checked above; the date is checked again by the rule itself.
        Err(RatingError::OffTheScale | RatingError::InTheFuture) => {
            return refused("ratedAt", RateItemErrorCode::InTheFuture)
        }
    };
    // Read straight from storage: the loader may hold the rating from before.
    let record = watch
        .ratings(viewer, std::slice::from_ref(&item.guid))
        .await?
        .into_iter()
        .next()
        .map(|(_, record)| record);
    if let Some(record) = record {
        context
            .data::<DataLoader<UserRatingLoader>>()?
            .feed_one((viewer, item.guid.clone()), record)
            .await;
    }
    Ok(RateItemPayload {
        client_mutation_id: input.client_mutation_id,
        user_rating: record.and_then(UserRatingObject::of),
        item: Some(Item::from_domain(item)),
        applied: matches!(outcome, RateOutcome::Rated | RateOutcome::Cleared),
        user_errors: Vec::new(),
    })
}
