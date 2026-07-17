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
//! identity central presents at the HTTPS API origin serving `/api/enroll` — no
//! trust-on-first-use. The long-running tunnel is a separate Slice-8 listener; this
//! slice defines the API identity central pins by and the token/credential exchange.

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, Uri};
use axum::routing::post;
use axum::{Json, Router};
use rustls::pki_types::ServerName;
use rustls::pki_types::{pem::PemObject, CertificateDer};
use serde::Serialize;
use shared::protocol::{
    fingerprint, sha256_hex, EnrollRequest, EnrollResponse, EnrollmentParams, PROTOCOL_VERSION,
};

use crate::auth::{hash_password, random_hex, random_id, AdminSession, ApiError, ClientContext};
use crate::observability::{correlation_id, log_validation_rejected};
use crate::store::{unix_now, Agent, EnrollmentToken, NodeKind};
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

/// Central's API identity — the certificate an enrolling agent pins.
/// Its [`Self::fingerprint`] (SHA-256 hex) is what the install command carries. In
/// production it should be the DER end-entity certificate presented by the HTTPS
/// API origin that serves `/api/enroll`, not the separate tunnel listener.
pub struct CentralIdentity {
    material: Vec<u8>,
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
                    Ok(material) => return Self { material },
                    Err(error) => tracing::warn!(
                        %error,
                        "{CERT_ENV} could not be loaded; falling back to {ID_ENV} or ephemeral identity"
                    ),
                }
            }
        }
        match std::env::var(ID_ENV) {
            Ok(value) if !value.is_empty() => Self {
                material: value.into_bytes(),
            },
            _ => {
                let material = random_hex(SECRET_BYTES).into_bytes();
                tracing::warn!(
                    "{CERT_ENV}/{ID_ENV} unset — using an ephemeral central API identity; install commands are \
                     invalidated on restart. Set {ID_ENV} for a stable central fingerprint."
                );
                Self { material }
            }
        }
    }

    pub fn from_material(material: Vec<u8>) -> Self {
        Self { material }
    }

    /// The SHA-256 fingerprint the install command embeds and the agent pins.
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.material)
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
    pub tunnel_url: Arc<str>,
    pub identity: Arc<CentralIdentity>,
    pub agent_url: Arc<str>,
    pub agent_sha256: Arc<str>,
    pub agent_install_script_url: Arc<str>,
    pub agent_install_script_sha256: Arc<str>,
}

impl EnrollConfig {
    pub fn from_env() -> Self {
        let central_url =
            std::env::var(URL_ENV).unwrap_or_else(|_| DEFAULT_CENTRAL_URL.to_string());
        let tunnel_url =
            std::env::var(TUNNEL_URL_ENV).unwrap_or_else(|_| DEFAULT_TUNNEL_URL.to_string());
        let agent_url = std::env::var(AGENT_URL_ENV).unwrap_or_default();
        let agent_sha256 = std::env::var(AGENT_SHA_ENV).unwrap_or_default();
        let agent_install_script_url =
            std::env::var(AGENT_INSTALL_SCRIPT_URL_ENV).unwrap_or_default();
        let agent_install_script_sha256 =
            std::env::var(AGENT_INSTALL_SCRIPT_SHA_ENV).unwrap_or_default();
        Self {
            central_url: Arc::from(central_url.as_str()),
            tunnel_url: Arc::from(tunnel_url.as_str()),
            identity: Arc::new(CentralIdentity::from_env_or_generate()),
            agent_url: Arc::from(agent_url.as_str()),
            agent_sha256: Arc::from(agent_sha256.as_str()),
            agent_install_script_url: Arc::from(agent_install_script_url.as_str()),
            agent_install_script_sha256: Arc::from(agent_install_script_sha256.as_str()),
        }
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
        Self {
            central_url: Arc::from(central_url),
            tunnel_url: Arc::from("https://central.test:8443"),
            identity: Arc::new(CentralIdentity::from_material(identity_material)),
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
}

/// The agent-facing enroll endpoint (token-gated, cleartext-refused) plus the
/// admin-facing ticket endpoint (session-gated). Both merge into the setup-gated
/// api router in `lib.rs`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/locations/{id}/enroll", post(create_enrollment))
        .route("/api/enroll", post(enroll_agent))
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
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<EnrollmentTicket>, ApiError> {
    let correlation_id = correlation_id(&headers);
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
    if !is_https_origin(&state.enroll.tunnel_url) {
        log_validation_rejected(&correlation_id, "agent.enroll", "invalid_tunnel_url");
        return Err(ApiError::Validation(
            "LG_TUNNEL_URL must be a plain https tunnel origin with a valid host and optional non-zero port.".to_string(),
        ));
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
    state.store.put_enrollment_token(&token)?;

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

fn is_https_url(value: &str) -> bool {
    let Ok(uri) = value.parse::<Uri>() else {
        return false;
    };
    if uri.scheme_str() != Some("https") {
        return false;
    }
    let Some(authority) = uri.authority() else {
        return false;
    };
    let host = authority.host();
    if host.is_empty() {
        return false;
    }
    if host.starts_with('[') || host.ends_with(']') {
        let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) else {
            return false;
        };
        if inner.parse::<IpAddr>().is_err() {
            return false;
        }
    }

    let host_port = authority.as_str().rsplit('@').next().unwrap_or_default();
    let port_suffix = &host_port[host.len()..];
    if port_suffix.starts_with(':') && authority.port_u16().is_none() {
        return false;
    }
    port_suffix.is_empty() || port_suffix.starts_with(':')
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
    // Atomic single-use: the consume decides the winner. A lost race consumes
    // nothing and issues nothing.
    if !state.store.consume_enrollment_token(&token.id, now)? {
        tracing::warn!(
            event = "agent.enroll",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason = "token_race_lost",
            "agent enrollment rejected"
        );
        return Err(ApiError::Unauthorized);
    }

    // Issue the long-lived per-agent credential: 256-bit CSPRNG, Argon2id-hashed at
    // rest (the Slice-2 hashing seam). The cleartext is returned once, here.
    let credential = random_hex(SECRET_BYTES);
    let credential_hash = hash_password(&credential)?;
    let agent = Agent {
        id: random_id(),
        location_id: token.location_id,
        credential_hash,
        enrolled_at: now,
        last_seen: None,
        revoked: false,
    };
    state.store.put_agent(&agent)?;
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
    use super::DEFAULT_CENTRAL_URL;

    #[test]
    fn default_enrollment_url_is_the_api_origin_not_the_tunnel_listener() {
        assert_eq!(DEFAULT_CENTRAL_URL, "https://localhost");
        assert_ne!(DEFAULT_CENTRAL_URL, "https://localhost:8443");
    }
}
