//! What a client reads to recognize a Jellyfin server, before and after signing in.

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Serialize;

use crate::error::JellyfinError;
use crate::response::json;
use crate::{JellyfinState, ServerInfo, BASE_PATH, JELLYFIN_VERSION};

pub fn router() -> Router<JellyfinState> {
    Router::new()
        .route("/system/info/public", get(public_info))
        .route("/system/info", get(info))
        .route("/system/ping", get(ping).post(ping))
        .route("/branding/configuration", get(branding))
        .route("/branding/css", get(css))
        .route("/branding/css.css", get(css))
        .route("/quickconnect/enabled", get(quick_connect))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct PublicSystemInfo {
    local_address: String,
    server_name: String,
    version: &'static str,
    product_name: &'static str,
    operating_system: &'static str,
    id: String,
    startup_wizard_completed: bool,
}

/// Where the request reached the server, below which clients find the surface.
fn local_address(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .unwrap_or("localhost");
    format!("http://{host}{BASE_PATH}")
}

fn public(server: &ServerInfo, headers: &HeaderMap) -> PublicSystemInfo {
    PublicSystemInfo {
        local_address: local_address(headers),
        server_name: server.name.clone(),
        version: JELLYFIN_VERSION,
        product_name: "Jellyfin Server",
        operating_system: "",
        id: server.id.clone(),
        startup_wizard_completed: true,
    }
}

async fn public_info(State(state): State<JellyfinState>, headers: HeaderMap) -> Response {
    json(&public(&state.server, &headers))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Jellyfin's wire shape, field for field"
)]
struct SystemInfo {
    operating_system_display_name: &'static str,
    has_pending_restart: bool,
    is_shutting_down: bool,
    supports_library_monitor: bool,
    web_socket_port_number: u16,
    completed_installations: [(); 0],
    can_self_restart: bool,
    can_launch_web_browser: bool,
    /// The reference's own directories. This server has none of them to show, and no client
    /// reads them but the administration pages, which a user here never reaches.
    program_data_path: &'static str,
    web_path: &'static str,
    items_by_name_path: &'static str,
    cache_path: &'static str,
    log_path: &'static str,
    internal_metadata_path: &'static str,
    transcoding_temp_path: &'static str,
    cast_receiver_applications: [(); 0],
    has_update_available: bool,
    encoder_location: &'static str,
    system_architecture: &'static str,
    #[serde(flatten)]
    public: PublicSystemInfo,
}

async fn info(
    State(state): State<JellyfinState>,
    headers: HeaderMap,
) -> Result<Response, JellyfinError> {
    crate::gql::viewer(&state.schema).await?;
    let port = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .and_then(|host| host.rsplit_once(':'))
        .and_then(|(_, port)| port.parse().ok())
        .unwrap_or(80);
    Ok(json(&SystemInfo {
        operating_system_display_name: "",
        has_pending_restart: false,
        is_shutting_down: false,
        supports_library_monitor: false,
        web_socket_port_number: port,
        completed_installations: [],
        can_self_restart: false,
        can_launch_web_browser: false,
        program_data_path: "",
        web_path: "",
        items_by_name_path: "",
        cache_path: "",
        log_path: "",
        internal_metadata_path: "",
        transcoding_temp_path: "",
        cast_receiver_applications: [],
        has_update_available: false,
        encoder_location: "Custom",
        system_architecture: system_architecture(),
        public: public(&state.server, &headers),
    }))
}

/// As .NET names the processor architecture.
fn system_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "X64",
        "aarch64" => "Arm64",
        "x86" => "X86",
        "arm" => "Arm",
        _ => "Unknown",
    }
}

async fn ping() -> Response {
    json(&"Jellyfin Server")
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Branding {
    splashscreen_enabled: bool,
}

async fn branding() -> Response {
    json(&Branding {
        splashscreen_enabled: false,
    })
}

async fn css() -> Response {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/css; charset=utf-8"),
        )],
        "",
    )
        .into_response()
}

/// Quick Connect signs a device in from another one already signed in. Not built here: `false`
/// is what makes clients offer only the password.
async fn quick_connect() -> Response {
    json(&false)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_local_address_is_the_host_asked_and_the_base() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("192.168.1.4:32500"));
        assert_eq!(local_address(&headers), "http://192.168.1.4:32500/jellyfin");
        assert_eq!(
            local_address(&HeaderMap::new()),
            "http://localhost/jellyfin"
        );
    }

    #[test]
    fn the_public_info_names_a_jellyfin_server_of_the_reference_version() {
        let server = ServerInfo {
            id: "5e1f0a0e5e1f0a0e5e1f0a0e5e1f0a0e".to_owned(),
            name: "open-plex".to_owned(),
        };
        let value = serde_json::to_value(public(&server, &HeaderMap::new())).unwrap();
        assert_eq!(value["ProductName"], "Jellyfin Server");
        assert_eq!(value["Version"], "12.4.0");
        assert_eq!(value["Id"], server.id);
        assert_eq!(value["StartupWizardCompleted"], true);
    }
}
