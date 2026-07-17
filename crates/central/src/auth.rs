//! Admin authentication: Argon2id hashing, the fail-closed session extractor,
//! and the login/logout handlers. The admin panel controls what commands the
//! box runs, so every gate here denies by default.

use std::net::{IpAddr, SocketAddr};
use std::sync::LazyLock;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use garde::Validate;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_sessions::Session;

use crate::observability::{correlation_id, log_validation_rejected};
use crate::store::{unix_now, StoreError};
use crate::AppState;

const SESSION_ADMIN_KEY: &str = "admin_id";
const SESSION_AUTH_AT_KEY: &str = "auth_at";
const ABSOLUTE_SESSION_CAP_SECS: u64 = 12 * 60 * 60;

/// Verified against on a username miss so a failed login costs the same work
/// whether or not the account exists — no timing oracle to enumerate usernames.
/// A failed init would silently re-open that oracle, so panic instead.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    hash_password("timing-equalization-target").expect("static dummy hash must init")
});

pub fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| ApiError::Internal)
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

pub(crate) fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buf);
    buf.iter()
        .fold(String::with_capacity(bytes * 2), |mut acc, b| {
            acc.push_str(&format!("{b:02x}"));
            acc
        })
}

pub(crate) fn random_id() -> String {
    random_hex(16)
}

/// The trusted-proxy-derived client identity plus whether the external leg was
/// TLS. Extraction never fails; an absent peer or untrusted forwarded data
/// simply yields `ip: None` / `secure: false`, which the handlers treat as
/// fail-closed.
pub struct ClientContext {
    pub ip: Option<IpAddr>,
    pub secure: bool,
}

impl FromRequestParts<AppState> for ClientContext {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|conn| conn.0.ip());
        Ok(Self {
            ip: state.transport.client_ip(peer, &parts.headers),
            secure: state.transport.tls_attested(peer, &parts.headers),
        })
    }
}

/// Proof of an authenticated admin — its successful extraction *is* the gate.
/// Missing session, absent admin id, or a session past the absolute cap all
/// reject with `Unauthorized`, so the admin surface fails closed (FR-006/AC4).
pub struct AdminSession;

impl FromRequestParts<AppState> for AdminSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::Unauthorized)?;

        let admin_id: Option<String> = session
            .get(SESSION_ADMIN_KEY)
            .await
            .map_err(|_| ApiError::Internal)?;
        if admin_id.is_none() {
            return Err(ApiError::Unauthorized);
        }

        let auth_at: u64 = session
            .get(SESSION_AUTH_AT_KEY)
            .await
            .map_err(|_| ApiError::Internal)?
            .unwrap_or(0);

        if unix_now().saturating_sub(auth_at) > ABSOLUTE_SESSION_CAP_SECS {
            session.delete().await.ok();
            return Err(ApiError::Unauthorized);
        }

        Ok(Self)
    }
}

#[derive(Deserialize, Validate)]
pub struct LoginRequest {
    #[garde(length(min = 1, max = 64))]
    pub username: String,
    #[garde(length(min = 1, max = 512))]
    pub password: String,
}

