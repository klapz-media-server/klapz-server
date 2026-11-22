//! Matching on demand: the results a provider has for a film or a series, matching it to one,
//! refreshing it and forgetting its match. For a library's admins and the server's owners.
//!
//! Matching a series fetches every episode in every language, which can outlast a request, so
//! `matchItem` and `refreshItemMetadata` start the work and answer at once, as the reference
//! does: the item's `refreshedAt` tells when it is done.

use async_graphql::{Context, Enum, InputObject, Result, SimpleObject, ID};
use klapz_core::source_metadata::{Matching, Outcome};
use klapz_core::RatingKey;

use crate::item::{Item, ITEM_TYPE};
use crate::schema::ApiContext;
use crate::GlobalId;

/// A result a provider has for an item, as a search finds it.
#[derive(Debug, SimpleObject)]
pub struct MetadataMatch {
    /// The provider: `tmdb` for films, `tvdb` for series.
    pub agent: String,
    /// The result's id at the provider, which `matchItem` takes.
    pub provider_id: String,
    /// Its title, in the library's language where the provider has it.
    pub name: String,
    /// Its release or first air year, when the provider has one.
    pub year: Option<i32>,
    /// How well it answers the search, out of about 100. From 85, matching takes it unasked.
    pub score: i32,
}

#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum MetadataErrorCode {
    /// No film or series the caller may edit has this id.
    ItemNotFound,
    /// No provider has a key for this kind of library.
    NoProvider,
}

/// A problem with what was asked, as opposed to a failure of the server.
#[derive(Debug, Clone, SimpleObject)]
pub struct MetadataUserError {
    /// The input field at fault, as a path from the input's root.
    pub field: Vec<String>,
    /// What went wrong, for a person.
    pub message: String,
    /// What went wrong, for a program.
    pub code: MetadataErrorCode,
}

impl MetadataUserError {
    fn new(field: &str, code: MetadataErrorCode, message: impl Into<String>) -> Self {
        Self {
            field: vec![field.to_owned()],
            message: message.into(),
            code,
        }
    }

    fn not_found() -> Self {
        Self::new(
            "itemId",
            MetadataErrorCode::ItemNotFound,
            "no such film or series",
        )
    }
}

/// Which film or series, and which provider record to match it to.
#[derive(Debug, InputObject)]
#[allow(
    clippy::struct_field_names,
    reason = "Relay's `clientMutationId` and the API's naming of ids put `id` at every end"
)]
pub struct MatchItemInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The film or series, as returned in its `id`.
    pub item_id: ID,
    /// The record at its library's provider, as `metadataMatches` names it. Without it, the
    /// best search result is taken if it scores high enough.
    pub provider_id: Option<String>,
}

/// Which film or series.
#[derive(Debug, InputObject)]
pub struct ItemMetadataInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The film or series, as returned in its `id`.
    pub item_id: ID,
}

/// What a matching mutation did.
#[derive(Debug, SimpleObject)]
pub struct ItemMetadataPayload {
    /// The value the input carried.
    pub client_mutation_id: Option<String>,
    /// The item as it is now: before the work a mutation started in the background is done.
    pub item: Option<Item>,
    /// Why nothing was done. Empty when it was.
    pub user_errors: Vec<MetadataUserError>,
}

/// The film or series `id` names, if the caller may edit it, and the matching to use.
async fn editable(
    context: &Context<'_>,
    id: &ID,
) -> Result<std::result::Result<(klapz_core::MetadataItem, Matching), MetadataUserError>> {
    let Some(key) = GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == ITEM_TYPE)
        .map(|decoded| RatingKey::new(decoded.key))
    else {
        return Ok(Err(MetadataUserError::not_found()));
    };
    // What a caller may not edit answers as if it did not exist, readable or not.
    let Some(item) = crate::schema::readable_item(context, key).await? else {
        return Ok(Err(MetadataUserError::not_found()));
    };
    if item.parent_id.is_some() || !crate::access::can_edit(context, item.section_id) {
        return Ok(Err(MetadataUserError::not_found()));
    }
    let Some(matching) = context.data::<ApiContext>()?.matching.clone() else {
        return Ok(Err(MetadataUserError::new(
            "itemId",
            MetadataErrorCode::NoProvider,
            "this server has no metadata provider",
        )));
    };
    Ok(Ok((item, matching)))
}

