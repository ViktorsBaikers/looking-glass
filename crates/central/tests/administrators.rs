//! Equal-peer Administrators integration (ADR-0001, spec #1): the peer list,
//! creation through one-time 24h activation links, regeneration invalidating the
//! old link, removal ending the peer's sessions, change-password semantics, every
//! guard rail, and the legacy single-admin volume migration. Seeded through the
//! admin API like the other integration files.

mod common;

use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, Response, StatusCode};
use redb::{Database, TableDefinition};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use central::AppState;
use common::{
    assert_status, body_string, secure_request, send, session_cookie, temp_db_path, test_state,
    test_state_at, CENTRAL_URL, SETUP_TOKEN, TRUSTED_PROXY,
};

const PASSWORD: &str = "correct-horse-battery-staple";
const PEER_PASSWORD: &str = "peer-passphrase-long-enough";

async fn setup_and_login(state: &AppState) -> String {
    let install = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            "/api/setup",
            &json!({ "setup_token": SETUP_TOKEN, "username": "alice", "password": PASSWORD })
                .to_string(),
        ),
    )
    .await;
    assert_status(&install, StatusCode::CREATED);
    login(state, "alice", PASSWORD)
        .await
        .expect("alice session cookie")
}

/// Sign in and return the session cookie, or `None` when the login is refused.
async fn login(state: &AppState, username: &str, password: &str) -> Option<String> {
    let response = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            "/api/auth/login",
            &json!({ "username": username, "password": password }).to_string(),
        ),
    )
    .await;
    (response.status() == StatusCode::NO_CONTENT)
        .then(|| session_cookie(&response).expect("session cookie from an accepted login"))
}

/// A trusted-proxy admin request carrying the session cookie.
fn authed(method: &str, uri: &str, cookie: &str, json: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-forwarded-proto", "https")
        .header("cookie", cookie)
        .extension(ConnectInfo(SocketAddr::new(TRUSTED_PROXY, 40000)))
        .body(Body::from(json.to_string()))
        .unwrap()
}

async fn json_body(response: Response<Body>) -> Value {
    serde_json::from_str(&body_string(response).await).expect("json body")
}

/// Create a pending peer through the admin API; returns the 201 activation-link body.
async fn create_pending(state: &AppState, cookie: &str, username: &str) -> Value {
    let response = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/administrators",
            cookie,
            &json!({ "username": username }).to_string(),
        ),
    )
    .await;
    assert_status(&response, StatusCode::CREATED);
    json_body(response).await
}

/// The raw activation token from a link body (the URL is `{origin}/activate/{token}`).
fn token_of(link: &Value) -> String {
    let url = link["activation_url"].as_str().expect("activation_url");
    url.strip_prefix(&format!("{CENTRAL_URL}/activate/"))
        .unwrap_or_else(|| panic!("activation_url {url} must use the enrollment origin"))
        .to_string()
}

fn argon2_hash(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("hash password")
        .to_string()
}

// ----- Legacy migration -------------------------------------------------------

