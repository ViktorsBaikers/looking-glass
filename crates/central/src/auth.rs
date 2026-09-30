//! Admin authentication: Argon2id hashing, the fail-closed session extractor,
//! and the login/logout handlers. The admin panel controls what commands the
//! box runs, so every gate here denies by default.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, LazyLock};

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
use tokio::sync::Semaphore;
use tower_sessions::Session;

use crate::admin_api::AdminJson;
use crate::observability::{correlation_id, log_validation_rejected};
use crate::store::{unix_now, AdministratorStatus, StoreError};
use crate::AppState;

pub(crate) const SESSION_ADMIN_KEY: &str = "admin_id";
const SESSION_AUTH_AT_KEY: &str = "auth_at";
/// The credential epoch the session was minted under — stamped at login and
/// compared against the administrator row on every request, so a session
/// record resurrected after a password rotation is refused even though the
/// row is still active. Sessions predating the key read as generation 0,
/// matching rows migrated with the serde default.
pub(crate) const SESSION_GENERATION_KEY: &str = "session_generation";
const ABSOLUTE_SESSION_CAP_SECS: u64 = 12 * 60 * 60;

/// Verified against on a username miss so a failed login costs the same work
/// whether or not the account exists — no timing oracle to enumerate usernames.
/// A failed init would silently re-open that oracle, so panic instead.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    hash_password("timing-equalization-target").expect("static dummy hash must init")
});

/// One Argon2 job per core: each holds ~19 MiB and a core for the whole hash.
static ARGON2_PERMITS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    Arc::new(Semaphore::new(cores))
});

/// Agent tunnel credential checks get their own small pool, so a login flood
/// queued on [`ARGON2_PERMITS`] cannot hold a reconnecting agent past its
/// pre-auth deadline.
static TUNNEL_ARGON2_PERMITS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(2)));

/// Runs Argon2 work on the blocking pool, so a login or agent-handshake flood
/// queues here instead of parking the async workers every other request needs.
/// The permit travels with the job: a dropped request keeps it until the hash
/// ends. `None` if the job panicked.
pub(crate) async fn argon2_off_runtime<R: Send + 'static>(
    job: impl FnOnce() -> R + Send + 'static,
) -> Option<R> {
    argon2_off_runtime_in(&ARGON2_PERMITS, job).await
}

/// [`argon2_off_runtime`] for agent tunnel credential checks.
pub(crate) async fn tunnel_argon2_off_runtime<R: Send + 'static>(
    job: impl FnOnce() -> R + Send + 'static,
) -> Option<R> {
    argon2_off_runtime_in(&TUNNEL_ARGON2_PERMITS, job).await
}

async fn argon2_off_runtime_in<R: Send + 'static>(
    permits: &Arc<Semaphore>,
    job: impl FnOnce() -> R + Send + 'static,
) -> Option<R> {
    let permit = Arc::clone(permits).acquire_owned().await.ok()?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        #[cfg(test)]
        let _off_runtime = tests::OffRuntime::enter();
        job()
    })
    .await
    .ok()
}

pub fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| ApiError::Internal)?;
    #[cfg(test)]
    tests::OffRuntime::record(&hash);
    Ok(hash)
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    #[cfg(test)]
    if password == tests::GATED_PASSWORD {
        tests::HASH_GATE.wait();
        return false;
    }
    #[cfg(test)]
    tests::OffRuntime::record(hash);
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

/// Marks a request that reached central over its own TLS listener (the tunnel
/// port), so the transport is secure with no proxy attestation. Only that
/// listener inserts it; nothing a client sends to the web port can.
#[derive(Clone, Copy)]
pub(crate) struct DirectTls;

/// The trusted-proxy-derived client identity plus whether the external leg was
/// TLS. Extraction never fails; an absent peer or untrusted forwarded data
/// simply yields `ip: None` / `secure: false`, which the handlers treat as
/// fail-closed. A [`DirectTls`] request is secure and its peer is the client.
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
        if parts.extensions.get::<DirectTls>().is_some() {
            return Ok(Self {
                ip: peer,
                secure: true,
            });
        }
        Ok(Self {
            ip: state.transport.client_ip(peer, &parts.headers),
            secure: state.transport.tls_attested(peer, &parts.headers),
        })
    }
}

