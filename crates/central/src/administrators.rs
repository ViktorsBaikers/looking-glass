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
use crate::store::{unix_now, Administrator, AdministratorStatus};
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
    let target = state
        .store
        .get_administrator(&id)?
        .ok_or(ApiError::NotFound)?;
    removal_guard(
        &correlation_id,
        &admin.admin_id,
        &target,
        &state.store.list_administrators()?,
    )?;
    state.store.delete_administrator(&id)?;
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

/// The guard rails of spec #1: a peer can never remove themselves, and the last
/// active administrator can never be removed — the site cannot be locked out.
/// (Over HTTP the self check fires first for the sole-active-admin self-delete;
/// the last-active check is the defensive backstop for any caller whose own row
/// is gone or inactive but whose session record briefly survived.)
fn removal_guard(
    correlation_id: &str,
    caller_id: &str,
    target: &Administrator,
    administrators: &[Administrator],
) -> Result<(), ApiError> {
    if target.id == caller_id {
        log_validation_rejected(correlation_id, "admin.administrators", "cannot_remove_self");
        return Err(ApiError::Coded(
            StatusCode::CONFLICT,
            "cannot_remove_self",
            "You cannot remove your own account.",
        ));
    }
    if target.status == AdministratorStatus::Active
        && administrators
            .iter()
            .filter(|admin| admin.status == AdministratorStatus::Active)
            .count()
            <= 1
    {
        log_validation_rejected(
            correlation_id,
            "admin.administrators",
            "last_active_administrator",
        );
        return Err(ApiError::Coded(
            StatusCode::CONFLICT,
            "last_active_administrator",
            "The last active administrator cannot be removed.",
        ));
    }
    Ok(())
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
    headers: HeaderMap,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let correlation_id = correlation_id(&headers);
    if let Err(report) = body.validate() {
        log_validation_rejected(&correlation_id, "auth.password", "invalid_payload");
        return Err(ApiError::Validation(first_message(&report)));
    }
    let mut row = state
        .store
        .get_administrator(&admin.admin_id)?
        .ok_or(ApiError::Unauthorized)?;
    let verified = row
        .password_hash
        .as_deref()
        .is_some_and(|hash| verify_password(&body.current_password, hash));
    if !verified {
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
    }
    row.password_hash = Some(hash_password(&body.new_password)?);
    state.store.put_administrator(&row)?;
    // A leaked session must not survive a password change (spec #1): every other
    // session of the caller is deleted; the current one stays signed in.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn admin(id: &str, status: AdministratorStatus) -> Administrator {
        Administrator {
            id: id.to_string(),
            username: id.to_string(),
            password_hash: None,
            status,
            created_at: 0,
            activation_token_hash: None,
            activation_expires_at: None,
        }
    }

    fn guard_code(result: Result<(), ApiError>) -> Option<&'static str> {
        match result {
            Ok(()) => None,
            Err(ApiError::Coded(_, code, _)) => Some(code),
            Err(_) => Some("unexpected"),
        }
    }

    // The removal guard rails (spec #1) as pure semantics: self is always
    // refused; the last active administrator is refused for any other caller;
    // an active peer may be removed while another active peer remains; a
    // pending peer (which holds no session) may always be removed.
    #[test]
    fn removal_guards_refuse_self_and_last_active() {
        let alice = admin("alice", AdministratorStatus::Active);
        let bob = admin("bob", AdministratorStatus::Active);

        assert_eq!(
            guard_code(removal_guard(
                "cid",
                "alice",
                &alice,
                &[alice.clone(), bob.clone()]
            )),
            Some("cannot_remove_self")
        );
        assert_eq!(
            guard_code(removal_guard(
                "cid",
                "ghost",
                &alice,
                std::slice::from_ref(&alice)
            )),
            Some("last_active_administrator"),
            "the last active administrator cannot be removed by anyone"
        );
        assert_eq!(
            guard_code(removal_guard(
                "cid",
                "alice",
                &bob.clone(),
                &[alice.clone(), bob]
            )),
            None,
            "an active peer may remove another active peer"
        );
        let pending = admin("cara", AdministratorStatus::Pending);
        assert_eq!(
            guard_code(removal_guard(
                "cid",
                "alice",
                &pending.clone(),
                &[alice, pending]
            )),
            None,
            "a pending peer may always be removed"
        );
    }
}