// Spec #1 "legacy single-admin row is migrated automatically on open" (AC of
// issue #3): a volume written by the pre-Administrators schema — a single-row
// `admin` table plus an installed `setup` state, exactly what the old bootstrap
// created — opens under the new code and the original administrator signs in.
#[tokio::test]
async fn legacy_single_admin_volume_migrates_and_still_signs_in() {
    let path = temp_db_path();
    {
        // The old schema, captured verbatim: redb tables `admin` (one row keyed
        // "admin") and `setup` (one row keyed "state").
        let db = Database::create(&path).expect("create legacy volume");
        let txn = db.begin_write().expect("legacy write txn");
        {
            let admin: TableDefinition<&str, &[u8]> = TableDefinition::new("admin");
            let mut table = txn.open_table(admin).expect("open legacy admin table");
            let row = json!({
                "id": "legacy-admin-id",
                "username": "alice",
                "password_hash": argon2_hash(PASSWORD),
                "created_at": 1_700_000_000u64,
            });
            table
                .insert("admin", serde_json::to_vec(&row).unwrap().as_slice())
                .expect("write legacy admin row");
            let setup: TableDefinition<&str, &[u8]> = TableDefinition::new("setup");
            let mut setup_table = txn.open_table(setup).expect("open legacy setup table");
            let state = json!({ "installed": true, "completed_at": 1_700_000_000u64 });
            setup_table
                .insert("state", serde_json::to_vec(&state).unwrap().as_slice())
                .expect("write legacy setup row");
        }
        txn.commit().expect("commit legacy volume");
    }

    let state = test_state_at(path.clone());

    // The legacy credential still authenticates, and the session carries the
    // migrated identity.
    let cookie = login(&state, "alice", PASSWORD)
        .await
        .expect("the migrated administrator signs in with the legacy password");
    let me = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/me", &cookie, ""),
    )
    .await;
    assert_status(&me, StatusCode::OK);
    assert_eq!(
        json_body(me).await,
        json!({ "id": "legacy-admin-id", "username": "alice" })
    );

    // The peer list shows exactly the migrated administrator, active, with no
    // activation link.
    let list = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/administrators", &cookie, ""),
    )
    .await;
    assert_status(&list, StatusCode::OK);
    let body = json_body(list).await;
    let admins = body.as_array().expect("administrator list");
    assert_eq!(admins.len(), 1, "migration creates exactly one peer");
    assert_eq!(admins[0]["id"], "legacy-admin-id");
    assert_eq!(admins[0]["username"], "alice");
    assert_eq!(admins[0]["status"], "active");
    assert_eq!(admins[0]["created_at"], 1_700_000_000u64);
    assert_eq!(admins[0]["activation_expires_at"], Value::Null);
    assert!(
        admins[0].get("password_hash").is_none(),
        "the list never exposes credential material"
    );
    drop(state);

    // Reopening the migrated volume must not duplicate or re-migrate.
    let reopened = test_state_at(path);
    assert_eq!(
        reopened.store.list_administrators().unwrap().len(),
        1,
        "the migration is idempotent across reopens"
    );
}

// ----- Creation + activation --------------------------------------------------

// Spec #1: POST administrators returns 201 with the peer, a one-time activation
// URL on the enrollment origin, and the 24h expiry; the peer lists as pending.
#[tokio::test]
async fn creating_a_peer_returns_a_one_time_activation_link() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    let before = common::unix_now_secs();
    let link = create_pending(&state, &cookie, "bob").await;

    let admin = &link["administrator"];
    assert_eq!(admin["username"], "bob");
    assert_eq!(admin["status"], "pending");
    assert_eq!(admin["activation_expires_at"], link["expires_at"]);
    let expires_at = link["expires_at"].as_u64().unwrap();
    assert!(
        expires_at >= before + 24 * 3600 - 5 && expires_at <= common::unix_now_secs() + 24 * 3600,
        "the activation link expires after 24h: {expires_at}"
    );
    assert!(!token_of(&link).is_empty());

    let list = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/administrators", &cookie, ""),
    )
    .await;
    let body = json_body(list).await;
    let usernames: Vec<(String, String)> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["username"].as_str().unwrap().to_string(),
                a["status"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(usernames.contains(&("bob".to_string(), "pending".to_string())));
}