pub async fn login(
    State(state): State<AppState>,
    session: Session,
    ctx: ClientContext,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<StatusCode, ApiError> {
    let correlation_id = correlation_id(&headers);
    if !ctx.secure {
        tracing::warn!(
            event = "auth.login",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason = "insecure_transport",
            "admin login rejected"
        );
        return Err(ApiError::CleartextRefused);
    }
    let client = ctx.ip.ok_or(ApiError::CleartextRefused)?;
    if !state.login_limiter.allow(client) {
        tracing::warn!(
            event = "auth.login",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason = "rate_limited",
            "admin login rejected"
        );
        return Err(ApiError::RateLimited);
    }

    let request_valid = body.validate().is_ok();
    if !request_valid {
        log_validation_rejected(&correlation_id, "auth.login", "invalid_login_payload");
    }
    let credentials_ok = request_valid && {
        let admin = state.store.admin()?;
        match &admin {
            Some(admin) if admin.username == body.username => {
                verify_password(&body.password, &admin.password_hash)
            }
            _ => {
                verify_password(&body.password, &DUMMY_HASH);
                false
            }
        }
    };

    if !credentials_ok {
        tracing::warn!(
            event = "auth.login",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason = "invalid_credentials",
            "admin login rejected"
        );
        return Err(ApiError::InvalidCredentials);
    }

    let admin = state.store.admin()?.ok_or(ApiError::InvalidCredentials)?;
    // Rotate the session id across the auth boundary so a fixed pre-auth id
    // cannot be promoted to an authenticated one (session fixation).
    session.cycle_id().await.map_err(|_| ApiError::Internal)?;
    session
        .insert(SESSION_ADMIN_KEY, &admin.id)
        .await
        .map_err(|_| ApiError::Internal)?;
    session
        .insert(SESSION_AUTH_AT_KEY, unix_now())
        .await
        .map_err(|_| ApiError::Internal)?;
    state.login_limiter.clear(client);
    tracing::info!(
        event = "auth.login",
        correlation_id = %correlation_id,
        outcome = "success",
        admin_id = %admin.id,
        "admin authenticated"
    );
    Ok(StatusCode::NO_CONTENT)
}

pub async fn logout(session: Session, headers: HeaderMap) -> Result<StatusCode, ApiError> {
    let correlation_id = correlation_id(&headers);
    session.delete().await.map_err(|_| ApiError::Internal)?;
    tracing::info!(
        event = "auth.logout",
        correlation_id = %correlation_id,
        outcome = "success",
        "admin logged out"
    );
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub struct MeResponse {
    pub username: String,
}

pub async fn me(
    State(state): State<AppState>,
    _admin: AdminSession,
) -> Result<Json<MeResponse>, ApiError> {
    let admin = state.store.admin()?.ok_or(ApiError::Unauthorized)?;
    Ok(Json(MeResponse {
        username: admin.username,
    }))
}

#[derive(Debug)]
pub enum ApiError {
    SetupRequired,
    SetupTokenInvalid,
    AlreadyInstalled,
    InvalidCredentials,
    Unauthorized,
    CleartextRefused,
    RateLimited,
    Validation(String),
    NotFound,
    Internal,
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::AlreadyInstalled => ApiError::AlreadyInstalled,
            StoreError::Backend(_) => ApiError::Internal,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::SetupRequired => (
                StatusCode::FORBIDDEN,
                "setup_required",
                "First-run setup must be completed before this action.".to_string(),
            ),
            ApiError::SetupTokenInvalid => (
                StatusCode::FORBIDDEN,
                "invalid_setup_token",
                "A valid first-run setup token is required.".to_string(),
            ),
            ApiError::AlreadyInstalled => (
                StatusCode::CONFLICT,
                "already_installed",
                "Setup has already been completed.".to_string(),
            ),
            ApiError::InvalidCredentials => (
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "Invalid username or password.".to_string(),
            ),
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Authentication required.".to_string(),
            ),
            ApiError::CleartextRefused => (
                StatusCode::FORBIDDEN,
                "insecure_transport",
                "A secure (TLS) connection is required for this action.".to_string(),
            ),
            ApiError::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Too many attempts. Try again later.".to_string(),
            ),
            ApiError::Validation(message) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_input", message)
            }
            ApiError::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                "The requested item does not exist.".to_string(),
            ),
            ApiError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Something went wrong.".to_string(),
            ),
        };
        (status, Json(json!({ "error": code, "message": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{hash_password, verify_password};

    const PASSWORD: &str = "correct-horse-battery-staple";

    #[test]
    fn hash_is_salted_argon2id() {
        let hash = hash_password(PASSWORD).unwrap();
        assert!(hash.starts_with("$argon2id$"), "must use Argon2id: {hash}");
        // A random salt per hash means the same password yields distinct hashes.
        assert_ne!(hash, hash_password(PASSWORD).unwrap());
    }

    #[test]
    fn verify_accepts_correct_and_rejects_wrong_password() {
        let hash = hash_password(PASSWORD).unwrap();
        assert!(verify_password(PASSWORD, &hash));
        assert!(!verify_password("wrong-password", &hash));
    }

    #[test]
    fn verify_rejects_a_malformed_hash() {
        assert!(!verify_password(PASSWORD, "not-a-phc-string"));
    }
}
