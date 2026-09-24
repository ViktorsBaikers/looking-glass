//! Equal-peer Administrators (ADR-0001, spec #1): any signed-in peer creates
//! Pending administrators with one-time 24-hour activation links, regenerates or
//! removes them, and changes their own password. Sessions are tied to an
//! administrator id; removing a peer purges that peer's sessions and a password
//! change purges the caller's OTHER sessions. The two `/api/activate/{token}`
//! routes are public — the token itself is the authentication (256-bit, stored
//! only as a SHA-256 hash, single-use) — and they refuse cleartext transport
//! like every credential-entry surface.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use garde::Validate;
use serde::{Deserialize, Serialize};
use shared::protocol::sha256_hex;
use tower_sessions::Session;

use crate::admin_api::first_message;
use crate::auth::{
    hash_password, random_hex, random_id, verify_password, AdminSession, ApiError, ClientContext,
};
use crate::installer::username_allowed;
use crate::observability::{correlation_id, log_validation_rejected};
use crate::session::RedbSessionStore;
use crate::store::{unix_now, Administrator, AdministratorStatus, RemoveAdministratorError};
use crate::AppState;

/// Activation links live for 24 hours (spec #1) — long enough to hand to a peer,
/// short enough to bound the window of a lost link.
const ACTIVATION_TTL_SECS: u64 = 24 * 60 * 60;
/// 256 bits of CSPRNG entropy for the activation token, as for enrollment tokens.
const ACTIVATION_TOKEN_BYTES: usize = 32;

/// The admin peer routes, each behind [`AdminSession`]. Mounted inside the
/// setup-gated, session-layered admin router in `lib.rs`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/admin/administrators",
            get(list_administrators).post(create_administrator),
        )
        .route(
            "/api/admin/administrators/{id}/activation",
            post(regenerate_activation),
        )
        .route(
            "/api/admin/administrators/{id}",
            delete(remove_administrator),
        )
        .route("/api/admin/me/password", put(change_password))
}

/// The public activation routes (token-gated, cleartext-refused). Mounted into
/// the setup-gated api router without the session layer — activation precedes
/// credentials, it never requires them.
pub fn activation_routes(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/activate/{token}",
            get(activation_username).post(activate),
        )
        .with_state(state)
}

/// A peer as the spec lists it — never the password or the token hash.
#[derive(Serialize)]
struct AdministratorPayload {
    id: String,
    username: String,
    status: AdministratorStatus,
    created_at: u64,
    activation_expires_at: Option<u64>,
}

impl AdministratorPayload {
    fn from(admin: &Administrator) -> Self {
        Self {
            id: admin.id.clone(),
            username: admin.username.clone(),
            status: admin.status,
            created_at: admin.created_at,
            activation_expires_at: admin.activation_expires_at,
        }
    }
}

/// The one-time create/regenerate response (spec #1): the absolute URL to hand
/// the peer, shown once and never retrievable again — regenerate replaces it.
#[derive(Serialize)]
struct ActivationLink {
    administrator: AdministratorPayload,
    activation_url: String,
    expires_at: u64,
}

/// `{origin}/activate/{token}` — the origin derived exactly as the enrollment
/// command derives it (the configured central API origin, `LG_CENTRAL_URL`).
fn activation_url(state: &AppState, raw_token: &str) -> String {
    format!(
        "{}/activate/{raw_token}",
        state.enroll.central_url.trim_end_matches('/')
    )
}

fn activation_link(
    state: &AppState,
    admin: &Administrator,
    raw_token: &str,
    expires_at: u64,
) -> ActivationLink {
    ActivationLink {
        administrator: AdministratorPayload::from(admin),
        activation_url: activation_url(state, raw_token),
        expires_at,
    }
}

/// Mint a fresh raw token + its absolute expiry. The raw token is returned to
/// the caller exactly once and never logged or stored (only its hash is).
fn mint_activation() -> (String, String, u64) {
    let raw_token = random_hex(ACTIVATION_TOKEN_BYTES);
    let token_hash = sha256_hex(raw_token.as_bytes());
    let expires_at = unix_now() + ACTIVATION_TTL_SECS;
    (raw_token, token_hash, expires_at)
}

async fn list_administrators(
    State(state): State<AppState>,
    _admin: AdminSession,
) -> Result<Json<Vec<AdministratorPayload>>, ApiError> {
    let admins = state
        .store
        .list_administrators()?
        .iter()
        .map(AdministratorPayload::from)
        .collect();
    Ok(Json(admins))
}

#[derive(Deserialize, Validate)]
struct CreateAdministratorRequest {
    #[garde(length(min = 1, max = 64))]
    username: String,
}