// Spec #1 activation happy path: GET reveals the username, POST sets the
// password (204), the peer becomes active and can sign in, and the token is
// single-use — reuse is 410 activation_invalid.
#[tokio::test]
async fn a_pending_peer_activates_once_through_the_link() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let link = create_pending(&state, &cookie, "bob").await;
    let token = token_of(&link);

    let seen = send(
        central::build(state.clone()),
        secure_request("GET", &format!("/api/activate/{token}"), ""),
    )
    .await;
    assert_status(&seen, StatusCode::OK);
    assert_eq!(json_body(seen).await, json!({ "username": "bob" }));

    let activated = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            &format!("/api/activate/{token}"),
            &json!({ "password": PEER_PASSWORD }).to_string(),
        ),
    )
    .await;
    assert_status(&activated, StatusCode::NO_CONTENT);

    // The activated peer signs in with the password they chose.
    assert!(
        login(&state, "bob", PEER_PASSWORD).await.is_some(),
        "the activated peer signs in"
    );
    // A pending login was impossible before activation and the old password never
    // existed: the creator's guess must not work.
    assert!(login(&state, "bob", PASSWORD).await.is_none());

    // Single use: the same token is now invalid.
    let reused = send(
        central::build(state.clone()),
        secure_request("GET", &format!("/api/activate/{token}"), ""),
    )
    .await;
    assert_status(&reused, StatusCode::GONE);
    assert_eq!(json_body(reused).await["error"], "activation_invalid");
    let repost = send(
        central::build(state),
        secure_request(
            "POST",
            &format!("/api/activate/{token}"),
            &json!({ "password": PEER_PASSWORD }).to_string(),
        ),
    )
    .await;
    assert_status(&repost, StatusCode::GONE);
}

// An expired activation link is refused on both verbs with 410.
#[tokio::test]
async fn an_expired_activation_link_is_refused() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let link = create_pending(&state, &cookie, "bob").await;
    let token = token_of(&link);

    // Push the stored expiry into the past (no clock travel available over HTTP).
    let mut bob = state
        .store
        .list_administrators()
        .unwrap()
        .into_iter()
        .find(|a| a.username == "bob")
        .expect("bob exists");
    bob.activation_expires_at = Some(1);
    state.store.put_administrator(&bob).unwrap();

    let seen = send(
        central::build(state.clone()),
        secure_request("GET", &format!("/api/activate/{token}"), ""),
    )
    .await;
    assert_status(&seen, StatusCode::GONE);
    assert_eq!(json_body(seen).await["error"], "activation_invalid");

    let posted = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            &format!("/api/activate/{token}"),
            &json!({ "password": PEER_PASSWORD }).to_string(),
        ),
    )
    .await;
    assert_status(&posted, StatusCode::GONE);

    // The expired link did not activate anything: bob still cannot sign in.
    assert!(login(&state, "bob", PEER_PASSWORD).await.is_none());
}

// An unknown token is indistinguishable from an expired one: 410.
#[tokio::test]
async fn an_unknown_activation_token_is_refused() {
    let state = test_state();
    setup_and_login(&state).await;

    let seen = send(
        central::build(state.clone()),
        secure_request("GET", "/api/activate/deadbeefdeadbeef", ""),
    )
    .await;
    assert_status(&seen, StatusCode::GONE);
    assert_eq!(json_body(seen).await["error"], "activation_invalid");

    let posted = send(
        central::build(state),
        secure_request(
            "POST",
            "/api/activate/deadbeefdeadbeef",
            &json!({ "password": PEER_PASSWORD }).to_string(),
        ),
    )
    .await;
    assert_status(&posted, StatusCode::GONE);
}

// A password the install rules would refuse (shorter than 12) is rejected
// WITHOUT consuming the link — the peer can retry with a valid one.
#[tokio::test]
async fn activation_rejects_a_short_password_without_burning_the_link() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let link = create_pending(&state, &cookie, "bob").await;
    let token = token_of(&link);

    let rejected = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            &format!("/api/activate/{token}"),
            &json!({ "password": "too-short" }).to_string(),
        ),
    )
    .await;
    assert_status(&rejected, StatusCode::UNPROCESSABLE_ENTITY);

    let retry = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            &format!("/api/activate/{token}"),
            &json!({ "password": PEER_PASSWORD }).to_string(),
        ),
    )
    .await;
    assert_status(&retry, StatusCode::NO_CONTENT);
    assert!(login(&state, "bob", PEER_PASSWORD).await.is_some());
}

// ----- Regeneration ------------------------------------------------------------

