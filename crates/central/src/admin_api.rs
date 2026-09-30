//! Admin CRUD for the location catalogue and global settings (FR-010..017,
//! AC23/24/25). Every admin route is gated by the fail-closed [`AdminSession`]
//! extractor — its successful extraction *is* the authorization — and every
//! mutating handler validates its input at the boundary *before* any store write,
//! so a rejected request performs no partial write (AC24). The one public route
//! (`GET /api/locations`) exposes the catalogue the visitor-facing selector reads
//! (Slice 6); it carries no admin-only data (FR-045).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use axum::extract::{FromRequest, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use garde::Validate;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use shared::protocol::CertificateStatus;

use crate::auth::{random_id, AdminSession, ApiError};
use crate::observability::{correlation_id, log_validation_rejected};
use crate::store::{
    derive_location_status, latest_last_seen, unix_now, Agent, Family, GlobalSettings,
    IperfEndpoint, Location, LocationStatus, NodeKind, OfferedMethod, TestFile, TestIp,
    EXEC_MAX_CONCURRENT_MAX, EXEC_MAX_OUTPUT_KIB_MAX, EXEC_RATE_MAX_MAX, EXEC_RATE_WINDOW_SECS_MAX,
    EXEC_TIMEOUT_SECS_MAX,
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
        .route("/api/admin/locations/order", put(reorder_locations))
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
            || settings
                .custom_block
                .as_ref()
                .is_some_and(|text| text.chars().count() > 5000)
        {
            return Err(ApiError::Internal);
        }

        Ok(Self {
            site_title: settings.site_title,
            logo_url: stored_public_url(settings.logo_url, 500)?,
            default_theme: settings.default_theme,
            terms_url: stored_public_url(settings.terms_url, 300)?,
            custom_block: settings.custom_block,
        })
    }
}

/// A stored logo or terms URL outside the contract every release enforced (too
/// long, or not https with a host) is corruption; one saved before the strict
/// check that the strict check now refuses is served as absent.
fn stored_public_url(value: Option<String>, max_length: usize) -> Result<Option<String>, ApiError> {
    let Some(url) = value else {
        return Ok(None);
    };
    let legacy_ok = url.chars().count() <= max_length
        && url
            .parse::<Uri>()
            .is_ok_and(|uri| uri.scheme_str() == Some("https") && uri.authority().is_some());
    if !legacy_ok {
        return Err(ApiError::Internal);
    }
    Ok(is_https_url(&url).then_some(url))
}

fn is_optional_https_url(value: &Option<String>, max_length: usize) -> bool {
    value
        .as_ref()
        .is_none_or(|url| url.chars().count() <= max_length && is_https_url(url))
}

/// The one strict https check for the admin-set public links (logo, terms and
/// facility) and enroll's agent asset URLs: an https scheme, a host (a bracketed
/// one must be IPv6, one ending in a number a dotted-quad IPv4) and nothing
/// after the host but an optional `:` and decimal port, so the SPA's URL parser
/// accepts every URL this accepts. http's parser alone lets through
/// `[::1]x:443`, `x[::1]:443`, `:+443` and `256.1.1.1`.
pub(crate) fn is_https_url(value: &str) -> bool {
    let Ok(uri) = value.parse::<Uri>() else {
        return false;
    };
    let Some(authority) = uri.authority() else {
        return false;
    };
    let host = authority.host();
    let host_ok = match host.strip_prefix('[') {
        Some(rest) => rest
            .strip_suffix(']')
            .is_some_and(|ip| matches!(ip.parse::<IpAddr>(), Ok(IpAddr::V6(_)))),
        None => {
            // WHATWG parses a host whose last label is a number as IPv4.
            let last = host.strip_suffix('.').unwrap_or(host).rsplit('.').next();
            let last = last.unwrap_or_default();
            let ends_in_number = match last.strip_prefix("0x").or(last.strip_prefix("0X")) {
                Some(hex) => hex.bytes().all(|b| b.is_ascii_hexdigit()),
                None => !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()),
            };
            !host.is_empty()
                && !host.ends_with(']')
                && (!ends_in_number || host.parse::<Ipv4Addr>().is_ok())
        }
    };
    let host_port = authority.as_str().rsplit('@').next().unwrap_or_default();
    let port_ok = match host_port.strip_prefix(host) {
        Some("") => true,
        Some(rest) => rest.strip_prefix(':').is_some_and(|port| {
            port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok()
        }),
        None => false,
    };
    uri.scheme_str() == Some("https") && host_ok && port_ok
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

