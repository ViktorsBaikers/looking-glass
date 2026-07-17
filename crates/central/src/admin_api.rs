//! Admin CRUD for the location catalogue and global settings (FR-010..017,
//! AC23/24/25). Every admin route is gated by the fail-closed [`AdminSession`]
//! extractor — its successful extraction *is* the authorization — and every
//! mutating handler validates its input at the boundary *before* any store write,
//! so a rejected request performs no partial write (AC24). The one public route
//! (`GET /api/locations`) exposes the catalogue the visitor-facing selector reads
//! (Slice 6); it carries no admin-only data (FR-045).

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Component, Path as FsPath};

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use garde::Validate;
use serde::{Deserialize, Serialize};

use crate::auth::{random_id, AdminSession, ApiError};
use crate::observability::{correlation_id, log_validation_rejected};
use crate::store::{
    derive_location_status, latest_last_seen, unix_now, Agent, Family, GlobalSettings,
    IperfEndpoint, Location, LocationStatus, NodeKind, OfferedMethod, TestFile, TestIp,
};
use crate::AppState;

/// The admin routes, each behind [`AdminSession`]. Mounted inside the
/// setup-gated, session-layered admin router in `lib.rs`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/admin/locations",
            get(list_locations).post(create_location),
        )
        .route(
            "/api/admin/locations/{id}",
            get(get_location)
                .put(update_location)
                .delete(delete_location),
        )
        .route("/api/admin/locations/{id}/agent/revoke", post(revoke_agent))
        .route("/api/admin/locations/{id}/test-ips", post(create_test_ip))
        .route(
            "/api/admin/test-ips/{id}",
            put(update_test_ip).delete(delete_test_ip),
        )
        .route("/api/admin/locations/{id}/iperf", post(create_iperf))
        .route(
            "/api/admin/iperf/{id}",
            put(update_iperf).delete(delete_iperf),
        )
        .route("/api/admin/locations/{id}/files", post(create_test_file))
        .route(
            "/api/admin/files/{id}",
            put(update_test_file).delete(delete_test_file),
        )
        .route(
            "/api/admin/settings",
            get(get_settings).put(update_settings),
        )
}

/// The public catalogue route (unauthenticated): the online/offline locations and
/// their public entities the visitor selector consumes. Not setup-gated — before
/// setup there are simply no locations.
pub fn public_routes(state: AppState) -> Router {
    Router::new()
        .route("/api/locations", get(public_locations))
        .route("/api/public/settings", get(public_settings))
        .with_state(state)
}

#[derive(Serialize)]
struct PublicSettings {
    site_title: String,
    logo_url: Option<String>,
    default_theme: crate::store::Theme,
    terms_url: Option<String>,
    custom_block: Option<String>,
}

impl PublicSettings {
    fn from_settings(settings: GlobalSettings) -> Result<Self, ApiError> {
        if !(1..=100).contains(&settings.site_title.chars().count())
            || !is_optional_https_url(&settings.logo_url, 500)
            || !is_optional_https_url(&settings.terms_url, 300)
            || !settings
                .custom_block
                .as_ref()
                .is_none_or(|text| text.chars().count() <= 5000)
        {
            return Err(ApiError::Internal);
        }

        Ok(Self {
            site_title: settings.site_title,
            logo_url: settings.logo_url,
            default_theme: settings.default_theme,
            terms_url: settings.terms_url,
            custom_block: settings.custom_block,
        })
    }
}

fn is_optional_https_url(value: &Option<String>, max_length: usize) -> bool {
    value
        .as_ref()
        .is_none_or(|url| url.chars().count() <= max_length && is_https_url(url))
}

fn is_https_url(value: &str) -> bool {
    value
        .parse::<Uri>()
        .ok()
        .is_some_and(|uri| uri.scheme_str() == Some("https") && uri.authority().is_some())
}

/// A location plus its child entities — the shape both the admin editor and the
/// public selector read. No entity here carries a secret, so one shape serves both.
#[derive(Serialize)]
struct LocationDetail {
    #[serde(flatten)]
    location: Location,
    test_ips: Vec<TestIp>,
    iperf: Vec<IperfEndpoint>,
    files: Vec<TestFile>,
}