async fn create_administrator(
    State(state): State<AppState>,
    _admin: AdminSession,
    headers: HeaderMap,
    Json(body): Json<CreateAdministratorRequest>,
) -> Result<(StatusCode, Json<ActivationLink>), ApiError> {
    let correlation_id = correlation_id(&headers);
    if let Err(report) = body.validate() {
        log_validation_rejected(&correlation_id, "admin.administrators", "invalid_payload");
        return Err(ApiError::Validation(first_message(&report)));
    }
    // The username rules are identical to install (spec #1).
    if !username_allowed(&body.username) {
        log_validation_rejected(&correlation_id, "admin.administrators", "invalid_username");
        return Err(ApiError::Validation(
            "Username may contain only letters, digits, and . _ -".to_string(),
        ));
    }

    let (raw_token, token_hash, expires_at) = mint_activation();
    let now = unix_now();
    let admin = Administrator {
        id: random_id(),
        username: body.username,
        password_hash: None,
        status: AdministratorStatus::Pending,
        created_at: now,
        activation_token_hash: Some(token_hash),
        activation_expires_at: Some(expires_at),
        session_generation: 0,
    };
    // The store enforces case-insensitive username uniqueness atomically; a
    // clash surfaces as StoreError::UsernameTaken → 409 username_taken.
    let admin = state.store.create_pending_administrator(admin)?;
    tracing::info!(
        event = "admin.administrators",
        correlation_id = %correlation_id,
        outcome = "pending_created",
        admin_id = %admin.id,
        "pending administrator created"
    );
    Ok((
        StatusCode::CREATED,
        Json(activation_link(&state, &admin, &raw_token, expires_at)),
    ))
}

async fn regenerate_activation(
    State(state): State<AppState>,
    _admin: AdminSession,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ActivationLink>, ApiError> {
    let correlation_id = correlation_id(&headers);
    let existing = state
        .store
        .get_administrator(&id)?
        .ok_or(ApiError::NotFound)?;
    if existing.status != AdministratorStatus::Pending {
        return Err(not_pending(&correlation_id));
    }
    let (raw_token, token_hash, expires_at) = mint_activation();
    // The store re-checks pending inside its write transaction: a regeneration
    // racing an activation must never hand out a fresh link for an account that
    // already has a password. Replacing the stored hash is what invalidates the
    // previous link.
    let Some(updated) = state
        .store
        .regenerate_activation(&id, token_hash, expires_at)?
    else {
        return Err(not_pending(&correlation_id));
    };
    tracing::info!(
        event = "admin.administrators",
        correlation_id = %correlation_id,
        outcome = "activation_regenerated",
        admin_id = %id,
        "activation link regenerated"
    );
    Ok(Json(activation_link(
        &state, &updated, &raw_token, expires_at,
    )))
}

fn not_pending(correlation_id: &str) -> ApiError {
    log_validation_rejected(correlation_id, "admin.administrators", "not_pending");
    ApiError::Coded(
        StatusCode::CONFLICT,
        "not_pending",
        "Only a pending administrator has an activation link to regenerate.",
    )
}

async fn remove_administrator(
    State(state): State<AppState>,
    admin: AdminSession,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let correlation_id = correlation_id(&headers);
    // The guard rails (spec #1: never self, never the last active peer) run
    // inside the same store write transaction as the delete, so two peers
    // concurrently removing each other can never both leave zero active
    // administrators behind.
    if let Err(reason) = state.store.remove_administrator(&admin.admin_id, &id) {
        return Err(match reason {
            RemoveAdministratorError::NotFound => ApiError::NotFound,
            RemoveAdministratorError::Backend(error) => error.into(),
            RemoveAdministratorError::CannotRemoveSelf => {
                log_validation_rejected(
                    &correlation_id,
                    "admin.administrators",
                    "cannot_remove_self",
                );
                ApiError::Coded(
                    StatusCode::CONFLICT,
                    "cannot_remove_self",
                    "You cannot remove your own account.",
                )
            }
            RemoveAdministratorError::LastActive => {
                log_validation_rejected(
                    &correlation_id,
                    "admin.administrators",
                    "last_active_administrator",
                );
                ApiError::Coded(
                    StatusCode::CONFLICT,
                    "last_active_administrator",
                    "The last active administrator cannot be removed.",
                )
            }
        });
    }
    // The removal only takes effect once the peer's session records are gone —
    // their next request is refused (ADR-0001: revoked access is truly revoked).
    RedbSessionStore::new(&state.store)
        .delete_for_admin(&id, None)
        .await
        .map_err(|_| ApiError::Internal)?;
    tracing::info!(
        event = "admin.administrators",
        correlation_id = %correlation_id,
        outcome = "removed",
        admin_id = %id,
        "administrator removed"
    );
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Validate)]
struct ChangePasswordRequest {
    #[garde(length(min = 1, max = 512))]
    current_password: String,
    // The password rule is identical to install (spec #1).
    #[garde(length(min = 12, max = 512))]
    new_password: String,
}

