//! Agent enrollment: the auth boundary of the RCE surface (Slice 7). Two handlers,
//! two audiences:
//!
//! - `create_enrollment` (admin, session-gated): for a **remote** location, mint a
//!   single-use, 15-minute, 256-bit token, store only its hash, and hand the admin a
//!   no-edit copy-paste install command carrying central's identity fingerprint.
//! - `enroll_agent` (agent, token-gated): a fresh host presents its token over a
//!   verified-TLS transport; central consumes the token exactly once and issues a
//!   256-bit per-agent credential, Argon2id-hashed at rest and returned in cleartext
//!   exactly once. A reused/expired/unknown token is refused uniformly with no
//!   credential (AC8); cleartext transport is refused (AC34/FR-071).
//!
//! The agent verifies **central's** pinned identity at enrollment (AC35): the pin's
//! origin is the fingerprint in the install command, checked agent-side against the
//! identity central presents at the origin serving `/api/enroll` — no
//! trust-on-first-use. With no `LG_CENTRAL_URL`, `LG_CENTRAL_CERT` or
//! `LG_CENTRAL_IDENTITY` that origin is the tunnel listener, which serves
//! `/api/enroll` too, and the pin is its certificate's; setting them keeps a
//! separate HTTPS API origin and certificate.

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use redb::{ReadableDatabase, ReadableTable};
use rustls::pki_types::ServerName;
use rustls::pki_types::{pem::PemObject, CertificateDer};
use serde::Serialize;
use shared::protocol::{
    identity_pin, sha256_hex, EnrollRequest, EnrollResponse, EnrollmentParams, PROTOCOL_VERSION,
};

use crate::admin_api::is_https_url;
use crate::auth::{
    argon2_off_runtime, hash_password, random_hex, random_id, AdminSession, ApiError, ClientContext,
};
use crate::observability::{correlation_id, log_validation_rejected};
use crate::store::{unix_now, Agent, EnrollmentToken, NodeKind, StoreError, ENROLLMENT_TOKEN};
use crate::AppState;

/// Enrollment tokens live for 15 minutes — long enough to paste and run the install
/// command, short enough to bound the replay window (decisions.md q-011).
const TOKEN_TTL_SECS: u64 = 15 * 60;
/// 256 bits of CSPRNG entropy for both the token and the per-agent credential.
const SECRET_BYTES: usize = 32;

const ID_ENV: &str = "LG_CENTRAL_IDENTITY";
const CERT_ENV: &str = "LG_CENTRAL_CERT";
const URL_ENV: &str = "LG_CENTRAL_URL";
const TUNNEL_URL_ENV: &str = "LG_TUNNEL_URL";
const AGENT_URL_ENV: &str = "LG_AGENT_URL";
const AGENT_SHA_ENV: &str = "LG_AGENT_SHA256";
const AGENT_INSTALL_SCRIPT_URL_ENV: &str = "LG_AGENT_INSTALL_SCRIPT_URL";
const AGENT_INSTALL_SCRIPT_SHA_ENV: &str = "LG_AGENT_INSTALL_SCRIPT_SHA256";
const DEFAULT_CENTRAL_URL: &str = "https://localhost";
const DEFAULT_TUNNEL_URL: &str = "https://localhost:8443";

impl AppState {
    #[doc(hidden)]
    pub fn enrollment_token_count(&self, location_id: &str) -> Result<usize, StoreError> {
        let transaction = self
            .store
            .database()
            .begin_read()
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        let table = transaction
            .open_table(ENROLLMENT_TOKEN)
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        table
            .iter()
            .map_err(|error| StoreError::Backend(error.to_string()))?
            .try_fold(0, |count, entry| {
                let (_, value) = entry.map_err(|error| StoreError::Backend(error.to_string()))?;
                let token: EnrollmentToken = serde_json::from_slice(value.value())
                    .map_err(|error| StoreError::Backend(error.to_string()))?;
                Ok(count + usize::from(token.location_id == location_id))
            })
    }
}

/// Central's API identity — the certificate an enrolling agent pins.
/// Its [`Self::fingerprint`] (SHA-256 hex of the certificate's public key, so a
/// renewal that keeps the key keeps it) is what the install command carries: the
/// pin of the certificate presented by the origin that serves `/api/enroll`.
pub struct CentralIdentity {
    fingerprint: String,
    /// Whether the pin comes from a certificate. The agent pins the certificate TLS
    /// presents, so any other material can never match.
    certificate: bool,
}