/// Proof of an authenticated administrator — its successful extraction *is* the
/// gate. Missing session, absent administrator id, an administrator row that no
/// longer exists or is not active, or a session past the absolute cap all reject
/// with `Unauthorized`, so the admin surface fails closed (FR-006/AC4). Carries
/// the session's administrator id for the handlers that act as "me" (ADR-0001).
pub struct AdminSession {
    pub admin_id: String,
}

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
        let Some(admin_id) = admin_id else {
            return Err(ApiError::Unauthorized);
        };

        let auth_at: u64 = session
            .get(SESSION_AUTH_AT_KEY)
            .await
            .map_err(|_| ApiError::Internal)?
            .unwrap_or(0);

        if unix_now().saturating_sub(auth_at) > ABSOLUTE_SESSION_CAP_SECS {
            session.delete().await.ok();
            return Err(ApiError::Unauthorized);
        }

        // The administrator must still exist, still be active, and still carry
        // the credential generation this session was minted under. Removal
        // purges the peer's session records and a password change purges the
        // caller's other sessions and bumps the generation — but the session
        // middleware re-saves every in-flight request's record, so a stale
        // record can be resurrected after either purge. This check is what
        // makes revocation fail closed on the next request regardless.
        let generation: u64 = session
            .get(SESSION_GENERATION_KEY)
            .await
            .map_err(|_| ApiError::Internal)?
            .unwrap_or(0);
        let current = state
            .store
            .get_administrator(&admin_id)?
            .is_some_and(|admin| {
                admin.status == AdministratorStatus::Active
                    && admin.session_generation == generation
            });
        if !current {
            // Best-effort hygiene: the refusal below is the gate, and it fires
            // again on every request even if this delete fails.
            session.delete().await.ok();
            return Err(ApiError::Unauthorized);
        }

        Ok(Self { admin_id })
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
    AdminJson(body): AdminJson<LoginRequest>,
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
    // The session is stamped from the same row read the password verified
    // against: a second read could see a rotation that committed during the
    // Argon2 work and stamp its new generation onto an old-password login.
    let verified = if request_valid {
        let admin = state
            .store
            .find_active_administrator_by_username(&body.username)?;
        let hash = admin.as_ref().map(|admin| admin.password_hash.clone());
        let password = body.password;
        let matched = argon2_off_runtime(move || match hash {
            Some(hash) => hash.is_some_and(|hash| verify_password(&password, &hash)),
            None => {
                verify_password(&password, &DUMMY_HASH);
                false
            }
        })
        .await
        .unwrap_or(false);
        admin.filter(|_| matched)
    } else {
        None
    };

    let Some(admin) = verified else {
        tracing::warn!(
            event = "auth.login",
            correlation_id = %correlation_id,
            outcome = "rejected",
            reason = "invalid_credentials",
            "admin login rejected"
        );
        return Err(ApiError::InvalidCredentials);
    };
    #[cfg(test)]
    tests::after_verify();

    // Rotate the session id across the auth boundary so a fixed pre-auth id
    // cannot be promoted to an authenticated one (session fixation).
    session.cycle_id().await.map_err(|error| {
        tracing::error!(
            event = "auth.login",
            correlation_id = %correlation_id,
            %error,
            "session id rotation failed"
        );
        ApiError::Internal
    })?;
    session
        .insert(SESSION_ADMIN_KEY, &admin.id)
        .await
        .map_err(|_| ApiError::Internal)?;
    session
        .insert(SESSION_AUTH_AT_KEY, unix_now())
        .await
        .map_err(|_| ApiError::Internal)?;
    session
        .insert(SESSION_GENERATION_KEY, admin.session_generation)
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
    // `flush` (not `delete`) also empties the in-memory session, so the layer
    // clears the cookie instead of saving a fresh empty record.
    session.flush().await.map_err(|error| {
        tracing::error!(
            event = "auth.logout",
            correlation_id = %correlation_id,
            %error,
            "session flush failed"
        );
        ApiError::Internal
    })?;
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
    pub id: String,
    pub username: String,
}