pub(crate) async fn metadata_matches(
    context: &Context<'_>,
    item_id: ID,
    title: Option<String>,
    year: Option<i32>,
) -> Result<Vec<MetadataMatch>> {
    let Ok((item, matching)) = editable(context, &item_id).await? else {
        return Ok(Vec::new());
    };
    let agent = match item.kind {
        klapz_core::metadata::ItemKind::Movie => "tmdb",
        _ => "tvdb",
    };
    let found = matching
        .candidates(item.id, title.as_deref(), year)
        .await
        .map_err(async_graphql::Error::new)?
        .unwrap_or_default();
    Ok(found
        .into_iter()
        .map(|(candidate, score)| MetadataMatch {
            agent: agent.to_owned(),
            name: candidate.names.first().cloned().unwrap_or_default(),
            provider_id: candidate.provider_id,
            year: candidate.year,
            score,
        })
        .collect())
}

/// Starts matching the item in the background: to `provider_id`, or to what it shows already,
/// or to the best result.
async fn start(
    context: &Context<'_>,
    client_mutation_id: Option<String>,
    item_id: &ID,
    provider_id: Option<String>,
) -> Result<ItemMetadataPayload> {
    let (item, matching) = match editable(context, item_id).await? {
        Ok(found) => found,
        Err(error) => {
            return Ok(ItemMetadataPayload {
                client_mutation_id,
                item: None,
                user_errors: vec![error],
            })
        }
    };
    let key = item.id;
    tokio::spawn(async move {
        match matching
            .item(key, provider_id, klapz_core::now_epoch())
            .await
        {
            Ok(Some(Outcome::Matched(matched))) => tracing::info!(
                item = key.get(),
                provider_id = matched.provider_id,
                "matched on demand"
            ),
            Ok(Some(Outcome::Unmatched(_)) | None) => {
                tracing::info!(item = key.get(), "no result scored high enough");
            }
            Err(reason) => tracing::warn!(item = key.get(), %reason, "matching on demand failed"),
        }
    });
    Ok(ItemMetadataPayload {
        client_mutation_id,
        item: Some(Item::from_domain(item)),
        user_errors: Vec::new(),
    })
}

pub(crate) async fn match_item(
    context: &Context<'_>,
    input: MatchItemInput,
) -> Result<ItemMetadataPayload> {
    let provider_id = input.provider_id.filter(|id| !id.trim().is_empty());
    start(
        context,
        input.client_mutation_id,
        &input.item_id,
        provider_id,
    )
    .await
}

pub(crate) async fn refresh_item_metadata(
    context: &Context<'_>,
    input: ItemMetadataInput,
) -> Result<ItemMetadataPayload> {
    start(context, input.client_mutation_id, &input.item_id, None).await
}

pub(crate) async fn unmatch_item(
    context: &Context<'_>,
    input: ItemMetadataInput,
) -> Result<ItemMetadataPayload> {
    let (item, matching) = match editable(context, &input.item_id).await? {
        Ok(found) => found,
        Err(error) => {
            return Ok(ItemMetadataPayload {
                client_mutation_id: input.client_mutation_id,
                item: None,
                user_errors: vec![error],
            })
        }
    };
    matching.repository.unmatch(item.id).await?;
    let item = crate::schema::readable_item(context, item.id).await?;
    Ok(ItemMetadataPayload {
        client_mutation_id: input.client_mutation_id,
        item: item.map(Item::from_domain),
        user_errors: Vec::new(),
    })
}