impl CentralIdentity {
    /// Load stable identity material from `LG_CENTRAL_CERT` or `LG_CENTRAL_IDENTITY`,
    /// or generate ephemeral material and warn. An ephemeral identity changes the
    /// fingerprint on restart, invalidating outstanding install commands, so a real
    /// deploy sets it to the API endpoint's presented certificate.
    pub fn from_env_or_generate() -> Self {
        if let Ok(cert_path) = std::env::var(CERT_ENV) {
            if !cert_path.is_empty() {
                match load_end_entity_cert(&cert_path) {
                    Ok(material) => return Self::from_material(material),
                    Err(error) => tracing::warn!(
                        %error,
                        "{CERT_ENV} could not be loaded; falling back to {ID_ENV} or ephemeral identity"
                    ),
                }
            }
        }
        match std::env::var(ID_ENV) {
            Ok(value) if !value.is_empty() => Self {
                fingerprint: identity_pin(value.as_bytes()),
                certificate: false,
            },
            _ => {
                let material = random_hex(SECRET_BYTES);
                tracing::warn!(
                    "{CERT_ENV}/{ID_ENV} unset — using an ephemeral central API identity; install commands are \
                     refused until {CERT_ENV} names the certificate the HTTPS API presents."
                );
                Self {
                    fingerprint: identity_pin(material.as_bytes()),
                    certificate: false,
                }
            }
        }
    }

    /// Identity from explicit DER certificate material — the seam tests use.
    pub fn from_material(material: Vec<u8>) -> Self {
        Self::from_certificate_pin(identity_pin(&material))
    }

    /// Identity from the pin of a certificate central presents, such as the
    /// tunnel listener's.
    fn from_certificate_pin(fingerprint: String) -> Self {
        Self {
            fingerprint,
            certificate: true,
        }
    }

    /// The pin the install command embeds and the agent verifies: see [`identity_pin`].
    pub fn fingerprint(&self) -> String {
        self.fingerprint.clone()
    }
}

fn load_end_entity_cert(path: &str) -> std::io::Result<Vec<u8>> {
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    certs
        .first()
        .map(|cert| cert.as_ref().to_vec())
        .ok_or_else(|| std::io::Error::other("central certificate file contains no certificate"))
}

/// Deployment-scoped enrollment config carried in [`AppState`]: where agents reach
/// central's HTTPS API enroll endpoint, and the identity they pin it by.
#[derive(Clone)]
pub struct EnrollConfig {
    pub central_url: Arc<str>,
    /// The web origin admin activation links use: `central_url`, or with the
    /// tunnel default, the tunnel host on the default HTTPS port.
    pub web_url: Arc<str>,
    pub tunnel_url: Arc<str>,
    pub identity: Arc<CentralIdentity>,
    pub agent_url: Arc<str>,
    pub agent_sha256: Arc<str>,
    pub agent_install_script_url: Arc<str>,
    pub agent_install_script_sha256: Arc<str>,
    /// The pin of the tunnel listener's certificate, when it runs. Agents pin
    /// every tunnel connect to the identity they enrolled by.
    pub tunnel_pin: Option<Arc<str>>,
}

impl EnrollConfig {
    /// Read the enrollment config. `tunnel_pin` is the running tunnel listener's
    /// pin. With it and none of `LG_CENTRAL_URL`, `LG_CENTRAL_CERT` or
    /// `LG_CENTRAL_IDENTITY` set, agents enroll at the tunnel origin under the
    /// tunnel pin; otherwise those variables apply as before.
    pub fn from_env(tunnel_pin: Option<&str>) -> Result<Self, String> {
        let unset = |name| std::env::var(name).map_or(true, |value| value.is_empty());
        let tunnel_default =
            tunnel_pin.filter(|_| unset(URL_ENV) && unset(CERT_ENV) && unset(ID_ENV));
        let tunnel_url =
            std::env::var(TUNNEL_URL_ENV).unwrap_or_else(|_| DEFAULT_TUNNEL_URL.to_string());
        if !is_tunnel_origin(&tunnel_url) {
            return Err(format!(
                "{TUNNEL_URL_ENV} must be https://host:port with an explicit port and no bracketed IPv6 literal, got {tunnel_url:?}"
            ));
        }
        let (central_url, web_url, identity) = match tunnel_default {
            Some(pin) => {
                tracing::info!(
                    %tunnel_url,
                    "{URL_ENV}/{CERT_ENV}/{ID_ENV} unset — agents enroll at the tunnel origin under its pin"
                );
                if unset(TUNNEL_URL_ENV) {
                    tracing::warn!(
                        "{TUNNEL_URL_ENV} unset — install commands point at {DEFAULT_TUNNEL_URL}, \
                         which remote agents cannot reach; set {TUNNEL_URL_ENV} (or LG_DOMAIN with \
                         Compose) to central's public name"
                    );
                }
                // A valid tunnel URL is `https://host:port`, so this drops the port.
                let host = tunnel_url
                    .rsplit_once(':')
                    .map_or(tunnel_url.as_str(), |(host, _)| host);
                (
                    tunnel_url.clone(),
                    host.to_string(),
                    CentralIdentity::from_certificate_pin(pin.to_string()),
                )
            }
            None => {
                let central_url =
                    std::env::var(URL_ENV).unwrap_or_else(|_| DEFAULT_CENTRAL_URL.to_string());
                (
                    central_url.clone(),
                    central_url,
                    CentralIdentity::from_env_or_generate(),
                )
            }
        };
        let agent_url = std::env::var(AGENT_URL_ENV).unwrap_or_default();
        let agent_sha256 = std::env::var(AGENT_SHA_ENV).unwrap_or_default();
        let agent_install_script_url =
            std::env::var(AGENT_INSTALL_SCRIPT_URL_ENV).unwrap_or_default();
        let agent_install_script_sha256 =
            std::env::var(AGENT_INSTALL_SCRIPT_SHA_ENV).unwrap_or_default();
        let config = Self {
            central_url: Arc::from(central_url.as_str()),
            web_url: Arc::from(web_url.as_str()),
            tunnel_url: Arc::from(tunnel_url.as_str()),
            identity: Arc::new(identity),
            agent_url: Arc::from(agent_url.as_str()),
            agent_sha256: Arc::from(agent_sha256.as_str()),
            agent_install_script_url: Arc::from(agent_install_script_url.as_str()),
            agent_install_script_sha256: Arc::from(agent_install_script_sha256.as_str()),
            tunnel_pin: None,
        };
        Ok(match tunnel_pin {
            Some(pin) => config.with_tunnel_pin(pin),
            None => config,
        })
    }