// Spec #1: regenerating returns a new link and invalidates the previous one.
#[tokio::test]
async fn regenerating_a_link_invalidates_the_previous_one() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let first = create_pending(&state, &cookie, "bob").await;
    let old_token = token_of(&first);

    let second = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!(
                "/api/admin/administrators/{}/activation",
                first["administrator"]["id"].as_str().unwrap()
            ),
            &cookie,
            "",
        ),
    )
    .await;
    assert_status(&second, StatusCode::OK);
    let second = json_body(second).await;
    let new_token = token_of(&second);
    assert_ne!(old_token, new_token, "regeneration mints a fresh token");

    let old = send(
        central::build(state.clone()),
        secure_request("GET", &format!("/api/activate/{old_token}"), ""),
    )
    .await;
    assert_status(&old, StatusCode::GONE);

    let fresh = send(
        central::build(state.clone()),
        secure_request("GET", &format!("/api/activate/{new_token}"), ""),
    )
    .await;
    assert_status(&fresh, StatusCode::OK);
    assert_eq!(json_body(fresh).await, json!({ "username": "bob" }));
}

// Regeneration is for pending peers only — an activated administrator has no
// link to regenerate (409 not_pending).
#[tokio::test]
async fn regeneration_is_refused_for_an_active_administrator() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let link = create_pending(&state, &cookie, "bob").await;
    let token = token_of(&link);
    let bob_id = link["administrator"]["id"].as_str().unwrap().to_string();

    let activated = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            &format!("/api/activate/{token}"),
            &json!({ "password": PEER_PASSWORD }).to_string(),
        ),
    )
    .await;
    assert_status(&activated, StatusCode::NO_CONTENT);

    let regenerate = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/administrators/{bob_id}/activation"),
            &cookie,
            "",
        ),
    )
    .await;
    assert_status(&regenerate, StatusCode::CONFLICT);
    assert_eq!(json_body(regenerate).await["error"], "not_pending");

    // A peer that never existed is a plain 404.
    let ghost = send(
        central::build(state),
        authed(
            "POST",
            "/api/admin/administrators/ghost/activation",
            &cookie,
            "",
        ),
    )
    .await;
    assert_status(&ghost, StatusCode::NOT_FOUND);
}

// ----- Guard rails ---------------------------------------------------------------

// Spec #1: usernames are case-insensitively unique (409 username_taken).
#[tokio::test]
async fn usernames_are_taken_case_insensitively() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    create_pending(&state, &cookie, "bob").await;

    let clash = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/administrators",
            &cookie,
            &json!({ "username": "BoB" }).to_string(),
        ),
    )
    .await;
    assert_status(&clash, StatusCode::CONFLICT);
    assert_eq!(json_body(clash).await["error"], "username_taken");

    // The install-time administrator's name is taken too.
    let installer_clash = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/administrators",
            &cookie,
            &json!({ "username": "ALICE" }).to_string(),
        ),
    )
    .await;
    assert_status(&installer_clash, StatusCode::CONFLICT);

    // Username rules match install: a disallowed character is a validation error.
    let bad = send(
        central::build(state),
        authed(
            "POST",
            "/api/admin/administrators",
            &cookie,
            &json!({ "username": "no spaces" }).to_string(),
        ),
    )
    .await;
    assert_status(&bad, StatusCode::UNPROCESSABLE_ENTITY);
}

// Spec #1: you cannot remove yourself (409 cannot_remove_self).
#[tokio::test]
async fn an_administrator_cannot_remove_themselves() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let me = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/me", &cookie, ""),
    )
    .await;
    let me = json_body(me).await;

    let removed = send(
        central::build(state.clone()),
        authed(
            "DELETE",
            &format!("/api/admin/administrators/{}", me["id"].as_str().unwrap()),
            &cookie,
            "",
        ),
    )
    .await;
    assert_status(&removed, StatusCode::CONFLICT);
    assert_eq!(json_body(removed).await["error"], "cannot_remove_self");

    // The account survived its own refused removal.
    assert!(login(&state, "alice", PASSWORD).await.is_some());
}

