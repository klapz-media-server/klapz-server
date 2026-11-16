//! `/:/rate`: the caller's star rating of an item, bound onto the official API's `rateItem`.
//!
//! Measured on the reference's test libraries: `rating=-1` clears, any other value is kept and
//! published to one decimal, `ratedAt` dates the change, and the answer is an empty `200` typed
//! `text/html`, as `/:/scrobble`'s. Parameters are checked before the item: a missing one, a
//! value that is no number, a `ratedAt` of `0`, a fraction or in the future are `400`s, then a
//! foreign `identifier` or an unknown `key` a `404`. Declared `PUT` by the spec; the reference
//! answers `GET`, `POST` and `DELETE` alike.
//!
//! Where it is laxer, we refuse: a rating off the scale — above 10, a negative other than `-1`,
//! `inf`, `nan` — is a `400` here, where the reference stores what it is sent, nulling the
//! rating on a negative; a negative `ratedAt`, which the reference accepts as a very old
//! change, is a `400` like `0`; and a `key` that is no number a `404`, as for a scrobble,
//! where the reference fails with a `500`.

use async_graphql::Variables;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::{date_time, GlobalId};
use serde::Deserialize;
use serde_json::json;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::items::LIBRARY_IDENTIFIER;
use crate::PlexState;

const RATE: &str = r"
mutation PlexRate($input: RateItemInput!) {
  rateItem(input: $input) { userErrors { code } }
}
";

/// The rating Plex clients send to clear one: Plex Web's "no stars".
const CLEARED: f64 = -1.0;

pub fn add_routes(router: Router<PlexState>) -> Router<PlexState> {
    router.route("/:/rate", get(rate).put(rate).post(rate).delete(rate))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateParams {
    identifier: Option<String>,
    key: Option<String>,
    rating: Option<String>,
    rated_at: Option<String>,
}

/// A request read, before the item is looked for.
#[derive(Debug, PartialEq)]
struct RateRequest {
    key: i64,
    /// `None` clears.
    rating: Option<f64>,
    rated_at: Option<i64>,
}

/// Reads the parameters in the reference's order: every one present and well formed, then
/// the provider, then the key.
fn read(params: &RateParams) -> Result<RateRequest, ApiError> {
    let (Some(identifier), Some(key), Some(rating)) = (
        params.identifier.as_deref(),
        params.key.as_deref(),
        params.rating.as_deref(),
    ) else {
        return Err(ApiError::BadRequest);
    };
    let rating: f64 = rating.parse().map_err(|_| ApiError::BadRequest)?;
    let rated_at = params
        .rated_at
        .as_deref()
        .map(|at| at.parse::<i64>().ok().filter(|at| *at > 0))
        .map(|at| at.ok_or(ApiError::BadRequest))
        .transpose()?;
    if identifier != LIBRARY_IDENTIFIER {
        return Err(ApiError::NotFound);
    }
    let key = key.parse().map_err(|_| ApiError::NotFound)?;
    #[allow(
        clippy::float_cmp,
        reason = "-1 is a sentinel clients send exactly; -1.0 parses to the same value"
    )]
    let cleared = rating == CLEARED;
    Ok(RateRequest {
        key,
        rating: (!cleared).then_some(rating),
        rated_at,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateData {
    rate_item: RateResult,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateResult {
    user_errors: Vec<UserError>,
}

#[derive(Debug, Deserialize)]
struct UserError {
    code: String,
}

async fn rate(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<RateParams>,
) -> Result<impl IntoResponse, ApiError> {
    let request = read(&params)?;
    let variables = Variables::from_json(json!({
        "input": {
            "itemId": GlobalId::encode(ITEM_TYPE, request.key),
            "rating": request.rating,
            "ratedAt": request.rated_at.map(date_time::format),
        }
    }));
    let data: RateData = gql::execute(&state.schema, RATE, variables).await?;
    if let Some(error) = data.rate_item.user_errors.first() {
        return Err(match error.code.as_str() {
            "ITEM_NOT_FOUND" => ApiError::NotFound,
            _ => ApiError::BadRequest,
        });
    }
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/html")]))
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn params(query: &str) -> RateParams {
        let mut params = RateParams {
            identifier: None,
            key: None,
            rating: None,
            rated_at: None,
        };
        for (name, value) in form_urlencoded::parse(query.as_bytes()) {
            let slot = match name.as_ref() {
                "identifier" => &mut params.identifier,
                "key" => &mut params.key,
                "rating" => &mut params.rating,
                "ratedAt" => &mut params.rated_at,
                _ => continue,
            };
            *slot = Some(value.into_owned());
        }
        params
    }

    fn status(query: &str) -> Option<StatusCode> {
        read(&params(query)).err().map(|error| match error {
            ApiError::BadRequest => StatusCode::BAD_REQUEST,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })
    }

    const LIBRARY: &str = "identifier=com.plexapp.plugins.library";

    #[test]
    fn reads_a_rating_and_a_clearing() {
        assert_eq!(
            read(&params(&format!("{LIBRARY}&key=577986&rating=7.26"))).unwrap(),
            RateRequest {
                key: 577_986,
                rating: Some(7.26),
                rated_at: None
            }
        );
        for clear in ["-1", "-1.0"] {
            let query = format!("{LIBRARY}&key=577986&rating={clear}&ratedAt=1794500000");
            assert_eq!(
                read(&params(&query)).unwrap(),
                RateRequest {
                    key: 577_986,
                    rating: None,
                    rated_at: Some(1_794_500_000)
                }
            );
        }
    }

    #[test]
    fn checks_the_parameters_before_the_item_as_the_reference_does() {
        let bad = Some(StatusCode::BAD_REQUEST);
        for query in [
            "key=577986&rating=4".to_owned(),
            format!("{LIBRARY}&rating=4"),
            format!("{LIBRARY}&key=577986"),
            format!("{LIBRARY}&key=577986&rating="),
            format!("{LIBRARY}&key=577986&rating=abc"),
            format!("{LIBRARY}&key=577986&rating=3,5"),
            format!("{LIBRARY}&key=577986&rating=4&ratedAt=abc"),
            format!("{LIBRARY}&key=577986&rating=4&ratedAt=0"),
            format!("{LIBRARY}&key=577986&rating=4&ratedAt=1600000001.5"),
            // An unknown key and a foreign provider wait for the parameters.
            format!("{LIBRARY}&key=99999999&rating=abc"),
            "identifier=x&key=577986".to_owned(),
        ] {
            assert_eq!(status(&query), bad, "{query}");
        }
        let missing = Some(StatusCode::NOT_FOUND);
        for query in [
            "identifier=x&key=577986&rating=4".to_owned(),
            "identifier=&key=577986&rating=4".to_owned(),
            "identifier=tv.plex.provider.metadata&key=577986&rating=4".to_owned(),
            format!("{LIBRARY}&key=abc&rating=4"),
            format!("{LIBRARY}&key=577986,577990&rating=4"),
        ] {
            assert_eq!(status(&query), missing, "{query}");
        }
    }
}