    /// Build config from explicit material — the seam the integration tests inject a
    /// known fingerprint through.
    pub fn for_test(central_url: &str, identity_material: Vec<u8>) -> Self {
        Self::for_test_with_agent(
            central_url,
            identity_material,
            "https://downloads.example/lg-agent",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "https://downloads.example/install-agent.sh",
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        )
    }

    pub fn for_test_with_agent(
        central_url: &str,
        identity_material: Vec<u8>,
        agent_url: &str,
        agent_sha256: &str,
        agent_install_script_url: &str,
        agent_install_script_sha256: &str,
    ) -> Self {
        let identity = CentralIdentity::from_material(identity_material);
        Self {
            central_url: Arc::from(central_url),
            web_url: Arc::from(central_url),
            tunnel_url: Arc::from("https://central.test:8443"),
            // A running tunnel listener whose key matches the enrollment pin.
            tunnel_pin: Some(Arc::from(identity.fingerprint())),
            identity: Arc::new(identity),
            agent_url: Arc::from(agent_url),
            agent_sha256: Arc::from(agent_sha256),
            agent_install_script_url: Arc::from(agent_install_script_url),
            agent_install_script_sha256: Arc::from(agent_install_script_sha256),
        }
    }

    pub fn with_tunnel_url(mut self, tunnel_url: &str) -> Self {
        self.tunnel_url = Arc::from(tunnel_url);
        self
    }

    /// Record the tunnel listener's pin. One that differs from the enrollment
    /// pin is an error: agents enrolled by it could never connect.
    pub fn with_tunnel_pin(mut self, tunnel_pin: &str) -> Self {
        self.tunnel_pin = Some(Arc::from(tunnel_pin));
        if self.pins_differ() {
            tracing::error!(
                %tunnel_pin,
                enroll_pin = %self.identity.fingerprint(),
                "the enrollment pin ({CERT_ENV}) does not match the tunnel certificate's key; \
                 agents pin both to it, so enrollment is refused until they match"
            );
        }
        self
    }

    fn pins_differ(&self) -> bool {
        self.tunnel_pin
            .as_deref()
            .is_some_and(|pin| pin != self.identity.fingerprint())
    }

    /// Why an agent enrolled now could never hold a tunnel: no tunnel listener
    /// runs, or its key differs from the enrollment pin.
    fn tunnel_refusal(&self) -> Option<(&'static str, ApiError)> {
        if self.tunnel_pin.is_none() {
            Some(("tunnel_unavailable", tunnel_unavailable()))
        } else if self.pins_differ() {
            Some(("identity_mismatch", identity_mismatch()))
        } else {
            None
        }
    }
}

fn tunnel_unavailable() -> ApiError {
    ApiError::Coded(
        StatusCode::SERVICE_UNAVAILABLE,
        "tunnel_unavailable",
        "Central's agent tunnel is not running, so an enrolled agent could never connect. Either central's own tunnel.crt and tunnel.key beside its database are missing or damaged (repair or remove them; see central's log), or the configured LG_TUNNEL_CERT and LG_TUNNEL_KEY do not load.",
    )
}