// Removing a peer ends that peer's sessions immediately — their cookie is dead
// on the next request, while the remover stays signed in.
#[tokio::test]
async fn removing_a_peer_ends_their_sessions_immediately() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let link = create_pending(&state, &cookie, "bob").await;
    let bob_id = link["administrator"]["id"].as_str().unwrap().to_string();
    let token = token_of(&link);
    let activated = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            &format!("/api/activate/{token}"),
            &json!({ "password": PEER_PASSWORD }).to_string(),
        ),
    )
    .await;
    assert_status(&activated, StatusCode::NO_CONTENT);
    let bob_cookie = login(&state, "bob", PEER_PASSWORD)
        .await
        .expect("bob signs in");

    // Bob's session works before the removal.
    let before = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/me", &bob_cookie, ""),
    )
    .await;
    assert_status(&before, StatusCode::OK);

    let removed = send(
        central::build(state.clone()),
        authed(
            "DELETE",
            &format!("/api/admin/administrators/{bob_id}"),
            &cookie,
            "",
        ),
    )
    .await;
    assert_status(&removed, StatusCode::NO_CONTENT);

    let after = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/me", &bob_cookie, ""),
    )
    .await;
    assert_status(&after, StatusCode::UNAUTHORIZED);
    assert!(
        login(&state, "bob", PEER_PASSWORD).await.is_none(),
        "the removed peer can no longer sign in"
    );

    // The remover is untouched, and bob is gone from the list.
    let still = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/administrators", &cookie, ""),
    )
    .await;
    assert_status(&still, StatusCode::OK);
    let body = json_body(still).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["username"], "alice");

    // Removing a peer that does not exist is a 404.
    let ghost = send(
        central::build(state),
        authed("DELETE", "/api/admin/administrators/ghost", &cookie, ""),
    )
    .await;
    assert_status(&ghost, StatusCode::NOT_FOUND);
}

// Two active peers: one may remove the other (the last-active guard must not
// false-trigger while an active peer remains).
#[tokio::test]
async fn an_active_peer_may_remove_another_active_peer() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let link = create_pending(&state, &cookie, "bob").await;
    let bob_id = link["administrator"]["id"].as_str().unwrap().to_string();
    let activated = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            &format!("/api/activate/{}", token_of(&link)),
            &json!({ "password": PEER_PASSWORD }).to_string(),
        ),
    )
    .await;
    assert_status(&activated, StatusCode::NO_CONTENT);

    let removed = send(
        central::build(state.clone()),
        authed(
            "DELETE",
            &format!("/api/admin/administrators/{bob_id}"),
            &cookie,
            "",
        ),
    )
    .await;
    assert_status(&removed, StatusCode::NO_CONTENT);
    assert!(login(&state, "alice", PASSWORD).await.is_some());
}

// ----- Change password -----------------------------------------------------------

// Spec #1: PUT /api/admin/me/password verifies the current password (403
// invalid_credentials on a miss), and signs out the caller's OTHER sessions
// while keeping the current one.
#[tokio::test]
async fn changing_the_password_ends_other_sessions_but_not_the_current_one() {
    let state = test_state();
    let cookie_a = setup_and_login(&state).await;
    let cookie_b = login(&state, "alice", PASSWORD)
        .await
        .expect("second alice session");

    const NEW_PASSWORD: &str = "rotated-passphrase-long-enough";
    let changed = send(
        central::build(state.clone()),
        authed(
            "PUT",
            "/api/admin/me/password",
            &cookie_a,
            &json!({ "current_password": PASSWORD, "new_password": NEW_PASSWORD }).to_string(),
        ),
    )
    .await;
    assert_status(&changed, StatusCode::NO_CONTENT);

    // The current session survives.
    let me = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/me", &cookie_a, ""),
    )
    .await;
    assert_status(&me, StatusCode::OK);

    // The other session is dead.
    let other = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/me", &cookie_b, ""),
    )
    .await;
    assert_status(&other, StatusCode::UNAUTHORIZED);

    // The new password signs in; the old one does not.
    assert!(login(&state, "alice", NEW_PASSWORD).await.is_some());
    assert!(login(&state, "alice", PASSWORD).await.is_none());
}

