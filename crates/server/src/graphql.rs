//! The official GraphQL endpoint.
//!
//! Mounted at `/graphql`: `POST` executes operations, `GET` serves `GraphiQL` so the schema is
//! explorable without extra tooling. The Plex adapter does not come through here — it
//! executes against the same schema in-process — so this route exists for real clients.

use async_graphql::http::GraphiQLSource;
use async_graphql::Request;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::extract::{OriginalUri, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Router};
use std::sync::Arc;

use open_plex_api::{ApiSchema, Sessions};
use open_plex_core::Principal;

#[derive(Clone)]
struct GraphqlState {
    schema: ApiSchema,
    sessions: Arc<Sessions>,
}

pub fn router(schema: ApiSchema, sessions: Arc<Sessions>) -> Router {
    Router::new()
        .route("/graphql", post(execute))
        .route("/graphql", get(graphiql))
        .with_state(GraphqlState { schema, sessions })
}

/// Executes an operation on behalf of the authenticated caller.
///
/// The principal is attached by the token middleware and forwarded into the request data, so
/// resolvers can scope results to the caller without knowing how authentication happened. The
/// session keys go along, for the operations that sign in and out.
async fn execute(
    State(GraphqlState { schema, sessions }): State<GraphqlState>,
    principal: Option<Extension<Principal>>,
    headers: HeaderMap,
    request: GraphQLRequest,
) -> Response {
    if !is_json(&headers) {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let mut request: Request = request.into_inner().data(sessions);
    if let Some(Extension(principal)) = principal {
        request = request.data(principal);
    }
    GraphQLResponse::from(schema.execute(request).await).into_response()
}

/// A web page can send any site a form or `text/plain` without asking, but not JSON. The
/// session cookie is `SameSite=Lax`, which a page of the same site — another port, another
/// subdomain — still gets sent along, so without this such a page could act as whoever is
/// signed in; refusing the rest makes the browser ask first, and nothing here says yes.
fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
}

#[cfg(test)]
#[test]
fn only_json_reaches_the_schema() {
    let with = |value: &'static str| {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            value.parse().unwrap_or_else(|_| unreachable!()),
        );
        is_json(&headers)
    };
    assert!(with("application/json"));
    assert!(with("Application/JSON; charset=utf-8"));
    assert!(!with("text/plain"));
    assert!(!with("application/x-www-form-urlencoded"));
    assert!(!with("multipart/form-data; boundary=x"));
    assert!(!is_json(&HeaderMap::new()));
}

/// Serves the `GraphiQL` playground.
///
/// The token is carried through into the endpoint `GraphiQL` posts to. Without that, the page
/// loads but every query it sends is rejected: the browser fetches `/graphql` with no
/// credential, and the token middleware is right to refuse it.
async fn graphiql(OriginalUri(uri): OriginalUri) -> impl IntoResponse {
    let endpoint = match uri.query() {
        Some(query) if !query.is_empty() => format!("/graphql?{query}"),
        _ => "/graphql".to_owned(),
    };
    Html(GraphiQLSource::build().endpoint(&endpoint).finish())
}