/// Every ticket is refused at redeem while the pins differ, so minting one is
/// refused too, with the same operator-facing error.
fn identity_mismatch() -> ApiError {
    ApiError::Coded(
        StatusCode::SERVICE_UNAVAILABLE,
        "identity_mismatch",
        "Central's tunnel certificate does not match its enrollment certificate; the operator must fix LG_CENTRAL_CERT or LG_TUNNEL_CERT.",
    )
}

/// The agent-facing enroll endpoint (token-gated, cleartext-refused) plus the
/// admin-facing ticket endpoint (session-gated). Both merge into the setup-gated
/// api router in `lib.rs`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/locations/{id}/enroll", post(create_enrollment))
        .merge(agent_route())
}

/// The agent-facing enroll endpoint alone, which the tunnel listener serves too.
pub(crate) fn agent_route() -> Router<AppState> {
    Router::new().route("/api/enroll", post(enroll_agent))
}

/// What the admin sees after minting a token: the no-edit install command, the raw
/// token (shown once, over the admin's authenticated session), the fingerprint the
/// agent will pin, and the absolute expiry the dialog counts down to.
#[derive(Serialize)]
struct EnrollmentTicket {
    install_command: String,
    token: String,
    fingerprint: String,
    agent_sha256: String,
    install_script_sha256: String,
    expires_at: u64,
}

/// Admin mints an enrollment ticket for a remote location (AC7 token half, FR-021).
async fn create_enrollment(
    State(state): State<AppState>,
    _admin: AdminSession,
    ctx: ClientContext,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<EnrollmentTicket>, ApiError> {
    let correlation_id = correlation_id(&headers);
    if !ctx.secure {
        tracing::warn!(
            event = "agent.enroll",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason = "insecure_transport",
            "enrollment ticket rejected"
        );
        return Err(ApiError::CleartextRefused);
    }
    let location = state.store.get_location(&id)?.ok_or(ApiError::NotFound)?;
    if location.kind != NodeKind::Remote {
        log_validation_rejected(&correlation_id, "agent.enroll", "local_location");
        return Err(ApiError::Validation(
            "Enrollment applies only to remote locations.".to_string(),
        ));
    }
    if !is_https_origin(&state.enroll.central_url) {
        log_validation_rejected(&correlation_id, "agent.enroll", "invalid_central_url");
        return Err(ApiError::Validation(
            "LG_CENTRAL_URL must be a plain https API origin with a valid host and optional non-zero port.".to_string(),
        ));
    }
    if !is_tunnel_origin(&state.enroll.tunnel_url) {
        log_validation_rejected(&correlation_id, "agent.enroll", "invalid_tunnel_url");
        return Err(ApiError::Validation(
            "LG_TUNNEL_URL must be a plain https tunnel origin: a valid host (not a bracketed IPv6 literal) and an explicit non-zero port.".to_string(),
        ));
    }
    if !state.enroll.identity.certificate {
        log_validation_rejected(&correlation_id, "agent.enroll", "identity_not_certificate");
        return Err(ApiError::Validation(
            "LG_CENTRAL_CERT must name the certificate the HTTPS API presents; agents cannot pin LG_CENTRAL_IDENTITY or an ephemeral identity.".to_string(),
        ));
    }
    if let Some((reason, error)) = state.enroll.tunnel_refusal() {
        log_validation_rejected(&correlation_id, "agent.enroll", reason);
        return Err(error);
    }
    let Some(agent_sha256) = normalize_sha256(&state.enroll.agent_sha256) else {
        log_validation_rejected(&correlation_id, "agent.enroll", "invalid_agent_sha256");
        return Err(ApiError::Validation(
            "Agent binary SHA-256 must be 64 hex characters.".to_string(),
        ));
    };
    let Some(install_script_sha256) = normalize_sha256(&state.enroll.agent_install_script_sha256)
    else {
        log_validation_rejected(&correlation_id, "agent.enroll", "invalid_installer_sha256");
        return Err(ApiError::Validation(
            "Agent install script SHA-256 must be 64 hex characters.".to_string(),
        ));
    };
    if !is_https_url(&state.enroll.agent_url)
        || !is_https_url(&state.enroll.agent_install_script_url)
    {
        log_validation_rejected(&correlation_id, "agent.enroll", "invalid_asset_url");
        return Err(ApiError::Validation(
            "Agent install script URL and binary URL must be valid https URLs with a host."
                .to_string(),
        ));
    }

    let now = unix_now();
    let raw_token = random_hex(SECRET_BYTES);
    let token = EnrollmentToken {
        id: random_id(),
        location_id: location.id.clone(),
        token_hash: sha256_hex(raw_token.as_bytes()),
        expires_at: now + TOKEN_TTL_SECS,
        used_at: None,
    };
    state.store.mint_enrollment_token(&token, now)?;

    let params = EnrollmentParams {
        central_url: state.enroll.central_url.to_string(),
        tunnel_url: state.enroll.tunnel_url.to_string(),
        fingerprint: state.enroll.identity.fingerprint(),
        token: raw_token.clone(),
        agent_url: state.enroll.agent_url.to_string(),
        agent_sha256,
        install_script_url: state.enroll.agent_install_script_url.to_string(),
        install_script_sha256,
    };
    // Never log the raw token — only the location it was minted for (FR-064).
    tracing::info!(
        event = "agent.enroll",
        correlation_id = %correlation_id,
        outcome = "ticket_created",
        location_id = %location.id,
        "enrollment token generated"
    );

    Ok(Json(EnrollmentTicket {
        install_command: params.install_command(),
        token: raw_token,
        fingerprint: params.fingerprint,
        agent_sha256: params.agent_sha256,
        install_script_sha256: params.install_script_sha256,
        expires_at: token.expires_at,
    }))
}

fn normalize_sha256(value: &str) -> Option<String> {
    if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(value.to_ascii_lowercase())
    } else {
        None
    }
}