impl LocationDetail {
    fn load(state: &AppState, mut location: Location) -> Result<Self, ApiError> {
        scrub_local_data_plane_origin(&mut location);
        let id = location.id.clone();
        let files = match (location.kind, location.data_plane_origin.as_ref()) {
            (NodeKind::Remote, None) => Vec::new(),
            _ => state.store.list_test_files(&id)?,
        };
        Ok(Self {
            location,
            test_ips: state.store.list_test_ips(&id)?,
            iperf: state.store.list_iperf(&id)?,
            files,
        })
    }
}

fn scrub_local_data_plane_origin(location: &mut Location) {
    if location.kind == NodeKind::Local {
        location.data_plane_origin = None;
    }
}

fn clean_data_plane_origin(
    kind: NodeKind,
    origin: Option<String>,
) -> Result<Option<String>, ApiError> {
    if kind == NodeKind::Local {
        return Ok(None);
    }

    let Some(origin) = origin else {
        return Ok(None);
    };
    let trimmed = origin.trim();
    if trimmed.is_empty() {
        return Err(ApiError::Validation(
            "Enter an absolute http:// or https:// data-plane origin.".to_string(),
        ));
    }

    let uri: Uri = trimmed.parse().map_err(|_| {
        ApiError::Validation("Enter an absolute http:// or https:// data-plane origin.".to_string())
    })?;
    let scheme = uri.scheme_str().ok_or_else(|| {
        ApiError::Validation("Enter an absolute http:// or https:// data-plane origin.".to_string())
    })?;
    if scheme != "http" && scheme != "https" {
        return Err(ApiError::Validation(
            "Enter an absolute http:// or https:// data-plane origin.".to_string(),
        ));
    }
    let authority = uri.authority().ok_or_else(|| {
        ApiError::Validation("Enter an absolute http:// or https:// data-plane origin.".to_string())
    })?;
    let Some((_scheme, authority_tail)) = trimmed.split_once("://") else {
        return Err(ApiError::Validation(
            "Enter an absolute http:// or https:// data-plane origin.".to_string(),
        ));
    };
    if authority.as_str().contains('@')
        || authority_tail.contains('/')
        || authority_tail.contains('?')
        || authority_tail.contains('#')
    {
        return Err(ApiError::Validation(
            "Enter an origin without path, query, fragment, or userinfo.".to_string(),
        ));
    }
    Ok(Some(format!("{scheme}://{authority}")))
}

// ----- Locations -------------------------------------------------------------

#[derive(Deserialize, Validate)]
struct LocationInput {
    #[garde(length(min = 1, max = 100))]
    name: String,
    #[garde(length(max = 100))]
    geo_label: String,
    #[garde(inner(length(max = 200)))]
    map_query: Option<String>,
    #[garde(inner(length(max = 100)))]
    facility: Option<String>,
    #[garde(inner(length(max = 300)))]
    facility_url: Option<String>,
    #[garde(skip)]
    kind: NodeKind,
    #[garde(inner(length(max = 300)))]
    data_plane_origin: Option<String>,
    #[garde(length(max = 8))]
    offered_methods: Vec<OfferedMethod>,
}

impl LocationInput {
    /// Build the stored [`Location`] from validated input. A local node is online
    /// by definition; a remote node keeps `status` (offline for a create, its
    /// prior value for an edit) until its agent enrolls (Slice 8b).
    fn into_location(
        self,
        id: String,
        created_at: u64,
        status: LocationStatus,
    ) -> Result<Location, ApiError> {
        let data_plane_origin = clean_data_plane_origin(self.kind, self.data_plane_origin)?;
        Ok(Location {
            id,
            name: self.name,
            geo_label: self.geo_label,
            map_query: self.map_query,
            facility: self.facility,
            facility_url: self.facility_url,
            data_plane_origin,
            status: match self.kind {
                NodeKind::Local => LocationStatus::Online,
                NodeKind::Remote => status,
            },
            kind: self.kind,
            offered_methods: self.offered_methods,
            created_at,
        })
    }
}

