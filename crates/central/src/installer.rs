//! First-run installer: the setup-status probe the SPA reads, the guarded
//! create-admin action, and the fail-closed gate that refuses every non-setup
//! API route until an admin exists.
//!
//! The app is a static SPA (`ssr=false`), so the server has no per-route render
//! decision to redirect on. The gate is therefore server-side API authorization
//! — a `403 setup_required` on any protected route pre-setup — and the SPA reads
//! `/api/setup/status` and client-routes to `/install` (drift.md Slice 2 binding
//! constraint; error-handling.md "fail closed").

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use garde::Validate;
use serde::{Deserialize, Serialize};

use crate::admin_api::AdminJson;
use crate::auth::{
    argon2_off_runtime, hash_password, random_hex, random_id, ApiError, ClientContext,
};
use crate::observability::{correlation_id, log_validation_rejected};
use crate::AppState;

const EXEMPT_PATHS: [&str; 2] = ["/api/setup", "/api/setup/status"];
const SETUP_TOKEN_BYTES: usize = 32;

/// A high-entropy bootstrap secret generated when no admin exists and printed
/// once to the server log. Requiring it on create-admin authenticates first-run
/// setup to whoever can read the container logs (the operator) rather than
/// whoever races to the URL first.
pub fn generate_setup_token() -> String {
    random_hex(SETUP_TOKEN_BYTES)
}

#[derive(Serialize)]
pub struct SetupStatus {
    pub installed: bool,
}

pub async fn setup_status(State(state): State<AppState>) -> Result<Json<SetupStatus>, ApiError> {
    Ok(Json(SetupStatus {
        installed: state.store.is_installed()?,
    }))
}

#[derive(Deserialize, Validate)]
pub struct SetupRequest {
    #[garde(length(min = 1, max = 128))]
    pub setup_token: String,
    #[garde(length(min = 1, max = 64))]
    pub username: String,
    #[garde(length(min = 12, max = 512))]
    pub password: String,
}

pub(crate) fn username_allowed(username: &str) -> bool {
    username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub async fn create_admin(
    State(state): State<AppState>,
    ctx: ClientContext,
    headers: HeaderMap,
    AdminJson(body): AdminJson<SetupRequest>,
) -> Result<StatusCode, ApiError> {
    let correlation_id = correlation_id(&headers);
    if !ctx.secure {
        tracing::warn!(
            event = "auth.setup",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason = "insecure_transport",
            "admin setup rejected"
        );
        return Err(ApiError::CleartextRefused);
    }
    // Fail closed: setup must be open (a token was generated at startup) and the
    // presented token must match. A closed setup or a wrong token is refused
    // before any account is written.
    match &state.setup_token {
        Some(expected) if constant_time_eq(body.setup_token.as_bytes(), expected.as_bytes()) => {}
        _ => {
            tracing::warn!(
                event = "auth.setup",
                correlation_id = %correlation_id,
                outcome = "rejected",
                reason = "invalid_setup_token",
                "admin setup rejected"
            );
            return Err(ApiError::SetupTokenInvalid);
        }
    }
    if let Err(report) = body.validate() {
        log_validation_rejected(&correlation_id, "auth.setup", "invalid_setup_payload");
        return Err(ApiError::Validation(first_message(&report)));
    }
    if !username_allowed(&body.username) {
        log_validation_rejected(&correlation_id, "auth.setup", "invalid_username");
        return Err(ApiError::Validation(
            "Username may contain only letters, digits, and . _ -".to_string(),
        ));
    }

    let password = body.password;
    let password_hash = argon2_off_runtime(move || hash_password(&password))
        .await
        .ok_or(ApiError::Internal)??;
    let admin =
        state
            .store
            .create_first_administrator(random_id(), body.username, password_hash)?;
    tracing::info!(
        event = "auth.setup",
        correlation_id = %correlation_id,
        outcome = "created",
        admin_id = %admin.id,
        "admin account created"
    );
    Ok(StatusCode::CREATED)
}

fn first_message(report: &garde::Report) -> String {
    report
        .iter()
        .next()
        .map(|(_, error)| error.to_string())
        .unwrap_or_else(|| "Invalid input.".to_string())
}

/// Refuse every non-setup route until setup completes. Applied to the feature
/// router only, so unmatched paths still fall through to the SPA shell.
pub async fn require_setup(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if is_exempt(path) {
        return next.run(request).await;
    }
    match state.store.is_installed() {
        Ok(true) => next.run(request).await,
        Ok(false) => ApiError::SetupRequired.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn is_exempt(path: &str) -> bool {
    EXEMPT_PATHS.contains(&path)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::sync::Arc;
    use std::time::Duration;

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};

    use super::{create_admin, SetupRequest};
    use crate::admin_api::AdminJson;
    use crate::auth::{random_id, ClientContext};
    use crate::{
        AppState, EnrollConfig, LoginLimiter, RunService, Store, TransportConfig, TunnelHub,
    };

    // F-184/C-041: the first administrator's password is hashed through
    // argon2_off_runtime, so setup cannot hold an async worker.
    #[tokio::test]
    async fn the_first_admin_hash_runs_off_the_async_workers() {
        let dir = std::env::temp_dir().join(format!(
            "lg-installer-unit-{}-{}",
            std::process::id(),
            random_id()
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let state = AppState {
            store: Store::open(dir.join("db.redb")).expect("open test store"),
            transport: TransportConfig::new([std::net::IpAddr::from(Ipv4Addr::LOCALHOST)]),
            login_limiter: Arc::new(LoginLimiter::default()),
            setup_token: Some(Arc::from("setup-token")),
            run: RunService::for_test(8, Duration::from_secs(30), 100),
            files_root: Arc::from(dir.as_path()),
            enroll: EnrollConfig::for_test("https://central.test", b"identity".to_vec()),
            tunnel_hub: TunnelHub::new(),
        };
        let ctx = ClientContext {
            ip: None,
            secure: true,
        };
        let body = SetupRequest {
            setup_token: "setup-token".to_string(),
            username: "alice".to_string(),
            password: "correct-horse-battery-staple".to_string(),
        };

        let created =
            create_admin(State(state.clone()), ctx, HeaderMap::new(), AdminJson(body)).await;
        assert_eq!(created.ok(), Some(StatusCode::CREATED));

        let admins = state.store.list_administrators().unwrap();
        let hash = admins[0].password_hash.as_deref().unwrap();
        assert_eq!(
            crate::auth::tests::off_runtime_count(hash),
            1,
            "the first admin's password must hash off the runtime"
        );
    }
}