async fn change_password(
    State(state): State<AppState>,
    admin: AdminSession,
    session: Session,
    ctx: ClientContext,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let correlation_id = correlation_id(&headers);
    // A password change carries the current AND the new password, so it refuses
    // cleartext transport exactly like every other credential-entry surface.
    if !ctx.secure {
        tracing::warn!(
            event = "auth.password",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason = "insecure_transport",
            "password change rejected"
        );
        return Err(ApiError::CleartextRefused);
    }
    if let Err(report) = body.validate() {
        log_validation_rejected(&correlation_id, "auth.password", "invalid_payload");
        return Err(ApiError::Validation(first_message(&report)));
    }
    let row = state
        .store
        .get_administrator(&admin.admin_id)?
        .ok_or(ApiError::Unauthorized)?;
    let verified_hash = row
        .password_hash
        .as_deref()
        .filter(|hash| verify_password(&body.current_password, hash));
    let Some(verified_hash) = verified_hash else {
        tracing::warn!(
            event = "auth.password",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason = "invalid_credentials",
            "password change rejected"
        );
        return Err(ApiError::Coded(
            StatusCode::FORBIDDEN,
            "invalid_credentials",
            "The current password is incorrect.",
        ));
    };
    // The conditional store write re-checks existence, status, and the exact
    // verified hash inside its transaction — a removal or second rotation
    // racing the Argon2 work refuses instead of resurrecting the row, and the
    // same transaction bumps the credential generation that revokes every
    // session not stamped with the new value.
    let Some(generation) = state.store.rotate_password(
        &admin.admin_id,
        verified_hash,
        hash_password(&body.new_password)?,
    )?
    else {
        return Err(ApiError::Unauthorized);
    };
    // The caller's current session moves to the new generation and stays
    // signed in; every OTHER session is deleted — and a stale record an
    // overlapping request re-saves afterwards is refused by the extractor's
    // generation check (spec #1: a leaked session must not survive).
    session
        .insert(crate::auth::SESSION_GENERATION_KEY, generation)
        .await
        .map_err(|_| ApiError::Internal)?;
    RedbSessionStore::new(&state.store)
        .delete_for_admin(&row.id, Some(session.id().ok_or(ApiError::Internal)?))
        .await
        .map_err(|_| ApiError::Internal)?;
    tracing::info!(
        event = "auth.password",
        correlation_id = %correlation_id,
        outcome = "changed",
        admin_id = %row.id,
        "password changed"
    );
    Ok(StatusCode::NO_CONTENT)
}

// ----- Public activation -------------------------------------------------------

#[derive(Serialize)]
struct ActivationUsername {
    username: String,
}

#[derive(Deserialize, Validate)]
struct ActivateRequest {
    // The password rule is identical to install (spec #1).
    #[garde(length(min = 12, max = 512))]
    password: String,
}

/// The pending administrator an unexpired token points at — `None` for an
/// unknown, already-used, or expired token. Every miss answers identically
/// (410 `activation_invalid`), so there is no oracle telling the states apart.
fn pending_for_token(state: &AppState, token: &str) -> Result<Option<Administrator>, ApiError> {
    let now = unix_now();
    Ok(state
        .store
        .find_pending_by_activation_hash(&sha256_hex(token.as_bytes()))?
        .filter(|admin| {
            admin
                .activation_expires_at
                .is_some_and(|expiry| now <= expiry)
        }))
}

fn activation_invalid() -> ApiError {
    ApiError::Coded(
        StatusCode::GONE,
        "activation_invalid",
        "This activation link is no longer valid — ask a peer for a new one.",
    )
}

async fn activation_username(
    State(state): State<AppState>,
    ctx: ClientContext,
    Path(token): Path<String>,
) -> Result<Json<ActivationUsername>, ApiError> {
    if !ctx.secure {
        return Err(ApiError::CleartextRefused);
    }
    let admin = pending_for_token(&state, &token)?.ok_or_else(activation_invalid)?;
    Ok(Json(ActivationUsername {
        username: admin.username,
    }))
}

async fn activate(
    State(state): State<AppState>,
    ctx: ClientContext,
    headers: HeaderMap,
    Path(token): Path<String>,
    Json(body): Json<ActivateRequest>,
) -> Result<StatusCode, ApiError> {
    if !ctx.secure {
        return Err(ApiError::CleartextRefused);
    }
    let correlation_id = correlation_id(&headers);
    let admin = pending_for_token(&state, &token)?.ok_or_else(activation_invalid)?;
    // A refused password must not burn the single-use token — validate before
    // consuming so the peer can retry through the same link.
    if let Err(report) = body.validate() {
        log_validation_rejected(&correlation_id, "auth.activate", "invalid_payload");
        return Err(ApiError::Validation(first_message(&report)));
    }
    let password_hash = hash_password(&body.password)?;
    // Atomic single use: the store transaction decides the winner; a lost race
    // (or a link used between the lookup and here) is a plain 410.
    if !state.store.activate_administrator(
        &admin.id,
        &sha256_hex(token.as_bytes()),
        password_hash,
        unix_now(),
    )? {
        return Err(activation_invalid());
    }
    tracing::info!(
        event = "auth.activate",
        correlation_id = %correlation_id,
        outcome = "activated",
        admin_id = %admin.id,
        "administrator activated"
    );
    Ok(StatusCode::NO_CONTENT)
}
