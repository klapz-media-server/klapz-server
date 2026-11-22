//! The caller's favorites, and the mutation that makes or takes one back.
//!
//! Like a rating, a favorite is the caller's and the work's: every copy of a film is one at
//! once, and a request with no caller has none. Only items are made favorites: people,
//! genres, collections and playlists are not.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{Context, Enum, InputObject, Result, SimpleObject, ID};
use klapz_core::watch::{WatchError, WatchRepository};
use klapz_core::{now_epoch, RatingKey, UserId};

use crate::item::{Item, ITEM_TYPE};
use crate::node::GlobalId;
use crate::schema::ApiContext;

/// Loads whether the caller made several works favorites, in one call per viewer.
#[derive(Debug, Clone)]
pub struct FavoriteLoader {
    repository: Arc<dyn WatchRepository>,
}

impl FavoriteLoader {
    pub const fn new(repository: Arc<dyn WatchRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<(UserId, String)> for FavoriteLoader {
    type Value = bool;
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
            let favorites: HashSet<String> = self
                .repository
                .favorites(viewer, &guids)
                .await
                .map_err(Arc::new)?
                .into_iter()
                .collect();
            for guid in guids {
                let favorite = favorites.contains(&guid);
                found.insert((viewer, guid), favorite);
            }
        }
        Ok(found)
    }
}

/// Whether the caller made the work a favorite. False for a request with no caller.
pub(crate) async fn is_favorite(context: &Context<'_>, guid: &str) -> Result<bool> {
    let Some(viewer) = crate::access::viewer_id(context) else {
        return Ok(false);
    };
    Ok(context
        .data::<DataLoader<FavoriteLoader>>()?
        .load_one((viewer, guid.to_owned()))
        .await?
        .unwrap_or(false))
}

/// Why `setFavorite` refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum SetFavoriteErrorCode {
    /// No item the caller may read has this id.
    ItemNotFound,
}

/// A request `setFavorite` refused, and where in the input the reason lies.
#[derive(Debug, Clone, SimpleObject)]
pub struct SetFavoriteUserError {
    /// The path to the input field at fault.
    pub field: Vec<String>,
    /// What went wrong, for a person to read.
    pub message: String,
    /// What went wrong, for a program to branch on.
    pub code: SetFavoriteErrorCode,
}

/// A film, a series, a season or an episode to make a favorite of the caller's, or to take
/// back.
#[derive(Debug, InputObject)]
pub struct SetFavoriteInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The item, as returned in its `id`. The favorite is the work's: every copy of it is
    /// one.
    pub item_id: ID,
    /// True makes it a favorite, false takes it back. Either is accepted when it already
    /// holds, and changes nothing.
    pub favorite: bool,
}

/// What `setFavorite` did.
#[derive(Debug, SimpleObject)]
pub struct SetFavoritePayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The item, unless the request was refused.
    pub item: Option<Item>,
    /// Whether the item is now one of the caller's favorites. False when the request was
    /// refused.
    pub favorite: bool,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<SetFavoriteUserError>,
}

pub(crate) async fn set_favorite(
    context: &Context<'_>,
    input: SetFavoriteInput,
) -> Result<SetFavoritePayload> {
    let key = GlobalId::decode(&input.item_id)
        .ok()
        .filter(|decoded| decoded.type_name == ITEM_TYPE)
        .map(|decoded| RatingKey::new(decoded.key));
    let found = match (crate::access::viewer_id(context), key) {
        (Some(viewer), Some(key)) => crate::schema::readable_item(context, key)
            .await?
            .map(|item| (viewer, item)),
        _ => None,
    };
    let Some((viewer, item)) = found else {
        return Ok(SetFavoritePayload {
            client_mutation_id: input.client_mutation_id,
            item: None,
            favorite: false,
            user_errors: vec![SetFavoriteUserError {
                field: vec!["input".to_owned(), "itemId".to_owned()],
                message: "no such item".to_owned(),
                code: SetFavoriteErrorCode::ItemNotFound,
            }],
        });
    };
    context
        .data::<ApiContext>()?
        .watch
        .set_favorite(viewer, &item.guid, input.favorite, now_epoch())
        .await?;
    // The loader may hold the answer from before the change.
    context
        .data::<DataLoader<FavoriteLoader>>()?
        .feed_one((viewer, item.guid.clone()), input.favorite)
        .await;
    Ok(SetFavoritePayload {
        client_mutation_id: input.client_mutation_id,
        item: Some(Item::from_domain(item)),
        favorite: input.favorite,
        user_errors: Vec::new(),
    })
}