/// A location as the admin list sees it: its live-derived `status` (compute-on-read,
/// Slice 8b) plus the most recent agent heartbeat for the last-seen column. The
/// persisted `status` is overwritten with the derived value before serialization.
#[derive(Serialize)]
struct AdminLocation {
    #[serde(flatten)]
    location: Location,
    last_seen: Option<u64>,
}

/// Group agents by their `location_id` so each location's live status derives from a
/// single agent scan, not an N+1 per-location query.
fn agents_by_location(agents: Vec<Agent>) -> HashMap<String, Vec<Agent>> {
    let mut map: HashMap<String, Vec<Agent>> = HashMap::new();
    for agent in agents {
        map.entry(agent.location_id.clone())
            .or_default()
            .push(agent);
    }
    map
}

async fn list_locations(
    State(state): State<AppState>,
    _admin: AdminSession,
) -> Result<Json<Vec<AdminLocation>>, ApiError> {
    let now = unix_now();
    let by_location = agents_by_location(state.store.all_agents()?);
    let out = state
        .store
        .list_locations()?
        .into_iter()
        .map(|mut location| {
            scrub_local_data_plane_origin(&mut location);
            let agents = by_location
                .get(&location.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let last_seen = latest_last_seen(&location, agents);
            location.status = derive_location_status(&location, agents, now);
            AdminLocation {
                location,
                last_seen,
            }
        })
        .collect();
    Ok(Json(out))
}

async fn get_location(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
) -> Result<Json<LocationDetail>, ApiError> {
    let mut location = state.store.get_location(&id)?.ok_or(ApiError::NotFound)?;
    scrub_local_data_plane_origin(&mut location);
    // Reflect the live-derived status so a poll (e.g. the enroll dialog awaiting
    // dial-home) sees the location come online (Slice 8b).
    let agents = state.store.list_agents(&id)?;
    location.status = derive_location_status(&location, &agents, unix_now());
    Ok(Json(LocationDetail::load(&state, location)?))
}

async fn create_location(
    State(state): State<AppState>,
    _admin: AdminSession,
    headers: HeaderMap,
    Json(body): Json<LocationInput>,
) -> Result<(StatusCode, Json<Location>), ApiError> {
    validate_admin(&headers, "admin.location", &body)?;
    let location = body
        .into_location(random_id(), unix_now(), LocationStatus::Offline)
        .map_err(|error| {
            admin_validation_error(&headers, "admin.location", "invalid_location", error)
        })?;
    state.store.put_location(&location)?;
    Ok((StatusCode::CREATED, Json(location)))
}

async fn update_location(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<LocationInput>,
) -> Result<Json<Location>, ApiError> {
    let existing = state.store.get_location(&id)?.ok_or(ApiError::NotFound)?;
    validate_admin(&headers, "admin.location", &body)?;
    let location = body
        .into_location(existing.id, existing.created_at, existing.status)
        .map_err(|error| {
            admin_validation_error(&headers, "admin.location", "invalid_location", error)
        })?;
    state.store.put_location(&location)?;
    Ok(Json(location))
}

async fn delete_location(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let deleted = state.store.delete_location_with_agents(&id)?;
    if deleted.existed {
        state.tunnel_hub.kick_agents(&deleted.agent_ids);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

async fn revoke_agent(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<AdminLocation>, ApiError> {
    let mut location = state.store.get_location(&id)?.ok_or(ApiError::NotFound)?;
    if location.kind != NodeKind::Remote {
        log_validation_rejected(
            &correlation_id(&headers),
            "admin.agent",
            "local_location_revoke",
        );
        return Err(ApiError::Validation(
            "Only remote locations have an agent to revoke.".to_string(),
        ));
    }
    let revoked = state.store.revoke_agents_for_location(&id)?;
    state.tunnel_hub.kick_agents(&revoked);
    let agents = state.store.list_agents(&id)?;
    location.status = derive_location_status(&location, &agents, unix_now());
    Ok(Json(AdminLocation {
        last_seen: latest_last_seen(&location, &agents),
        location,
    }))
}

// ----- Test IPs --------------------------------------------------------------

#[derive(Deserialize, Validate)]
struct TestIpInput {
    #[garde(skip)]
    family: Family,
    #[garde(length(min = 1, max = 45))]
    address: String,
    #[garde(inner(length(max = 100)))]
    label: Option<String>,
}

impl TestIpInput {
    /// Reject an address that is not a valid IP of the declared family — a display
    /// IP visitors copy must be a real address, and the family label must match so
    /// the public UI groups it correctly (validation before any write, AC24).
    fn check_address(&self) -> Result<(), ApiError> {
        let parsed: IpAddr = self
            .address
            .parse()
            .map_err(|_| ApiError::Validation("Enter a valid IP address.".to_string()))?;
        let matches = matches!(
            (self.family, parsed),
            (Family::V4, IpAddr::V4(_)) | (Family::V6, IpAddr::V6(_))
        );
        if matches {
            Ok(())
        } else {
            Err(ApiError::Validation(
                "The address does not match the selected family.".to_string(),
            ))
        }
    }

    fn into_test_ip(self, id: String, location_id: String) -> TestIp {
        TestIp {
            id,
            location_id,
            family: self.family,
            address: self.address,
            label: self.label,
        }
    }
}

async fn create_test_ip(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(location_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<TestIpInput>,
) -> Result<(StatusCode, Json<TestIp>), ApiError> {
    require_location(&state, &location_id)?;
    validate_admin(&headers, "admin.test_ip", &body)?;
    body.check_address().map_err(|error| {
        admin_validation_error(&headers, "admin.test_ip", "invalid_address", error)
    })?;
    let test_ip = body.into_test_ip(random_id(), location_id);
    state.store.put_test_ip(&test_ip)?;
    Ok((StatusCode::CREATED, Json(test_ip)))
}

async fn update_test_ip(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<TestIpInput>,
) -> Result<Json<TestIp>, ApiError> {
    let existing = state.store.get_test_ip(&id)?.ok_or(ApiError::NotFound)?;
    validate_admin(&headers, "admin.test_ip", &body)?;
    body.check_address().map_err(|error| {
        admin_validation_error(&headers, "admin.test_ip", "invalid_address", error)
    })?;
    let test_ip = body.into_test_ip(existing.id, existing.location_id);
    state.store.put_test_ip(&test_ip)?;
    Ok(Json(test_ip))
}

async fn delete_test_ip(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    ok_or_not_found(state.store.delete_test_ip(&id)?)
}

// ----- iperf endpoints -------------------------------------------------------

#[derive(Deserialize, Validate)]
struct IperfInput {
    #[garde(length(min = 1, max = 100))]
    label: String,
    #[garde(length(min = 1, max = 253))]
    host: String,
    #[garde(range(min = 1))]
    port: u16,
    #[garde(length(min = 1, max = 300))]
    cmd_incoming: String,
    #[garde(length(min = 1, max = 300))]
    cmd_outgoing: String,
}

impl IperfInput {
    fn into_endpoint(self, id: String, location_id: String) -> IperfEndpoint {
        IperfEndpoint {
            id,
            location_id,
            label: self.label,
            host: self.host,
            port: self.port,
            cmd_incoming: self.cmd_incoming,
            cmd_outgoing: self.cmd_outgoing,
        }
    }
}

async fn create_iperf(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(location_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<IperfInput>,
) -> Result<(StatusCode, Json<IperfEndpoint>), ApiError> {
    require_location(&state, &location_id)?;
    validate_admin(&headers, "admin.iperf", &body)?;
    let endpoint = body.into_endpoint(random_id(), location_id);
    state.store.put_iperf(&endpoint)?;
    Ok((StatusCode::CREATED, Json(endpoint)))
}

async fn update_iperf(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<IperfInput>,
) -> Result<Json<IperfEndpoint>, ApiError> {
    let existing = state.store.get_iperf(&id)?.ok_or(ApiError::NotFound)?;
    validate_admin(&headers, "admin.iperf", &body)?;
    let endpoint = body.into_endpoint(existing.id, existing.location_id);
    state.store.put_iperf(&endpoint)?;
    Ok(Json(endpoint))
}

async fn delete_iperf(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    ok_or_not_found(state.store.delete_iperf(&id)?)
}

// ----- Test files ------------------------------------------------------------

#[derive(Deserialize, Validate)]
struct TestFileInput {
    #[garde(length(min = 1, max = 100))]
    label: String,
    #[garde(length(min = 1, max = 50))]
    declared_size: String,
    #[garde(length(min = 1, max = 300))]
    source_ref: String,
}

impl TestFileInput {
    fn check_remote_source_ref(&self, location: &Location) -> Result<(), ApiError> {
        if location.kind != NodeKind::Remote {
            return Ok(());
        }
        let path = FsPath::new(&self.source_ref);
        if path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        {
            Ok(())
        } else {
            Err(ApiError::Validation(
                "Remote file source must be a relative path without dot segments.".to_string(),
            ))
        }
    }

    fn into_file(self, id: String, location_id: String) -> TestFile {
        TestFile {
            id,
            location_id,
            label: self.label,
            declared_size: self.declared_size,
            source_ref: self.source_ref,
        }
    }
}

async fn create_test_file(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(location_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<TestFileInput>,
) -> Result<(StatusCode, Json<TestFile>), ApiError> {
    let location = state
        .store
        .get_location(&location_id)?
        .ok_or(ApiError::NotFound)?;
    validate_admin(&headers, "admin.file", &body)?;
    body.check_remote_source_ref(&location).map_err(|error| {
        admin_validation_error(&headers, "admin.file", "invalid_remote_source_ref", error)
    })?;
    let file = body.into_file(random_id(), location_id);
    state.store.put_test_file(&file)?;
    Ok((StatusCode::CREATED, Json(file)))
}

async fn update_test_file(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<TestFileInput>,
) -> Result<Json<TestFile>, ApiError> {
    let existing = state.store.get_test_file(&id)?.ok_or(ApiError::NotFound)?;
    let location = state
        .store
        .get_location(&existing.location_id)?
        .ok_or(ApiError::NotFound)?;
    validate_admin(&headers, "admin.file", &body)?;
    body.check_remote_source_ref(&location).map_err(|error| {
        admin_validation_error(&headers, "admin.file", "invalid_remote_source_ref", error)
    })?;
    let file = body.into_file(existing.id, existing.location_id);
    state.store.put_test_file(&file)?;
    Ok(Json(file))
}

async fn delete_test_file(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    ok_or_not_found(state.store.delete_test_file(&id)?)
}

// ----- Settings --------------------------------------------------------------

#[derive(Deserialize, Validate)]
struct SettingsInput {
    #[garde(length(min = 1, max = 100))]
    site_title: String,
    #[garde(inner(length(max = 500)))]
    logo_url: Option<String>,
    #[garde(skip)]
    default_theme: crate::store::Theme,
    #[garde(inner(length(max = 300)))]
    terms_url: Option<String>,
    #[garde(inner(length(max = 5000)))]
    custom_block: Option<String>,
    #[garde(range(min = 1, max = 1024))]
    exec_max_concurrent: usize,
    #[garde(range(min = 1, max = 3600))]
    exec_timeout_secs: u64,
    #[garde(range(min = 1, max = 1_048_576))]
    exec_max_output_kib: usize,
    #[garde(range(min = 1, max = 100_000))]
    exec_rate_max: u32,
    #[garde(range(min = 1, max = 86_400))]
    exec_rate_window_secs: u64,
}

impl SettingsInput {
    fn into_settings(self) -> Result<GlobalSettings, ApiError> {
        if !is_optional_https_url(&self.logo_url, 500)
            || !is_optional_https_url(&self.terms_url, 300)
        {
            return Err(ApiError::Validation(
                "Logo and terms URLs must use https.".to_string(),
            ));
        }

        Ok(GlobalSettings {
            site_title: self.site_title,
            logo_url: self.logo_url,
            default_theme: self.default_theme,
            terms_url: self.terms_url,
            custom_block: self.custom_block,
            exec_max_concurrent: self.exec_max_concurrent,
            exec_timeout_secs: self.exec_timeout_secs,
            exec_max_output_kib: self.exec_max_output_kib,
            exec_rate_max: self.exec_rate_max,
            exec_rate_window_secs: self.exec_rate_window_secs,
        })
    }
}

async fn get_settings(
    State(state): State<AppState>,
    _admin: AdminSession,
) -> Result<Json<GlobalSettings>, ApiError> {
    Ok(Json(state.store.settings()?))
}

async fn update_settings(
    State(state): State<AppState>,
    _admin: AdminSession,
    headers: HeaderMap,
    Json(body): Json<SettingsInput>,
) -> Result<Json<GlobalSettings>, ApiError> {
    validate_admin(&headers, "admin.settings", &body)?;
    let settings = body.into_settings().map_err(|error| {
        admin_validation_error(&headers, "admin.settings", "invalid_branding_url", error)
    })?;
    state.run.save_settings(&state.store, &settings)?;
    Ok(Json(settings))
}

// ----- Public read -----------------------------------------------------------

async fn public_settings(State(state): State<AppState>) -> axum::response::Response {
    let mut response = match state.store.settings().and_then(|settings| {
        PublicSettings::from_settings(settings)
            .map_err(|_| crate::store::StoreError::Backend("invalid public settings".to_string()))
    }) {
        Ok(settings) => ([(header::CACHE_CONTROL, "no-store")], Json(settings)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn public_locations(
    State(state): State<AppState>,
) -> Result<Json<Vec<LocationDetail>>, ApiError> {
    // Only live locations are public (FR-026/AC17): a local node is always online; a
    // remote is online only while its agent is heartbeating (compute-on-read, Slice
    // 8b) and offline once the window lapses or it is revoked. An offline location and
    // its details (test IPs, iperf hosts, facility) never leak into the selector.
    let now = unix_now();
    let by_location = agents_by_location(state.store.all_agents()?);
    let mut out = Vec::new();
    for mut location in state.store.list_locations()? {
        scrub_local_data_plane_origin(&mut location);
        let agents = by_location
            .get(&location.id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        location.status = derive_location_status(&location, agents, now);
        if location.status == LocationStatus::Online {
            out.push(LocationDetail::load(&state, location)?);
        }
    }
    Ok(Json(out))
}

// ----- Shared helpers --------------------------------------------------------

fn validate_admin<T: Validate<Context = ()>>(
    headers: &HeaderMap,
    surface: &str,
    body: &T,
) -> Result<(), ApiError> {
    body.validate().map_err(|report| {
        log_validation_rejected(&correlation_id(headers), surface, "invalid_payload");
        ApiError::Validation(first_message(&report))
    })
}

fn admin_validation_error(
    headers: &HeaderMap,
    surface: &str,
    reason: &str,
    error: ApiError,
) -> ApiError {
    if matches!(error, ApiError::Validation(_)) {
        log_validation_rejected(&correlation_id(headers), surface, reason);
    }
    error
}

fn first_message(report: &garde::Report) -> String {
    report
        .iter()
        .next()
        .map(|(path, error)| format!("{path}: {error}"))
        .unwrap_or_else(|| "Invalid input.".to_string())
}

/// A child create must have a parent — refuse a child for a location that does not
/// exist rather than writing an orphan.
fn require_location(state: &AppState, location_id: &str) -> Result<(), ApiError> {
    state
        .store
        .get_location(location_id)?
        .map(|_| ())
        .ok_or(ApiError::NotFound)
}

fn ok_or_not_found(existed: bool) -> Result<StatusCode, ApiError> {
    if existed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}
