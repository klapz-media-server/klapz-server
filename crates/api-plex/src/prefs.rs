//! Settings: the server's, which Plex clients are refused, and a library's.
//!
//! `/:/prefs` answers `403`. Plex Web draws its server settings screens — general, remote
//! access, agents, languages, optimized versions, Live TV, troubleshooting — only once that
//! list has loaded, and every one of them drives machinery this server does not have. Refused,
//! the screens are not offered at all; served, they open onto routes that fail. Nothing else
//! in Plex Web reads the list: the licence agreement it holds is only looked at once loaded.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, State};
use axum::routing::get;
use axum::Router;
use klapz_api::library::LIBRARY_TYPE;
use klapz_api::GlobalId;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::language::Language;
use crate::response::{PlexResponse, ResponseFormat};
use crate::translations::Catalog;
use crate::PlexState;

/// Adds the route onto the caller's router rather than returning one to merge: the path
/// opens with a literal colon, which only the router with Axum's legacy checks turned off
/// accepts, and `merge` would re-validate it under the default rules.
pub fn add_routes(router: Router<PlexState>) -> Router<PlexState> {
    router
        .route("/:/prefs", get(refuse_prefs).put(refuse_prefs))
        .route(
            "/library/sections/{sectionId}/prefs",
            get(section_prefs).put(change_section_prefs),
        )
}

/// Response to `GET /library/sections/{id}/prefs`.
#[derive(Debug, Serialize)]
pub struct PrefsContainer {
    pub size: u32,
    #[serde(rename = "Setting")]
    pub setting: Vec<Setting>,
}

/// One setting, in the reference's attribute order.
#[derive(Debug, Serialize)]
pub struct Setting {
    pub id: &'static str,
    pub label: &'static str,
    pub summary: &'static str,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub default: String,
    pub value: String,
    pub hidden: bool,
    pub advanced: bool,
    pub group: &'static str,
    /// The choices of an enumerated setting, as `value:Label` pairs.
    #[serde(rename = "enumValues", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<String>,
}

/// `GET` and `PUT /:/prefs`, to any caller, an owner of the server included.
async fn refuse_prefs(_caller: Caller) -> ApiError {
    ApiError::Forbidden
}

const LIBRARY_EXISTS_QUERY: &str = r"
query PlexLibraryExists($id: ID!) {
  node(id: $id) { ... on Library { id } }
}
";

#[derive(Debug, Deserialize)]
struct LibraryExistsData {
    node: Option<serde_json::Value>,
}

/// `GET /library/sections/{id}/prefs`: a library's settings.
///
/// The reference lists 17 for a film library and 18 for a series, nearly all about scanning
/// and matching, which are out of scope here. As with the server's own, only the two Plex
/// Web looks up by name are published, at their defaults: no library is hidden, and nothing
/// is matched. The rest of the dialog is drawn from whatever the list holds.
async fn section_prefs(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    format: ResponseFormat,
) -> Result<PlexResponse<PrefsContainer>, ApiError> {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    let data: LibraryExistsData =
        gql::execute(&state.schema, LIBRARY_EXISTS_QUERY, variables).await?;
    data.node.ok_or(ApiError::NotFound)?;

    let setting = section_settings(Language::of(&client).catalog());
    Ok(PlexResponse::new(
        PrefsContainer {
            size: u32::try_from(setting.len()).unwrap_or(u32::MAX),
            setting,
        },
        format,
    ))
}

/// Answered for a library's admins only; the others are told it does not exist.
async fn change_section_prefs(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
) -> ApiError {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    let role: Result<LibraryRoleData, ApiError> =
        gql::execute(&state.schema, LIBRARY_ROLE_QUERY, variables).await;
    match role.map(|data| data.node.and_then(|node| node.viewer_role)) {
        Ok(Some(role)) if role == "ADMIN" => ApiError::NotImplemented,
        Ok(_) => ApiError::NotFound,
        Err(error) => error,
    }
}

const LIBRARY_ROLE_QUERY: &str = r"
query PlexLibraryRole($id: ID!) { node(id: $id) { ... on Library { viewerRole } } }
";

#[derive(Debug, Deserialize)]
struct LibraryRoleData {
    node: Option<LibraryRoleNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryRoleNode {
    viewer_role: Option<String>,
}

/// Every default and value here is a string on the wire, even for a boolean.
fn section_settings(catalog: &'static Catalog) -> Vec<Setting> {
    let text = |value: &str| value.to_owned();
    vec![
        Setting {
            id: "hidden",
            label: catalog.visibility,
            summary: catalog.visibility_summary,
            kind: "int",
            default: text("0"),
            value: text("0"),
            hidden: false,
            advanced: false,
            group: "",
            enum_values: Some(format!(
                "0:{}|1:{}|2:{}",
                catalog.visibility_everywhere,
                catalog.visibility_no_home,
                catalog.visibility_nowhere
            )),
        },
        Setting {
            id: "includeAdultContent",
            label: catalog.adult_matching,
            summary: catalog.adult_matching_summary,
            kind: "bool",
            default: text("false"),
            value: text("false"),
            hidden: false,
            advanced: false,
            group: "",
            enum_values: None,
        },
    ]
}