fn is_https_origin(value: &str) -> bool {
    let Some(authority) = value.strip_prefix("https://") else {
        return false;
    };
    if authority.is_empty()
        || authority.contains('@')
        || authority.contains('/')
        || authority.contains('?')
        || authority.contains('#')
    {
        return false;
    }

    if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, suffix)) = rest.split_once(']') else {
            return false;
        };
        if !matches!(host.parse::<IpAddr>(), Ok(IpAddr::V6(_))) {
            return false;
        }
        return match suffix.strip_prefix(':') {
            Some(port) => parse_non_zero_port(port).is_some(),
            None => suffix.is_empty(),
        };
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && !host.contains(':') => (host, Some(port)),
        Some((_, _)) => return false,
        None if authority.contains(':') => return false,
        None => (authority, None),
    };
    if host.is_empty() || ServerName::try_from(host.to_string()).is_err() {
        return false;
    }
    if let Some(port) = port {
        return parse_non_zero_port(port).is_some();
    }
    true
}

/// The agent dials only an explicit `https://host:port`, so the tunnel
/// origin also needs a port and must not be a bracketed IPv6 literal.
fn is_tunnel_origin(value: &str) -> bool {
    is_https_origin(value)
        && value
            .strip_prefix("https://")
            .is_some_and(|authority| !authority.starts_with('[') && authority.contains(':'))
}

fn parse_non_zero_port(value: &str) -> Option<u16> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port = value.parse::<u16>().ok()?;
    (port != 0).then_some(port)
}