pub(crate) fn clean_data_plane_origin(
    kind: NodeKind,
    origin: Option<String>,
) -> Result<Option<String>, ApiError> {
    if kind == NodeKind::Local {
        return Ok(None);
    }

    let Some(origin) = origin else {
        return Ok(None);
    };
    // The agent serves the data plane over HTTPS on :443 and proves the origin to
    // its CA with TLS-ALPN-01, which is always dialed on 443.
    let https_443 =
        || ApiError::Validation("Enter an https:// data-plane origin on port 443.".to_string());
    let trimmed = origin.trim();
    if trimmed.is_empty() {
        return Err(https_443());
    }

    let uri: Uri = trimmed.parse().map_err(|_| https_443())?;
    let scheme = uri.scheme_str().ok_or_else(https_443)?;
    if scheme != "https" {
        return Err(https_443());
    }
    let authority = uri.authority().ok_or_else(https_443)?;
    if authority.port_u16().is_some_and(|port| port != 443) {
        return Err(https_443());
    }
    let Some((_scheme, authority_tail)) = trimmed.split_once("://") else {
        return Err(https_443());
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

/// The optional ASN (spec #1): a whole number 1–4294967295, or absent. Any
/// other JSON value — 0, negative, beyond u32, a float, a string — is the
/// spec's 400 `invalid_asn`, judged here rather than by the deserializer so
/// the caller gets the coded error body instead of an extractor rejection.
fn clean_asn(value: &Option<serde_json::Value>) -> Result<Option<u32>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value.as_u64() {
        Some(asn) if (1..=4_294_967_295).contains(&asn) => Ok(Some(asn as u32)),
        _ => Err(ApiError::Coded(
            StatusCode::BAD_REQUEST,
            "invalid_asn",
            "ASN must be a whole number between 1 and 4294967295.",
        )),
    }
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
    /// Raw JSON on purpose — see [`clean_asn`]: invalid shapes must answer 400
    /// `invalid_asn`, not a body-extractor rejection.
    #[garde(skip)]
    asn: Option<serde_json::Value>,
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
        if !is_optional_https_url(&self.facility_url, 300) {
            return Err(ApiError::Validation(
                "Facility link must be an https:// URL.".to_string(),
            ));
        }
        let data_plane_origin = clean_data_plane_origin(self.kind, self.data_plane_origin)?;
        let asn = clean_asn(&self.asn)?;
        Ok(Location {
            id,
            name: self.name,
            geo_label: self.geo_label,
            map_query: self.map_query,
            facility: self.facility,
            facility_url: self.facility_url,
            data_plane_origin,
            asn,
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

/// The admin editor's view of one location: the detail plus the list's
/// `last_seen`, so both label an enrolled-but-offline remote the same way, and
/// the data-plane certificate status its agent last reported.
#[derive(Serialize)]
struct AdminLocationDetail {
    #[serde(flatten)]
    detail: LocationDetail,
    last_seen: Option<u64>,
    certificate: Option<CertificateStatus>,
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
) -> Result<Json<AdminLocationDetail>, ApiError> {
    let mut location = state.store.get_location(&id)?.ok_or(ApiError::NotFound)?;
    scrub_local_data_plane_origin(&mut location);
    // Reflect the live-derived status so a poll (e.g. the enroll dialog awaiting
    // dial-home) sees the location come online (Slice 8b).
    let agents = state.store.list_agents(&id)?;
    location.status = derive_location_status(&location, &agents, unix_now());
    let mut certificate = state.store.get_certificate_status(&id)?;
    // An origin saved before the https-on-443 rule is withheld from the agent and
    // visitors (see `public_locations`, `tunnel`); say so where the editor looks.
    if clean_data_plane_origin(location.kind, location.data_plane_origin.clone()).is_err() {
        certificate = Some(CertificateStatus {
            last_error: Some(
                "The saved origin is not https:// on port 443, so the node and visitors \
                 do not get it. Re-save a valid origin."
                    .to_string(),
            ),
            ..CertificateStatus::default()
        });
    }
    Ok(Json(AdminLocationDetail {
        last_seen: latest_last_seen(&location, &agents),
        certificate,
        detail: LocationDetail::load(&state, location)?,
    }))
}

async fn create_location(
    State(state): State<AppState>,
    _admin: AdminSession,
    headers: HeaderMap,
    AdminJson(body): AdminJson<LocationInput>,
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
    AdminJson(body): AdminJson<LocationInput>,
) -> Result<Json<Location>, ApiError> {
    let existing = state.store.get_location(&id)?.ok_or(ApiError::NotFound)?;
    validate_admin(&headers, "admin.location", &body)?;
    let location = body
        .into_location(existing.id, existing.created_at, existing.status)
        .map_err(|error| {
            admin_validation_error(&headers, "admin.location", "invalid_location", error)
        })?;
    let revoked = state
        .store
        .update_location(&location)?
        .ok_or(ApiError::NotFound)?;
    state.tunnel_hub.kick_agents(&revoked);
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

#[derive(Deserialize)]
struct LocationOrderInput {
    ids: Vec<String>,
}

/// Replace the location order (the public tab order). The body must list every
/// location exactly once, so a client working from a stale list gets a 409
/// instead of silently dropping a location a peer just added.
async fn reorder_locations(
    State(state): State<AppState>,
    _admin: AdminSession,
    headers: HeaderMap,
    AdminJson(body): AdminJson<LocationOrderInput>,
) -> Result<StatusCode, ApiError> {
    if state.store.reorder_locations(&body.ids)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        log_validation_rejected(&correlation_id(&headers), "admin.location", "stale_order");
        Err(ApiError::Coded(
            StatusCode::CONFLICT,
            "stale_order",
            "The location list changed. Reload it and try again.",
        ))
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
    AdminJson(body): AdminJson<TestIpInput>,
) -> Result<(StatusCode, Json<TestIp>), ApiError> {
    require_location(&state, &location_id)?;
    validate_admin(&headers, "admin.test_ip", &body)?;
    body.check_address().map_err(|error| {
        admin_validation_error(&headers, "admin.test_ip", "invalid_address", error)
    })?;
    let test_ip = body.into_test_ip(random_id(), location_id);
    ok_or_not_found(state.store.put_test_ip(&test_ip)?)?;
    Ok((StatusCode::CREATED, Json(test_ip)))
}

async fn update_test_ip(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
    headers: HeaderMap,
    AdminJson(body): AdminJson<TestIpInput>,
) -> Result<Json<TestIp>, ApiError> {
    let existing = state.store.get_test_ip(&id)?.ok_or(ApiError::NotFound)?;
    validate_admin(&headers, "admin.test_ip", &body)?;
    body.check_address().map_err(|error| {
        admin_validation_error(&headers, "admin.test_ip", "invalid_address", error)
    })?;
    let test_ip = body.into_test_ip(existing.id, existing.location_id);
    ok_or_not_found(state.store.update_test_ip(&test_ip)?)?;
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
    AdminJson(body): AdminJson<IperfInput>,
) -> Result<(StatusCode, Json<IperfEndpoint>), ApiError> {
    require_location(&state, &location_id)?;
    validate_admin(&headers, "admin.iperf", &body)?;
    let endpoint = body.into_endpoint(random_id(), location_id);
    ok_or_not_found(state.store.put_iperf(&endpoint)?)?;
    Ok((StatusCode::CREATED, Json(endpoint)))
}

async fn update_iperf(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
    headers: HeaderMap,
    AdminJson(body): AdminJson<IperfInput>,
) -> Result<Json<IperfEndpoint>, ApiError> {
    let existing = state.store.get_iperf(&id)?.ok_or(ApiError::NotFound)?;
    validate_admin(&headers, "admin.iperf", &body)?;
    let endpoint = body.into_endpoint(existing.id, existing.location_id);
    ok_or_not_found(state.store.update_iperf(&endpoint)?)?;
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
        // Split on '/' as the SPA's download URL does: `Path::components` would
        // normalise away the empty and `.` segments the SPA refuses.
        if self
            .source_ref
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
        {
            Ok(())
        } else {
            Err(ApiError::Validation(
                "Remote file source must be a relative path without empty or dot segments."
                    .to_string(),
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
    AdminJson(body): AdminJson<TestFileInput>,
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
    ok_or_not_found(state.store.put_test_file(&file)?)?;
    Ok((StatusCode::CREATED, Json(file)))
}

async fn update_test_file(
    State(state): State<AppState>,
    _admin: AdminSession,
    Path(id): Path<String>,
    headers: HeaderMap,
    AdminJson(body): AdminJson<TestFileInput>,
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
    ok_or_not_found(state.store.update_test_file(&file)?)?;
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
    #[garde(range(min = 1, max = EXEC_MAX_CONCURRENT_MAX))]
    exec_max_concurrent: usize,
    #[garde(range(min = 1, max = EXEC_TIMEOUT_SECS_MAX))]
    exec_timeout_secs: u64,
    #[garde(range(min = 1, max = EXEC_MAX_OUTPUT_KIB_MAX))]
    exec_max_output_kib: usize,
    #[garde(range(min = 1, max = EXEC_RATE_MAX_MAX))]
    exec_rate_max: u32,
    #[garde(range(min = 1, max = EXEC_RATE_WINDOW_SECS_MAX))]
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
    AdminJson(body): AdminJson<SettingsInput>,
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
        // Also withholds a remote origin saved before the https-on-443 rule.
        location.data_plane_origin =
            clean_data_plane_origin(location.kind, location.data_plane_origin).unwrap_or_default();
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

/// `Json<T>` for admin bodies. A body the extractor refuses (malformed JSON, a
/// wrong type, an out-of-range number) keeps axum's status but answers the
/// `{error, message}` envelope the SPA reads, not axum's text/plain body.
pub(crate) struct AdminJson<T>(pub(crate) T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for AdminJson<T> {
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Response> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) => Err((
                rejection.status(),
                Json(serde_json::json!({
                    "error": "invalid_input",
                    "message": rejection.body_text(),
                })),
            )
                .into_response()),
        }
    }
}

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
    if matches!(error, ApiError::Validation(_) | ApiError::Coded(..)) {
        log_validation_rejected(&correlation_id(headers), surface, reason);
    }
    error
}

pub(crate) fn first_message(report: &garde::Report) -> String {
    report
        .iter()
        .next()
        .map(|(path, error)| format!("{path}: {error}"))
        .unwrap_or_else(|| "Invalid input.".to_string())
}

/// A child create must have a parent — refuse a child for a location that does not
/// exist before validating it. The store write re-checks inside its transaction.
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

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::store::tests::set_before_write;
    use crate::{EnrollConfig, LoginLimiter, RunService, Store, TransportConfig, TunnelHub};

    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "lg-admin-unit-{}-{}",
            std::process::id(),
            random_id()
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        AppState {
            store: Store::open(dir.join("db.redb")).expect("open test store"),
            transport: TransportConfig::new([IpAddr::from(Ipv4Addr::LOCALHOST)]),
            login_limiter: Arc::new(LoginLimiter::default()),
            setup_token: None,
            run: RunService::for_test(8, Duration::from_secs(30), 100),
            files_root: Arc::from(dir.as_path()),
            enroll: EnrollConfig::for_test("https://central.test:8443", b"identity".to_vec()),
            tunnel_hub: TunnelHub::new(),
        }
    }

    fn admin() -> AdminSession {
        AdminSession {
            admin_id: "alice".to_string(),
        }
    }

    fn body<T: DeserializeOwned>(value: &serde_json::Value) -> AdminJson<T> {
        AdminJson(serde_json::from_value(value.clone()).expect("valid body"))
    }

    // Settings validation reads the same EXEC_*_MAX constants as
    // the LG_EXEC_* clamps. Each bound validates and one past it is refused.
    #[test]
    fn settings_validation_uses_the_shared_exec_bounds() {
        let at_max = json!({
            "site_title": "Looking Glass",
            "default_theme": "system",
            "exec_max_concurrent": EXEC_MAX_CONCURRENT_MAX,
            "exec_timeout_secs": EXEC_TIMEOUT_SECS_MAX,
            "exec_max_output_kib": EXEC_MAX_OUTPUT_KIB_MAX,
            "exec_rate_max": EXEC_RATE_MAX_MAX,
            "exec_rate_window_secs": EXEC_RATE_WINDOW_SECS_MAX
        });
        let input = |value: &serde_json::Value| -> SettingsInput {
            serde_json::from_value(value.clone()).unwrap()
        };
        assert!(input(&at_max).validate().is_ok());
        for field in [
            "exec_max_concurrent",
            "exec_timeout_secs",
            "exec_max_output_kib",
            "exec_rate_max",
            "exec_rate_window_secs",
        ] {
            let mut over = at_max.clone();
            over[field] = json!(at_max[field].as_u64().unwrap() + 1);
            assert!(input(&over).validate().is_err(), "{field} past its bound");
        }
    }

    // A location delete that lands after a handler's existence check and
    // before its write (the before_write barrier) is never undone. An edit must
    // not resurrect the location, and a child write must not orphan a row.
    #[tokio::test]
    async fn a_location_delete_before_the_write_is_never_undone() {
        let state = test_state();
        let location = json!({ "name": "Frankfurt", "geo_label": "DE", "kind": "local", "offered_methods": ["ping"] });
        let ip = json!({ "family": "v4", "address": "203.0.113.10" });
        let iperf = json!({ "label": "x", "host": "h", "port": 5201, "cmd_incoming": "a", "cmd_outgoing": "b" });
        let file = json!({ "label": "x", "declared_size": "1 B", "source_ref": "x.bin" });
        let st = || State(state.clone());
        let hm = HeaderMap::new;

        for write in 0..7 {
            let (_, Json(loc)) = create_location(st(), admin(), hm(), body(&location))
                .await
                .unwrap();
            let id = loc.id;
            let p = || Path(id.clone());
            let (_, Json(test_ip)) = create_test_ip(st(), admin(), p(), hm(), body(&ip))
                .await
                .unwrap();
            let (_, Json(endpoint)) = create_iperf(st(), admin(), p(), hm(), body(&iperf))
                .await
                .unwrap();
            let (_, Json(test_file)) = create_test_file(st(), admin(), p(), hm(), body(&file))
                .await
                .unwrap();

            let (store, doomed) = (state.store.clone(), id.clone());
            set_before_write(move || {
                store.delete_location(&doomed).unwrap();
            });
            let result = match write {
                0 => update_location(st(), admin(), p(), hm(), body(&location))
                    .await
                    .map(drop),
                1 => create_test_ip(st(), admin(), p(), hm(), body(&ip))
                    .await
                    .map(drop),
                2 => update_test_ip(st(), admin(), Path(test_ip.id), hm(), body(&ip))
                    .await
                    .map(drop),
                3 => create_iperf(st(), admin(), p(), hm(), body(&iperf))
                    .await
                    .map(drop),
                4 => update_iperf(st(), admin(), Path(endpoint.id), hm(), body(&iperf))
                    .await
                    .map(drop),
                5 => create_test_file(st(), admin(), p(), hm(), body(&file))
                    .await
                    .map(drop),
                _ => update_test_file(st(), admin(), Path(test_file.id), hm(), body(&file))
                    .await
                    .map(drop),
            };
            set_before_write(|| {});

            assert!(
                matches!(result, Err(ApiError::NotFound)),
                "write {write} succeeded over a delete: {result:?}"
            );
            assert!(
                state.store.get_location(&id).unwrap().is_none(),
                "write {write} resurrected the location"
            );
            let orphans = state.store.list_test_ips(&id).unwrap().len()
                + state.store.list_iperf(&id).unwrap().len()
                + state.store.list_test_files(&id).unwrap().len();
            assert_eq!(orphans, 0, "write {write} orphaned a child");
        }
    }

    // A child delete that lands after an edit's existence check and
    // before its write (the before_write barrier) is never undone: the edit
    // answers 404 and the row stays gone. Every kind is checked before failing.
    #[tokio::test]
    async fn a_child_delete_before_its_edit_is_never_undone() {
        let state = test_state();
        let location = json!({ "name": "Frankfurt", "geo_label": "DE", "kind": "local", "offered_methods": ["ping"] });
        let ip = json!({ "family": "v4", "address": "203.0.113.10" });
        let iperf = json!({ "label": "x", "host": "h", "port": 5201, "cmd_incoming": "a", "cmd_outgoing": "b" });
        let file = json!({ "label": "x", "declared_size": "1 B", "source_ref": "x.bin" });
        let st = || State(state.clone());
        let hm = HeaderMap::new;
        let (_, Json(loc)) = create_location(st(), admin(), hm(), body(&location))
            .await
            .unwrap();
        let p = || Path(loc.id.clone());
        let (_, Json(test_ip)) = create_test_ip(st(), admin(), p(), hm(), body(&ip))
            .await
            .unwrap();
        let (_, Json(endpoint)) = create_iperf(st(), admin(), p(), hm(), body(&iperf))
            .await
            .unwrap();
        let (_, Json(test_file)) = create_test_file(st(), admin(), p(), hm(), body(&file))
            .await
            .unwrap();

        let mut undone = Vec::new();
        for kind in ["test_ip", "iperf", "file"] {
            let store = state.store.clone();
            let (ip_id, iperf_id, file_id) = (
                test_ip.id.clone(),
                endpoint.id.clone(),
                test_file.id.clone(),
            );
            set_before_write(move || {
                match kind {
                    "test_ip" => store.delete_test_ip(&ip_id),
                    "iperf" => store.delete_iperf(&iperf_id),
                    _ => store.delete_test_file(&file_id),
                }
                .unwrap();
            });
            let (result, row_back) = match kind {
                "test_ip" => (
                    update_test_ip(st(), admin(), Path(test_ip.id.clone()), hm(), body(&ip))
                        .await
                        .map(drop),
                    state.store.get_test_ip(&test_ip.id).unwrap().is_some(),
                ),
                "iperf" => (
                    update_iperf(st(), admin(), Path(endpoint.id.clone()), hm(), body(&iperf))
                        .await
                        .map(drop),
                    state.store.get_iperf(&endpoint.id).unwrap().is_some(),
                ),
                _ => (
                    update_test_file(st(), admin(), Path(test_file.id.clone()), hm(), body(&file))
                        .await
                        .map(drop),
                    state.store.get_test_file(&test_file.id).unwrap().is_some(),
                ),
            };
            set_before_write(|| {});
            if !matches!(result, Err(ApiError::NotFound)) || row_back {
                undone.push(format!("{kind}: result={result:?} row_back={row_back}"));
            }
        }
        assert!(undone.is_empty(), "edit undid a child delete: {undone:?}");
    }
}