pub async fn me(
    State(state): State<AppState>,
    admin: AdminSession,
) -> Result<Json<MeResponse>, ApiError> {
    let row = state
        .store
        .get_administrator(&admin.admin_id)?
        .ok_or(ApiError::Unauthorized)?;
    Ok(Json(MeResponse {
        id: row.id,
        username: row.username,
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
    /// A spec-named error: explicit HTTP status and machine-readable code (the
    /// Administrators guard rails, ASN validation, activation lifecycle).
    Coded(StatusCode, &'static str, &'static str),
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::AlreadyInstalled => ApiError::AlreadyInstalled,
            StoreError::UsernameTaken => ApiError::Coded(
                StatusCode::CONFLICT,
                "username_taken",
                "That username is already taken.",
            ),
            StoreError::Backend(cause) => {
                // The client only sees a generic 500; the operator needs the cause.
                tracing::error!(error = %cause, "store operation failed");
                ApiError::Internal
            }
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
            ApiError::Coded(status, code, message) => (status, code, message.to_string()),
        };
        (status, Json(json!({ "error": code, "message": message }))).into_response()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::{mpsc, Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    use axum::body::Body;
    use axum::extract::{ConnectInfo, FromRequestParts, State};
    use axum::http::{HeaderMap, Request, StatusCode};
    use redb::{ReadableDatabase, ReadableTableMetadata};
    use shared::protocol::sha256_hex;
    use tower::ServiceExt;
    use tower_sessions::Session;

    use super::{
        hash_password, login, logout, verify_password, AdminSession, ApiError, ClientContext,
        LoginRequest, SESSION_ADMIN_KEY,
    };
    use crate::admin_api::AdminJson;
    use crate::session::RedbSessionStore;
    use crate::store::{Administrator, AdministratorStatus, SESSION};
    use crate::{
        AppState, EnrollConfig, LoginLimiter, RunService, Store, TransportConfig, TunnelHub,
    };

    pub(crate) const PASSWORD: &str = "correct-horse-battery-staple";

    thread_local! {
        static OFF_RUNTIME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    /// Every PHC string hashed or verified inside [`super::argon2_off_runtime`].
    static OFF_RUNTIME_HASHES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Marks the blocking-pool thread running an [`super::argon2_off_runtime`]
    /// job, so a test can tell which Argon2 work stayed off the async workers.
    pub(super) struct OffRuntime;

    impl OffRuntime {
        pub(super) fn enter() -> Self {
            OFF_RUNTIME.set(true);
            Self
        }

        pub(super) fn record(hash: &str) {
            if OFF_RUNTIME.get() {
                OFF_RUNTIME_HASHES.lock().unwrap().push(hash.to_string());
            }
        }
    }

    impl Drop for OffRuntime {
        fn drop(&mut self) {
            OFF_RUNTIME.set(false);
        }
    }

    /// How many Argon2 hashes or verifies of `hash` ran off the runtime.
    pub(crate) fn off_runtime_count(hash: &str) -> usize {
        OFF_RUNTIME_HASHES
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| *seen == hash)
            .count()
    }

    /// Runs inside `login` after the password verified and before the session
    /// is stamped — where a racing password rotation lands.
    static AFTER_VERIFY: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

    pub(super) fn after_verify() {
        if let Some(hook) = AFTER_VERIFY.lock().unwrap().take() {
            hook();
        }
    }

    /// A stand-in for a slow Argon2 run: verifying this password parks the
    /// calling thread until the test opens [`HASH_GATE`], then fails.
    pub(super) const GATED_PASSWORD: &str = "gated-hasher-password";
    pub(super) static HASH_GATE: Gate = Gate {
        open: Mutex::new(false),
        opened: Condvar::new(),
    };

    pub(super) struct Gate {
        open: Mutex<bool>,
        opened: Condvar,
    }

    impl Gate {
        pub(super) fn wait(&self) {
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.opened.wait(open).unwrap();
            }
        }

        fn open(&self) {
            *self.open.lock().unwrap() = true;
            self.opened.notify_all();
        }
    }

    const SETTLE: Duration = Duration::from_millis(500);
    const HEALTH_BOUND: Duration = Duration::from_secs(5);

    /// Holds redb's write lock from an OS thread while `load` runs, then
    /// reports whether `/health` answered within [`HEALTH_BOUND`]. `release`
    /// unblocks anything else the load waits on. Waits with std sleeps only: a
    /// runtime whose workers are all parked cannot fire tokio timers.
    async fn health_answers_under(
        state: &AppState,
        load: Vec<Request<Body>>,
        release: impl FnOnce(),
    ) -> (bool, Vec<StatusCode>) {
        let db = state.store.database();
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let txn = db.begin_write().unwrap();
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(txn);
        });
        held_rx.recv().unwrap();

        let app = crate::build(state.clone());
        let load: Vec<_> = load
            .into_iter()
            .map(|request| tokio::spawn(app.clone().oneshot(request)))
            .collect();
        std::thread::sleep(SETTLE);
        let health =
            tokio::spawn(app.oneshot(Request::get("/health").body(Body::empty()).unwrap()));
        let deadline = Instant::now() + HEALTH_BOUND;
        while !health.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let answered = health.is_finished();

        release();
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        let mut statuses = Vec::new();
        for task in load {
            statuses.push(task.await.unwrap().unwrap().status());
        }
        assert_eq!(health.await.unwrap().unwrap().status(), StatusCode::OK);
        (answered, statuses)
    }

    async fn login_cookie(state: &AppState) -> String {
        let login = crate::build(state.clone())
            .oneshot(proxied(
                "POST",
                "/api/auth/login",
                None,
                &format!(r#"{{"username":"alice","password":"{PASSWORD}"}}"#),
            ))
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::NO_CONTENT);
        login.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    // A flood of Argon2 jobs queues for the per-core permits; no more
    // than one job per core ever runs at once on the blocking pool.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn argon2_jobs_never_run_more_than_one_per_core() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let jobs: Vec<_> = (0..4 * cores)
            .map(|_| {
                let (running, peak) = (Arc::clone(&running), Arc::clone(&peak));
                tokio::spawn(super::argon2_off_runtime(move || {
                    peak.fetch_max(running.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(50));
                    running.fetch_sub(1, Ordering::SeqCst);
                }))
            })
            .collect();
        for job in jobs {
            assert!(job.await.unwrap().is_some(), "every queued job runs");
        }
        let peak = peak.load(Ordering::SeqCst);
        assert!(
            peak <= cores,
            "{peak} Argon2 jobs ran at once on {cores} cores"
        );
    }

    // Argon2 and session commits must not hold the async
    // workers, or one login flood plus a pending write stalls every endpoint.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn health_answers_during_a_login_flood_and_waiting_session_saves() {
        let state = test_state();
        install(&state);
        let cookie = login_cookie(&state).await;
        let mut load: Vec<_> = (0..8)
            .map(|n| {
                let mut request = proxied(
                    "POST",
                    "/api/auth/login",
                    None,
                    &format!(r#"{{"username":"alice","password":"{GATED_PASSWORD}"}}"#),
                );
                let client = format!("203.0.113.{}", 10 + n);
                request
                    .headers_mut()
                    .insert("x-forwarded-for", client.parse().unwrap());
                request
            })
            .collect();
        load.extend((0..2).map(|_| proxied("GET", "/api/admin/me", Some(&cookie), "")));

        let (answered, statuses) = health_answers_under(&state, load, || HASH_GATE.open()).await;
        assert!(answered, "/health must answer within {HEALTH_BOUND:?}");
        assert_eq!(statuses[..8], [StatusCode::UNAUTHORIZED; 8]);
        assert_eq!(statuses[8..], [StatusCode::OK; 2]);
    }

    fn stored_hash(state: &AppState, admin_id: &str) -> String {
        state
            .store
            .get_administrator(admin_id)
            .unwrap()
            .unwrap()
            .password_hash
            .unwrap()
    }

    // F-184/C-041: a password change verifies the current password and hashes
    // the new one through argon2_off_runtime, and so does activation, so no
    // Argon2 run holds an async worker every other request needs.
    #[tokio::test]
    async fn password_change_and_activation_run_argon2_off_the_async_workers() {
        let state = test_state();
        install(&state);
        let cookie = login_cookie(&state).await;
        let old_hash = stored_hash(&state, "alice-id");
        let verified_before = off_runtime_count(&old_hash);
        let body = format!(
            r#"{{"current_password":"{PASSWORD}","new_password":"a-brand-new-passphrase"}}"#
        );
        let changed = crate::build(state.clone())
            .oneshot(proxied(
                "PUT",
                "/api/admin/me/password",
                Some(&cookie),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(changed.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            off_runtime_count(&old_hash),
            verified_before + 1,
            "the current password must verify off the runtime"
        );
        assert_eq!(
            off_runtime_count(&stored_hash(&state, "alice-id")),
            1,
            "the new password must hash off the runtime"
        );

        state
            .store
            .create_pending_administrator(Administrator {
                id: "bob-id".to_string(),
                username: "bob".to_string(),
                password_hash: None,
                status: AdministratorStatus::Pending,
                created_at: 0,
                activation_token_hash: Some(sha256_hex(b"bob-activation-token")),
                activation_expires_at: Some(u64::MAX),
                session_generation: 0,
            })
            .unwrap();
        let activated = crate::build(state.clone())
            .oneshot(proxied(
                "POST",
                "/api/activate/bob-activation-token",
                None,
                r#"{"password":"bobs-own-passphrase"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(activated.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            off_runtime_count(&stored_hash(&state, "bob-id")),
            1,
            "activation must hash off the runtime"
        );
    }

    // An admin write waiting on redb's write lock must not hold a worker.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn health_answers_while_admin_writes_wait_on_the_write_lock() {
        let state = test_state();
        install(&state);
        let cookie = login_cookie(&state).await;
        let body = r#"{"name":"Lab","geo_label":"DE","kind":"local","offered_methods":["ping"]}"#;
        let load = (0..2)
            .map(|_| proxied("POST", "/api/admin/locations", Some(&cookie), body))
            .collect();

        let (answered, statuses) = health_answers_under(&state, load, || ()).await;
        assert!(answered, "/health must answer within {HEALTH_BOUND:?}");
        assert_eq!(statuses, [StatusCode::CREATED; 2]);
    }

    pub(crate) fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "lg-auth-unit-{}-{}",
            std::process::id(),
            super::random_id()
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        AppState {
            store: Store::open(dir.join("db.redb")).expect("open test store"),
            transport: TransportConfig::new([std::net::IpAddr::from(Ipv4Addr::LOCALHOST)]),
            login_limiter: Arc::new(LoginLimiter::default()),
            setup_token: None,
            run: RunService::for_test(8, Duration::from_secs(30), 100),
            files_root: Arc::from(dir.as_path()),
            enroll: EnrollConfig::for_test("https://central.test:8443", b"identity".to_vec()),
            tunnel_hub: TunnelHub::new(),
        }
    }

    pub(crate) fn install(state: &AppState) {
        state
            .store
            .create_first_administrator(
                "alice-id".to_string(),
                "alice".to_string(),
                hash_password(PASSWORD).unwrap(),
            )
            .unwrap();
    }

    fn session_rows(store: &Store) -> u64 {
        let txn = store.database().begin_read().unwrap();
        txn.open_table(SESSION).unwrap().len().unwrap()
    }

    fn proxied(method: &str, uri: &str, cookie: Option<&str>, body: &str) -> Request<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-forwarded-proto", "https")
            .extension(ConnectInfo(SocketAddr::from((Ipv4Addr::LOCALHOST, 40000))));
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request.body(Body::from(body.to_string())).unwrap()
    }

    // A login that verified the old password must not be stamped with the
    // credential generation of a rotation that committed after that check.
    #[tokio::test]
    async fn login_racing_a_password_rotation_is_refused_after_it() {
        let state = test_state();
        install(&state);
        let old_hash = state
            .store
            .get_administrator("alice-id")
            .unwrap()
            .unwrap()
            .password_hash
            .unwrap();
        let store = state.store.clone();
        *AFTER_VERIFY.lock().unwrap() = Some(Box::new(move || {
            let rotated = store
                .rotate_password(
                    "alice-id",
                    &old_hash,
                    hash_password("a-brand-new-passphrase").unwrap(),
                )
                .unwrap();
            assert_eq!(rotated, Some(1), "the racing rotation must commit");
        }));

        let session = Session::new(None, Arc::new(RedbSessionStore::new(&state.store)), None);
        let status = login(
            State(state.clone()),
            session.clone(),
            ClientContext {
                ip: Some(Ipv4Addr::LOCALHOST.into()),
                secure: true,
            },
            HeaderMap::new(),
            AdminJson(LoginRequest {
                username: "alice".to_string(),
                password: PASSWORD.to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(
            AFTER_VERIFY.lock().unwrap().is_none(),
            "the rotation must have run inside the login"
        );

        let (mut parts, ()) = Request::new(()).into_parts();
        parts.extensions.insert(session);
        let refused = AdminSession::from_request_parts(&mut parts, &state).await;
        assert!(
            matches!(refused, Err(ApiError::Unauthorized)),
            "an old-password session must not survive the rotation"
        );
    }

    // Logout removes the record and mints no replacement row.
    #[tokio::test]
    async fn logout_leaves_no_session_row() {
        let state = test_state();
        install(&state);
        let login = crate::build(state.clone())
            .oneshot(proxied(
                "POST",
                "/api/auth/login",
                None,
                &format!(r#"{{"username":"alice","password":"{PASSWORD}"}}"#),
            ))
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::NO_CONTENT);
        let cookie = login.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        assert_eq!(session_rows(&state.store), 1);

        let logout = crate::build(state.clone())
            .oneshot(proxied("POST", "/api/auth/logout", Some(&cookie), ""))
            .await
            .unwrap();
        assert_eq!(logout.status(), StatusCode::NO_CONTENT);
        assert_eq!(session_rows(&state.store), 0);
    }

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

    // A store backend failure answers a generic 500, so its cause must
    // reach the operator log at the one place it becomes an ApiError.
    #[test]
    fn store_backend_errors_are_logged_with_their_cause() {
        let logs = LogSink::default();
        let sink = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish();
        let error = tracing::subscriber::with_default(subscriber, || {
            ApiError::from(crate::StoreError::Backend(
                "f118 probe: disk full".to_string(),
            ))
        });

        assert!(matches!(error, ApiError::Internal));
        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert!(logs.contains("f118 probe: disk full"), "{logs}");
    }

    /// Swaps the session table for one of another type, so every session-store
    /// call fails with a redb backend error. Returns the error's text.
    pub(crate) fn break_session_table(store: &Store) -> String {
        let txn = store.database().begin_write().unwrap();
        txn.delete_table(SESSION).unwrap();
        txn.open_table(redb::TableDefinition::<u64, u64>::new("session"))
            .unwrap();
        let cause = txn.open_table(SESSION).unwrap_err().to_string();
        txn.commit().unwrap();
        cause
    }

    /// Captures every tracing event on this thread until the guard drops.
    pub(crate) fn capture_logs() -> (LogSink, tracing::subscriber::DefaultGuard) {
        let logs = LogSink::default();
        let sink = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    impl LogSink {
        pub(crate) fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    // F-334: a session-store failure while rotating the login's session id
    // answers a generic 500, so its cause must reach the operator log.
    #[tokio::test]
    async fn login_session_cycle_failure_is_logged_with_its_cause() {
        let state = test_state();
        install(&state);
        let cause = break_session_table(&state.store);
        let (logs, _guard) = capture_logs();

        let session = Session::new(None, Arc::new(RedbSessionStore::new(&state.store)), None);
        let result = login(
            State(state.clone()),
            session,
            ClientContext {
                ip: Some(Ipv4Addr::LOCALHOST.into()),
                secure: true,
            },
            HeaderMap::new(),
            AdminJson(LoginRequest {
                username: "alice".to_string(),
                password: PASSWORD.to_string(),
            }),
        )
        .await;

        assert!(matches!(result, Err(ApiError::Internal)));
        let logs = logs.text();
        assert!(logs.contains("ERROR") && logs.contains(&cause), "{logs}");
    }

    #[tokio::test]
    async fn logout_session_flush_failure_is_logged_with_its_cause() {
        let state = test_state();
        install(&state);
        // A saved session, so flush has a stored record to delete.
        let session = Session::new(None, Arc::new(RedbSessionStore::new(&state.store)), None);
        session.insert(SESSION_ADMIN_KEY, "alice-id").await.unwrap();
        session.save().await.unwrap();
        let cause = break_session_table(&state.store);
        let (logs, _guard) = capture_logs();

        let result = logout(session, HeaderMap::new()).await;

        assert!(matches!(result, Err(ApiError::Internal)));
        // tower-sessions already logs the store error itself (`instrument(err)`),
        // so require the handler's own line, which carries the correlation id.
        let logs = logs.text();
        assert!(
            logs.lines().any(|line| line.contains("ERROR")
                && line.contains("event=\"auth.logout\"")
                && line.contains(&cause)),
            "{logs}"
        );
    }

    #[derive(Clone, Default)]
    pub(crate) struct LogSink(pub(crate) Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for LogSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