/// A fresh host enrolls with its token (AC7 credential half, AC8, AC34/FR-071).
async fn enroll_agent(
    State(state): State<AppState>,
    ctx: ClientContext,
    headers: HeaderMap,
    Json(req): Json<EnrollRequest>,
) -> Result<Json<EnrollResponse>, ApiError> {
    let correlation_id = correlation_id(&headers);
    // FR-071: enrollment over cleartext is refused; no token consumed, no credential.
    if !ctx.secure {
        tracing::warn!(
            event = "agent.enroll",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason = "insecure_transport",
            "agent enrollment rejected"
        );
        return Err(ApiError::CleartextRefused);
    }
    if req.protocol_version != PROTOCOL_VERSION {
        log_validation_rejected(
            &correlation_id,
            "agent.enroll",
            "unsupported_protocol_version",
        );
        return Err(ApiError::Validation(
            "Unsupported protocol version.".to_string(),
        ));
    }

    let now = unix_now();
    let token_hash = sha256_hex(req.token.as_bytes());
    // Unknown, expired, and already-used tokens are all refused the same way — no
    // oracle tells an attacker which state the token was in (AC8).
    let token = state
        .store
        .find_token_by_hash(&token_hash)?
        .ok_or_else(|| {
            tracing::warn!(
                event = "agent.enroll",
                correlation_id = %correlation_id,
                outcome = "rejected",
                reason = "invalid_token",
                "agent enrollment rejected"
            );
            ApiError::Unauthorized
        })?;
    if now > token.expires_at || token.used_at.is_some() {
        tracing::warn!(
            event = "agent.enroll",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason = "invalid_token",
            "agent enrollment rejected"
        );
        return Err(ApiError::Unauthorized);
    }

    // The agent pins every tunnel connect to this enrollment's identity, so no
    // tunnel listener, or one with another key, would strand it: refuse while
    // the token is still unspent.
    if let Some((reason, error)) = state.enroll.tunnel_refusal() {
        tracing::warn!(
            event = "agent.enroll",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason,
            "agent enrollment rejected"
        );
        return Err(error);
    }

    // Issue the long-lived per-agent credential: 256-bit CSPRNG, Argon2id-hashed at
    // rest (the Slice-2 hashing seam). The cleartext is returned once, here.
    let credential = random_hex(SECRET_BYTES);
    let hashed = credential.clone();
    let credential_hash = argon2_off_runtime(move || hash_password(&hashed))
        .await
        .ok_or(ApiError::Internal)??;
    let agent = Agent {
        id: random_id(),
        location_id: token.location_id,
        credential_hash,
        enrolled_at: now,
        last_seen: None,
        revoked: false,
    };
    #[cfg(test)]
    tests::before_agent_write(&agent.location_id);
    // Atomic single-use: one transaction consumes the token, re-checks the
    // location and writes the agent. A lost race (another redemption,
    // Regenerate, Revoke or a location delete) writes nothing and issues nothing.
    if !state
        .store
        .redeem_enrollment_token(&token.id, now, &agent)?
    {
        tracing::warn!(
            event = "agent.enroll",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason = "token_race_lost",
            "agent enrollment rejected"
        );
        return Err(ApiError::Unauthorized);
    }
    // No secret in the log line — id and location only (FR-064).
    tracing::info!(
        event = "agent.enroll",
        correlation_id = %correlation_id,
        outcome = "credential_issued",
        location_id = %agent.location_id,
        agent_id = %agent.id,
        "agent enrolled"
    );

    Ok(Json(EnrollResponse {
        protocol_version: PROTOCOL_VERSION,
        agent_id: agent.id,
        credential,
    }))
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Duration;

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::Json;
    use shared::protocol::{identity_pin, sha256_hex, EnrollRequest, PROTOCOL_VERSION};

    use super::{create_enrollment, enroll_agent, CentralIdentity, DEFAULT_CENTRAL_URL};
    use crate::auth::{random_id, AdminSession, ClientContext};
    use crate::store::{EnrollmentToken, Location, LocationStatus, NodeKind};
    use crate::{
        AppState, EnrollConfig, LoginLimiter, RunService, Store, TransportConfig, TunnelHub,
    };

    type Hook = Box<dyn FnOnce() + Send>;

    /// Runs inside `enroll_agent` after the token checked out and before the
    /// agent is persisted, where a racing delete or second redemption lands.
    /// Keyed by location id so parallel tests never take each other's hook.
    static BEFORE_AGENT_WRITE: Mutex<Vec<(String, Hook)>> = Mutex::new(Vec::new());

    pub(super) fn before_agent_write(location_id: &str) {
        let hook = {
            let mut hooks = BEFORE_AGENT_WRITE.lock().unwrap();
            let found = hooks.iter().position(|(id, _)| id == location_id);
            found.map(|index| hooks.swap_remove(index).1)
        };
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Serializes the tests that set the identity env (`LG_CENTRAL_CERT`,
    /// `LG_CENTRAL_IDENTITY`), which one process shares.
    static IDENTITY_ENV: Mutex<()> = Mutex::new(());

    fn on_agent_write(location_id: &str, hook: impl FnOnce() + Send + 'static) {
        let mut hooks = BEFORE_AGENT_WRITE.lock().unwrap();
        hooks.push((location_id.to_string(), Box::new(hook)));
    }

    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "lg-enroll-unit-{}-{}",
            std::process::id(),
            random_id()
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        AppState {
            store: Store::open(dir.join("db.redb")).expect("open test store"),
            transport: TransportConfig::new([std::net::IpAddr::from(Ipv4Addr::LOCALHOST)]),
            login_limiter: Arc::new(LoginLimiter::default()),
            setup_token: None,
            run: RunService::for_test(8, Duration::from_secs(30), 100),
            files_root: Arc::from(dir.as_path()),
            enroll: EnrollConfig::for_test("https://central.test", b"identity".to_vec()),
            tunnel_hub: TunnelHub::new(),
        }
    }

    /// A remote location holding one fresh token; returns the raw token.
    fn seed(state: &AppState, location_id: &str) -> String {
        state
            .store
            .put_location(&Location {
                id: location_id.to_string(),
                name: "Remote".to_string(),
                geo_label: "DE".to_string(),
                map_query: None,
                facility: None,
                facility_url: None,
                kind: NodeKind::Remote,
                data_plane_origin: None,
                asn: None,
                offered_methods: Vec::new(),
                status: LocationStatus::Offline,
                created_at: 0,
            })
            .unwrap();
        let raw = format!("token-for-{location_id}");
        state
            .store
            .put_enrollment_token(&EnrollmentToken {
                id: format!("t-{location_id}"),
                location_id: location_id.to_string(),
                token_hash: sha256_hex(raw.as_bytes()),
                expires_at: u64::MAX,
                used_at: None,
            })
            .unwrap();
        raw
    }

    async fn enroll(state: AppState, token: String) -> StatusCode {
        let ctx = ClientContext {
            ip: None,
            secure: true,
        };
        let req = EnrollRequest {
            protocol_version: PROTOCOL_VERSION,
            token,
        };
        match enroll_agent(State(state), ctx, HeaderMap::new(), Json(req)).await {
            Ok(_) => StatusCode::OK,
            Err(error) => error.into_response().status(),
        }
    }

    // F-184/C-041: the new credential is hashed through argon2_off_runtime,
    // so a burst of enrollments cannot hold the async workers.
    #[tokio::test]
    async fn the_credential_hash_runs_off_the_async_workers() {
        let state = test_state();
        let token = seed(&state, "off-runtime");

        assert_eq!(enroll(state.clone(), token).await, StatusCode::OK);

        let agents = state.store.list_agents("off-runtime").unwrap();
        assert_eq!(
            crate::auth::tests::off_runtime_count(&agents[0].credential_hash),
            1,
            "the credential must hash off the runtime"
        );
    }

    // A location delete that commits while a redemption is in flight
    // leaves no agent behind, so no unrevocable credential survives.
    #[tokio::test]
    async fn a_redemption_racing_a_location_delete_leaves_no_agent() {
        let state = test_state();
        let token = seed(&state, "race-delete");
        let store = state.store.clone();
        on_agent_write("race-delete", move || {
            assert!(store.delete_location("race-delete").unwrap());
        });

        let status = enroll(state.clone(), token).await;

        assert!(
            state.store.all_agents().unwrap().is_empty(),
            "no agent may outlive its deleted location"
        );
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // Two redemptions of one token that both pass the read-only check
    // still yield exactly one agent; the loser writes nothing.
    #[tokio::test]
    async fn a_concurrent_double_redeem_yields_exactly_one_agent() {
        let state = test_state();
        let token = seed(&state, "race-double");
        let (racer_state, racer_token) = (state.clone(), token.clone());
        let (sender, racer) = mpsc::channel();
        on_agent_write("race-double", move || {
            // The racer runs to completion while the first request is parked
            // between its token check and its write.
            let status = std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap()
                    .block_on(enroll(racer_state, racer_token))
            })
            .join()
            .unwrap();
            sender.send(status).unwrap();
        });

        let first = enroll(state.clone(), token).await;
        let second = racer.recv().expect("the racer ran");

        assert_eq!(
            state.store.all_agents().unwrap().len(),
            1,
            "one token, one agent"
        );
        let mut statuses = [first, second];
        statuses.sort();
        assert_eq!(statuses, [StatusCode::OK, StatusCode::UNAUTHORIZED]);
    }

    // The asset URL gate is the one strict https check the admin links
    // use. A second, drifted copy fails here: port 99999 and an empty port are
    // refused only by the strict check, and a bracketed IPv6 host with a port
    // is accepted by it.
    #[tokio::test]
    async fn asset_urls_use_the_shared_strict_https_check() {
        for (agent_url, accepted) in [
            ("https://downloads.example:99999/lg-agent", false),
            ("https://downloads.example:65536/lg-agent", false),
            ("https://downloads.example:/lg-agent", false),
            ("https://[2001:db8::1]:8443/lg-agent", true),
            // Shapes http's parser lets through and a browser refuses.
            ("https://[2001:db8::1]x:443/lg-agent", false),
            ("https://x[::1]:443/lg-agent", false),
            ("https://downloads.example:+443/lg-agent", false),
            ("https://256.1.1.1/lg-agent", false),
            ("https://downloads.123/lg-agent", false),
            ("https://downloads.0x1/lg-agent", false),
            ("https://192.0.2.10:8443/lg-agent", true),
        ] {
            let mut state = test_state();
            state.enroll = crate::EnrollConfig::for_test_with_agent(
                "https://central.test",
                b"identity".to_vec(),
                agent_url,
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "https://downloads.example/install-agent.sh",
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            );
            seed(&state, "assets");
            let result = create_enrollment(
                State(state),
                AdminSession {
                    admin_id: "alice".to_string(),
                },
                ClientContext {
                    ip: None,
                    secure: true,
                },
                axum::extract::Path("assets".to_string()),
                HeaderMap::new(),
            )
            .await;
            let status =
                result.map_or_else(|error| error.into_response().status(), |_| StatusCode::OK);
            let expected = if accepted {
                StatusCode::OK
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            };
            assert_eq!(status, expected, "agent URL {agent_url:?}");
        }
    }

    #[test]
    fn default_enrollment_url_is_the_api_origin_not_the_tunnel_listener() {
        assert_eq!(DEFAULT_CENTRAL_URL, "https://localhost");
        assert_ne!(DEFAULT_CENTRAL_URL, "https://localhost:8443");
    }

    async fn mint(state: AppState, location_id: &str) -> StatusCode {
        let result = create_enrollment(
            State(state),
            AdminSession {
                admin_id: "alice".to_string(),
            },
            ClientContext {
                ip: None,
                secure: true,
            },
            axum::extract::Path(location_id.to_string()),
            HeaderMap::new(),
        )
        .await;
        result.map_or_else(|error| error.into_response().status(), |_| StatusCode::OK)
    }

    // The agent dials only an explicit `host:port`, so a bracketed
    // IPv6 literal or a portless tunnel URL is refused when central reads its
    // config, instead of shipping in install commands the agent then mis-dials.
    #[test]
    fn central_config_refuses_tunnel_urls_the_agent_cannot_dial() {
        for tunnel_url in [
            "https://[::1]:8443",
            "https://[2001:db8::1]",
            "https://tunnel.central.example",
        ] {
            std::env::set_var("LG_TUNNEL_URL", tunnel_url);
            assert!(
                EnrollConfig::from_env(None).is_err(),
                "central must refuse LG_TUNNEL_URL={tunnel_url:?} at startup"
            );
        }
        std::env::set_var("LG_TUNNEL_URL", "https://tunnel.central.example:8443");
        let accepted = EnrollConfig::from_env(None);
        std::env::remove_var("LG_TUNNEL_URL");
        assert!(accepted.is_ok(), "an explicit host:port tunnel URL starts");
    }

    // The same rule guards minting, so config built another way cannot ship an
    // undialable tunnel URL either.
    #[tokio::test]
    async fn tunnel_urls_the_agent_cannot_dial_are_refused_at_mint() {
        for tunnel_url in [
            "https://[::1]:8443",
            "https://[2001:db8::1]",
            "https://tunnel.central.example",
        ] {
            let mut state = test_state();
            state.enroll = EnrollConfig::for_test("https://central.test", b"identity".to_vec())
                .with_tunnel_url(tunnel_url);
            seed(&state, "undialable");

            let status = mint(state.clone(), "undialable").await;

            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "tunnel URL {tunnel_url:?}"
            );
            assert_eq!(state.enrollment_token_count("undialable").unwrap(), 1);
        }
    }

    // Agents pin the certificate the HTTPS API presents.
    // LG_CENTRAL_IDENTITY bytes or ephemeral material can never match one, so an
    // identity loaded without LG_CENTRAL_CERT must not mint install commands.
    #[tokio::test]
    async fn an_identity_without_a_certificate_refuses_to_mint() {
        for stable_identity in [None, Some("operator-chosen-identity")] {
            let identity = {
                let _env = IDENTITY_ENV
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                std::env::remove_var("LG_CENTRAL_CERT");
                match stable_identity {
                    Some(value) => std::env::set_var("LG_CENTRAL_IDENTITY", value),
                    None => std::env::remove_var("LG_CENTRAL_IDENTITY"),
                }
                let identity = CentralIdentity::from_env_or_generate();
                std::env::remove_var("LG_CENTRAL_IDENTITY");
                identity
            };
            let mut state = test_state();
            state.enroll.identity = Arc::new(identity);
            seed(&state, "no-cert");

            let status = mint(state.clone(), "no-cert").await;

            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "identity {stable_identity:?} has no certificate to pin"
            );
            assert_eq!(state.enrollment_token_count("no-cert").unwrap(), 1);
        }
    }

    // A certificate loaded from LG_CENTRAL_CERT is the identity agents
    // pin, so it is loaded as a certificate and mints install commands.
    #[tokio::test]
    async fn a_certificate_file_identity_mints_with_its_fingerprint() {
        let der = b"central-api-certificate-der";
        let dir = std::env::temp_dir().join(format!(
            "lg-enroll-cert-{}-{}",
            std::process::id(),
            random_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cert = dir.join("central.pem");
        std::fs::write(
            &cert,
            "-----BEGIN CERTIFICATE-----\nY2VudHJhbC1hcGktY2VydGlmaWNhdGUtZGVy\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let identity = {
            let _env = IDENTITY_ENV
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::env::set_var("LG_CENTRAL_CERT", &cert);
            let identity = CentralIdentity::from_env_or_generate();
            std::env::remove_var("LG_CENTRAL_CERT");
            identity
        };
        assert_eq!(identity.fingerprint(), identity_pin(der));

        let mut state = test_state();
        state.enroll.tunnel_pin = Some(Arc::from(identity.fingerprint()));
        state.enroll.identity = Arc::new(identity);
        seed(&state, "cert-file");
        assert_eq!(mint(state, "cert-file").await, StatusCode::OK);
    }
}