// A wrong current password is 403 invalid_credentials; a too-short new password
// is a validation error; neither changes anything — the old password still works
// and other sessions survive a refused change.
#[tokio::test]
async fn a_refused_password_change_changes_nothing() {
    let state = test_state();
    let cookie_a = setup_and_login(&state).await;
    let cookie_b = login(&state, "alice", PASSWORD)
        .await
        .expect("second alice session");

    let wrong_current = send(
        central::build(state.clone()),
        authed(
            "PUT",
            "/api/admin/me/password",
            &cookie_a,
            &json!({ "current_password": "not-the-password", "new_password": PEER_PASSWORD })
                .to_string(),
        ),
    )
    .await;
    assert_status(&wrong_current, StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(wrong_current).await["error"],
        "invalid_credentials"
    );

    let too_short = send(
        central::build(state.clone()),
        authed(
            "PUT",
            "/api/admin/me/password",
            &cookie_a,
            &json!({ "current_password": PASSWORD, "new_password": "short" }).to_string(),
        ),
    )
    .await;
    assert_status(&too_short, StatusCode::UNPROCESSABLE_ENTITY);

    // Nothing changed: the old password still signs in and both sessions live.
    assert!(login(&state, "alice", PASSWORD).await.is_some());
    let b = send(
        central::build(state),
        authed("GET", "/api/admin/me", &cookie_b, ""),
    )
    .await;
    assert_status(&b, StatusCode::OK);
}

// ----- Me --------------------------------------------------------------------------

// Spec #1: GET /api/admin/me returns {id, username} of the signed-in peer, and
// the id matches their entry in the peer list.
#[tokio::test]
async fn me_reports_the_signed_in_identity() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    let me = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/me", &cookie, ""),
    )
    .await;
    assert_status(&me, StatusCode::OK);
    let me = json_body(me).await;
    assert_eq!(me["username"], "alice");
    assert!(me["id"].as_str().is_some_and(|id| !id.is_empty()));

    let list = send(
        central::build(state),
        authed("GET", "/api/admin/administrators", &cookie, ""),
    )
    .await;
    let body = json_body(list).await;
    let admins = body.as_array().unwrap();
    assert_eq!(admins.len(), 1);
    assert_eq!(admins[0]["id"], me["id"]);
    assert_eq!(admins[0]["status"], "active");
}

// ----- Revocation races ----------------------------------------------------------

/// The tower-sessions `Id` inside a signed `lg.sid=` cookie value — the cookie
/// crate's signed format is `{44-char base64 digest}{id}`, so the id is the tail.
fn session_id(cookie: &str) -> tower_sessions::session::Id {
    let value = cookie.trim_start_matches(&format!("{}=", central::SESSION_COOKIE_NAME));
    value[44..].parse().expect("session id")
}

// SEC-003: a password change bumps the administrator's credential generation, so
// a stale session record re-saved by an overlapping in-flight request (the
// session middleware saves on every request) is refused by the extractor even
// though the administrator row is still active — the revoked cookie stays dead.
#[tokio::test]
async fn a_session_resurrected_after_a_password_change_is_refused() {
    use tower_sessions::SessionStore;

    let state = test_state();
    let cookie_a = setup_and_login(&state).await;
    let cookie_b = login(&state, "alice", PASSWORD)
        .await
        .expect("second alice session");

    // Capture the second session's record so it can be re-saved afterwards,
    // exactly as an overlapping request's always-save would.
    let sessions = central::RedbSessionStore::new(&state.store);
    let stale = sessions
        .load(&session_id(&cookie_b))
        .await
        .expect("session store")
        .expect("the second session record exists");

    let changed = send(
        central::build(state.clone()),
        authed(
            "PUT",
            "/api/admin/me/password",
            &cookie_a,
            &json!({ "current_password": PASSWORD, "new_password": PEER_PASSWORD }).to_string(),
        ),
    )
    .await;
    assert_status(&changed, StatusCode::NO_CONTENT);

    // The purge deleted the other record; the overlapping save puts it back.
    assert!(
        sessions
            .load(&stale.id)
            .await
            .expect("session store")
            .is_none(),
        "the password change purged the other session"
    );
    sessions.save(&stale).await.expect("resurrect the record");

    let resurrected = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/me", &cookie_b, ""),
    )
    .await;
    assert_status(&resurrected, StatusCode::UNAUTHORIZED);

    // The caller's own session was moved to the new generation and survives.
    let me = send(
        central::build(state),
        authed("GET", "/api/admin/me", &cookie_a, ""),
    )
    .await;
    assert_status(&me, StatusCode::OK);
}

