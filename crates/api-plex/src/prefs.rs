//! `GET /:/prefs`: the server's settings, as Plex clients read them.
//!
//! The reference publishes 162 settings — its transcoder, DLNA, maintenance tasks, remote
//! access — and most name machinery this server does not have. Publishing them would invent
//! knobs that change nothing. These are the ten Plex Web reads, found by searching its
//! bundle for every one of the 162 identifiers, with the reference's definitions and in its
//! order. A client reading another one finds it absent, which is what a setting we do not
//! have is.
//!
//! Changing a setting is not implemented yet: `PUT` answers `501`.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, State};
use axum::routing::get;
use axum::Router;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use serde::{Deserialize, Serialize};

use crate::auth::{AdminCaller, Caller};
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::general::{self, GqlServer};
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
        .route("/:/prefs", get(prefs).put(change_prefs))
        .route(
            "/library/sections/{sectionId}/prefs",
            get(section_prefs).put(change_section_prefs),
        )
}

/// Response to `GET /:/prefs`.
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
    pub default: SettingValue,
    pub value: SettingValue,
    pub hidden: bool,
    pub advanced: bool,
    pub group: &'static str,
    /// The choices of an enumerated setting, as `value:Label` pairs.
    #[serde(rename = "enumValues", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<String>,
}

/// A setting's value, typed as the reference types it: a JSON boolean, number or string.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum SettingValue {
    Bool(bool),
    Int(i64),
    Text(String),
}

/// A setting as the reference defines it, value aside.
struct Definition {
    id: &'static str,
    label: &'static str,
    summary: &'static str,
    kind: &'static str,
    default: Fixed,
    hidden: bool,
    advanced: bool,
    group: &'static str,
    enum_values: Option<&'static str>,
}

/// A default, as a constant can hold it.
#[derive(Clone, Copy)]
enum Fixed {
    Bool(bool),
    Int(i64),
    Text(&'static str),
}

impl From<Fixed> for SettingValue {
    fn from(fixed: Fixed) -> Self {
        match fixed {
            Fixed::Bool(value) => Self::Bool(value),
            Fixed::Int(value) => Self::Int(value),
            Fixed::Text(value) => Self::Text(value.to_owned()),
        }
    }
}

const fn hidden(id: &'static str, kind: &'static str, default: Fixed) -> Definition {
    Definition {
        id,
        label: "",
        summary: "",
        kind,
        default,
        hidden: true,
        advanced: false,
        group: "",
        enum_values: None,
    }
}

/// The ten, in the reference's order, with its labels and defaults.
const DEFINITIONS: [Definition; 10] = [
    Definition {
        id: "FriendlyName",
        label: "Friendly name",
        summary: "This name will be used to identify this media server to other computers on \
                  your network. If you leave it blank, your computer's name will be used \
                  instead.",
        kind: "text",
        default: Fixed::Text(""),
        hidden: false,
        advanced: false,
        group: "general",
        enum_values: None,
    },
    Definition {
        id: "allowMediaDeletion",
        label: "Allow media deletion",
        summary: "The owner of the server will be allowed to delete media files from disk.",
        kind: "bool",
        default: Fixed::Bool(true),
        hidden: false,
        advanced: true,
        group: "library",
        enum_values: None,
    },
    Definition {
        id: "iTunesLibraryXmlPath",
        label: "iTunes library XML path",
        summary: "",
        kind: "text",
        default: Fixed::Text(""),
        hidden: false,
        advanced: true,
        group: "library",
        enum_values: None,
    },
    Definition {
        label: "Has the user accepted the EULA",
        ..hidden("AcceptedEULA", "bool", Fixed::Bool(false))
    },
    Definition {
        label: "Publish server on Plex Online",
        summary: "Publishing a server makes it automatically available on your client devices \
                  without any configuration of your router.",
        ..hidden("PublishServerOnPlexOnlineKey", "bool", Fixed::Bool(false))
    },
    hidden("ManualPortMappingMode", "bool", Fixed::Bool(false)),
    hidden("ManualPortMappingPort", "int", Fixed::Int(32_400)),
    hidden("BackgroundQueueIdlePaused", "bool", Fixed::Bool(false)),
    Definition {
        id: "ButlerUpdateChannel",
        label: "Server update Channel",
        summary: "",
        kind: "text",
        default: Fixed::Text("16"),
        hidden: false,
        advanced: false,
        group: "general",
        enum_values: Some("0:Public"),
    },
    hidden("MergedRecentlyAdded", "bool", Fixed::Bool(false)),
];

/// `GET /:/prefs`.
async fn prefs(
    _caller: Caller,
    State(state): State<PlexState>,
    format: ResponseFormat,
) -> Result<PlexResponse<PrefsContainer>, ApiError> {
    let server = general::server(&state).await?;
    let setting: Vec<Setting> = DEFINITIONS
        .iter()
        .map(|definition| Setting {
            id: definition.id,
            label: definition.label,
            summary: definition.summary,
            kind: definition.kind,
            default: definition.default.into(),
            value: value_of(definition, &server),
            hidden: definition.hidden,
            advanced: definition.advanced,
            group: definition.group,
            enum_values: definition.enum_values.map(str::to_owned),
        })
        .collect();
    Ok(PlexResponse::new(
        PrefsContainer {
            size: u32::try_from(setting.len()).unwrap_or(u32::MAX),
            setting,
        },
        format,
    ))
}

/// `PUT /:/prefs`. Declared here rather than left to the stubs, which can only take a whole
/// path: without it the method would answer `405`, as if Plex had no such operation.
async fn change_prefs(_caller: Caller) -> ApiError {
    ApiError::NotImplemented
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

async fn change_section_prefs(_caller: AdminCaller) -> ApiError {
    ApiError::NotImplemented
}

/// Unlike the server's settings, every default and value here is a string on the wire, even
/// for a boolean.
fn section_settings(catalog: &'static Catalog) -> Vec<Setting> {
    let text = |value: &str| SettingValue::Text(value.to_owned());
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

/// What this server actually has for a setting; its default where nothing here differs.
fn value_of(definition: &Definition, server: &GqlServer) -> SettingValue {
    match definition.id {
        "FriendlyName" => SettingValue::Text(server.friendly_name.clone()),
        "allowMediaDeletion" => SettingValue::Bool(server.features.media_deletion),
        // There is no Plex licence to agree to here, and a client reading `false` stops on
        // an agreement screen before anything else.
        "AcceptedEULA" => SettingValue::Bool(true),
        // `PublishServerOnPlexOnlineKey` keeps its `false`: the server is not linked to
        // plex.tv, and saying otherwise would send a client looking for it there.
        _ => definition.default.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_the_ten_settings_plex_web_reads_in_the_reference_order() {
        let ids: Vec<&str> = DEFINITIONS.iter().map(|definition| definition.id).collect();
        assert_eq!(
            ids,
            [
                "FriendlyName",
                "allowMediaDeletion",
                "iTunesLibraryXmlPath",
                "AcceptedEULA",
                "PublishServerOnPlexOnlineKey",
                "ManualPortMappingMode",
                "ManualPortMappingPort",
                "BackgroundQueueIdlePaused",
                "ButlerUpdateChannel",
                "MergedRecentlyAdded",
            ]
        );
    }
}