// A password change carries the current AND the new password, so it refuses
// cleartext transport exactly like login and activation (403 insecure_transport)
// and changes nothing.
#[tokio::test]
async fn password_change_requires_secure_transport() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    let cleartext = Request::builder()
        .method("PUT")
        .uri("/api/admin/me/password")
        .header("content-type", "application/json")
        .header("cookie", &cookie)
        .extension(ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)),
            40000,
        )))
        .body(Body::from(
            json!({ "current_password": PASSWORD, "new_password": PEER_PASSWORD }).to_string(),
        ))
        .unwrap();
    let refused = send(central::build(state.clone()), cleartext).await;
    assert_status(&refused, StatusCode::FORBIDDEN);
    assert_eq!(json_body(refused).await["error"], "insecure_transport");

    // Nothing changed: the old password still signs in.
    assert!(login(&state, "alice", PASSWORD).await.is_some());
}

// SEC-001: the removal guard runs inside the delete transaction. Two active
// peers removing each other concurrently serialize in the store; the second
// removal sees the first one's commit and is refused as the last active
// administrator, even though its caller authorized against a stale view.
#[tokio::test]
async fn cross_removal_of_two_active_peers_keeps_one_active() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let link = create_pending(&state, &cookie, "bob").await;
    let bob_id = link["administrator"]["id"].as_str().unwrap().to_string();
    let activated = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            &format!("/api/activate/{}", token_of(&link)),
            &json!({ "password": PEER_PASSWORD }).to_string(),
        ),
    )
    .await;
    assert_status(&activated, StatusCode::NO_CONTENT);
    let alice_id = state
        .store
        .list_administrators()
        .unwrap()
        .into_iter()
        .find(|admin| admin.username == "alice")
        .unwrap()
        .id;

    state
        .store
        .remove_administrator(&alice_id, &bob_id)
        .unwrap();
    assert!(matches!(
        state.store.remove_administrator(&bob_id, &alice_id),
        Err(central::RemoveAdministratorError::LastActive)
    ));
    assert!(login(&state, "alice", PASSWORD).await.is_some());
}

// SEC-002: the password rotation is conditional — a removed row is never
// re-created, and a hash that no longer matches (a second rotation won the
// race) changes nothing.
#[tokio::test]
async fn password_rotation_never_recreates_or_overwrites_a_changed_row() {
    let state = test_state();
    setup_and_login(&state).await;
    let alice = state
        .store
        .list_administrators()
        .unwrap()
        .into_iter()
        .find(|admin| admin.username == "alice")
        .unwrap();
    let hash = alice.password_hash.clone().unwrap();

    assert_eq!(
        state
            .store
            .rotate_password(&alice.id, "stale-hash", argon2_hash(PEER_PASSWORD))
            .unwrap(),
        None
    );
    assert!(login(&state, "alice", PASSWORD).await.is_some());

    assert_eq!(
        state
            .store
            .rotate_password("ghost", &hash, argon2_hash(PEER_PASSWORD))
            .unwrap(),
        None
    );
    assert!(state.store.get_administrator("ghost").unwrap().is_none());
}
