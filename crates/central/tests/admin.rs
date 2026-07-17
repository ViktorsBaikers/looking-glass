//! Admin CRUD integration (Slice 5, AC23/24/25): the fail-closed admin gate, the
//! validate-before-write / no-partial-write guarantee, entities reflected in the
//! public read API, settings persistence, and cascade delete through the HTTP
//! surface. The exhaustive per-table cascade proof is a store unit test; this file
//! proves the admin surface end-to-end.

mod common;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::net::SocketAddr;

use central::AppState;
use common::{
    assert_status, body_string, secure_request, send, session_cookie, temp_db_path, test_state,
    test_state_at, SETUP_TOKEN, TRUSTED_PROXY,
};

const PASSWORD: &str = "correct-horse-battery-staple";

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

    let login = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            "/api/auth/login",
            &json!({ "username": "alice", "password": PASSWORD }).to_string(),
        ),
    )
    .await;
    assert_status(&login, StatusCode::NO_CONTENT);
    session_cookie(&login).expect("session cookie from login")
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

async fn json_body(response: axum::http::Response<Body>) -> Value {
    serde_json::from_str(&body_string(response).await).expect("json body")
}

// AC4/AC23: the admin surface is fail-closed — an admin route with no session is
// refused, never served.
#[tokio::test]
async fn admin_routes_require_a_session() {
    let state = test_state();
    setup_and_login(&state).await;

    let response = send(
        central::build(state),
        Request::builder()
            .uri("/api/admin/locations")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&response, StatusCode::UNAUTHORIZED);
}

// AC23: a created location and its entities are reflected in the public read API,
// with the offered-method set the visitor selector filters on (FR-015).
#[tokio::test]
async fn location_and_entities_are_reflected_in_the_public_api() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            &cookie,
            &json!({
                "name": "Frankfurt",
                "geo_label": "DE",
                "kind": "local",
                "offered_methods": ["ping", "mtr"]
            })
            .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    let location_id = json_body(created).await["id"].as_str().unwrap().to_string();

    let ip = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{location_id}/test-ips"),
            &cookie,
            &json!({ "family": "v4", "address": "203.0.113.10", "label": "primary" }).to_string(),
        ),
    )
    .await;
    assert_status(&ip, StatusCode::CREATED);

    // Public read (no auth) must reflect the location, its offered methods, and IP.
    let public = send(
        central::build(state),
        Request::builder()
            .uri("/api/locations")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&public, StatusCode::OK);
    let body = json_body(public).await;
    let loc = &body.as_array().unwrap()[0];
    assert_eq!(loc["name"], "Frankfurt");
    assert_eq!(loc["status"], "online", "a local node is online");
    assert_eq!(loc["offered_methods"], json!(["ping", "mtr"]));
    assert_eq!(loc["test_ips"][0]["address"], "203.0.113.10");
}

// FR-026 / info-disclosure: the public read exposes only live (Online) locations
// — a staged remote (Offline until its agent enrolls) and its details must not
// leak, while the built-in local node (Online) is shown.
#[tokio::test]
async fn public_api_hides_offline_locations() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    // A local node defaults Online; a remote defaults Offline (not yet enrolled).
    for (name, kind) in [("Live-Local", "local"), ("Staged-Remote", "remote")] {
        let created = send(
            central::build(state.clone()),
            authed(
                "POST",
                "/api/admin/locations",
                &cookie,
                &json!({ "name": name, "geo_label": "DE", "kind": kind, "offered_methods": [] })
                    .to_string(),
            ),
        )
        .await;
        assert_status(&created, StatusCode::CREATED);
    }

    // Admin read returns both.
    let admin_list = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/locations", &cookie, ""),
    )
    .await;
    assert_eq!(
        json_body(admin_list).await.as_array().unwrap().len(),
        2,
        "the admin read returns every location, online or not"
    );

    // Public read returns only the Online one.
    let public = send(
        central::build(state),
        Request::builder()
            .uri("/api/locations")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let body = json_body(public).await;
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|loc| loc["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Live-Local"], "only live locations are public");
}

#[tokio::test]
async fn remote_data_plane_origin_must_be_an_http_origin() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    for origin in [
        "",
        "/files",
        "javascript:alert(1)",
        "https://remote.example.test/files",
        "https://remote.example.test?x=1",
        "https://remote.example.test#frag",
        "https://user@remote.example.test",
    ] {
        let response = send(
            central::build(state.clone()),
            authed(
                "POST",
                "/api/admin/locations",
                &cookie,
                &json!({
                    "name": "Remote",
                    "geo_label": "DE",
                    "kind": "remote",
                    "data_plane_origin": origin,
                    "offered_methods": []
                })
                .to_string(),
            ),
        )
        .await;
        assert_status(&response, StatusCode::UNPROCESSABLE_ENTITY);
    }

    let valid = send(
        central::build(state),
        authed(
            "POST",
            "/api/admin/locations",
            &cookie,
            &json!({
                "name": "Remote",
                "geo_label": "DE",
                "kind": "remote",
                "data_plane_origin": "https://remote.example.test:9443",
                "offered_methods": []
            })
            .to_string(),
        ),
    )
    .await;
    assert_status(&valid, StatusCode::CREATED);
    assert_eq!(
        json_body(valid).await["data_plane_origin"],
        "https://remote.example.test:9443"
    );
}

#[tokio::test]
async fn local_locations_do_not_persist_or_expose_data_plane_origins() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            &cookie,
            &json!({
                "name": "Local",
                "geo_label": "DE",
                "kind": "local",
                "data_plane_origin": "https://stale-remote.example.test",
                "offered_methods": []
            })
            .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    let body = json_body(created).await;
    let id = body["id"].as_str().unwrap().to_string();
    assert!(body["data_plane_origin"].is_null());
    assert!(state
        .store
        .get_location(&id)
        .unwrap()
        .unwrap()
        .data_plane_origin
        .is_none());

    let public = send(
        central::build(state),
        Request::builder()
            .uri("/api/locations")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let local = json_body(public).await.as_array().unwrap()[0].clone();
    assert!(local["data_plane_origin"].is_null());
}

#[tokio::test]
async fn remote_test_file_source_ref_refuses_browser_normalizing_paths() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let loc_id = create_remote_location(&state, &cookie, json!(["ping"])).await;

    for source_ref in ["../secret.bin", "sub/../secret.bin", "/probe.bin"] {
        let response = send(
            central::build(state.clone()),
            authed(
                "POST",
                &format!("/api/admin/locations/{loc_id}/files"),
                &cookie,
                &json!({
                    "label": "Probe",
                    "declared_size": "16 B",
                    "source_ref": source_ref
                })
                .to_string(),
            ),
        )
        .await;
        assert_status(&response, StatusCode::UNPROCESSABLE_ENTITY);
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Create a remote location offering `methods` through the admin API and return its
/// id. A remote persists `Offline` until an agent heartbeats (Slice 8b derives it).
async fn create_remote_location(state: &AppState, cookie: &str, methods: Value) -> String {
    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            cookie,
            &json!({ "name": "Remote-1", "geo_label": "SG", "kind": "remote", "offered_methods": methods })
                .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    json_body(created).await["id"].as_str().unwrap().to_string()
}

async fn public_names(state: &AppState) -> Vec<String> {
    let public = send(
        central::build(state.clone()),
        Request::builder()
            .uri("/api/locations")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    json_body(public)
        .await
        .as_array()
        .unwrap()
        .iter()
        .map(|loc| loc["name"].as_str().unwrap().to_string())
        .collect()
}

// AC7 (online) / AC17 / FR-026: a remote whose agent is heartbeating (recent
// last_seen) derives online — it appears in the public selector carrying only its
// offered methods, and the admin list reflects online + a last-seen timestamp.
#[tokio::test]
async fn a_heartbeating_remote_is_public_and_reflected_in_admin() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let loc_id = create_remote_location(&state, &cookie, json!(["ping", "mtr"])).await;

    // The agent has beaten just now → within the 30s window → online.
    state
        .store
        .put_agent(&central::Agent {
            id: "agent-live".to_string(),
            location_id: loc_id.clone(),
            credential_hash: "$argon2id$stub".to_string(),
            enrolled_at: 0,
            last_seen: Some(unix_now()),
            revoked: false,
        })
        .unwrap();

    // Public selector: the online remote is listed, with its offered methods intact.
    let public = send(
        central::build(state.clone()),
        Request::builder()
            .uri("/api/locations")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let body = json_body(public).await;
    let remote = body
        .as_array()
        .unwrap()
        .iter()
        .find(|loc| loc["id"] == json!(loc_id))
        .expect("the online remote is in the public selector");
    assert_eq!(remote["status"], "online");
    assert_eq!(
        remote["offered_methods"],
        json!(["ping", "mtr"]),
        "the method selector still lists only this location's offered methods"
    );

    // Admin list: online status derived + a last-seen timestamp for its column.
    let admin = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/locations", &cookie, ""),
    )
    .await;
    let admin_body = json_body(admin).await;
    let admin_remote = admin_body
        .as_array()
        .unwrap()
        .iter()
        .find(|loc| loc["id"] == json!(loc_id))
        .expect("the remote is in the admin list");
    assert_eq!(admin_remote["status"], "online");
    assert!(
        admin_remote["last_seen"].is_u64(),
        "the admin list carries a last-seen timestamp for the remote"
    );
}

// AC17 / FR-026: a remote whose agent last beat outside the window derives offline —
// it is absent from the public selector and shown offline in admin.
#[tokio::test]
async fn a_remote_past_the_heartbeat_window_is_hidden_from_the_public_selector() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let loc_id = create_remote_location(&state, &cookie, json!(["ping"])).await;

    // Last beat well outside the 30s window → offline.
    state
        .store
        .put_agent(&central::Agent {
            id: "agent-stale".to_string(),
            location_id: loc_id.clone(),
            credential_hash: "$argon2id$stub".to_string(),
            enrolled_at: 0,
            last_seen: Some(unix_now().saturating_sub(120)),
            revoked: false,
        })
        .unwrap();

    assert!(
        !public_names(&state).await.iter().any(|n| n == "Remote-1"),
        "a stale remote is not selectable in the public UI"
    );

    let admin = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/locations", &cookie, ""),
    )
    .await;
    let admin_remote = json_body(admin)
        .await
        .as_array()
        .unwrap()
        .iter()
        .find(|loc| loc["id"] == json!(loc_id))
        .cloned()
        .expect("the remote is still in the admin list");
    assert_eq!(admin_remote["status"], "offline");
}

// Resurrection-hole guard (drift.md, security): a REVOKED agent with a fresh
// last_seen must never derive online — it stays out of the public selector.
#[tokio::test]
async fn a_revoked_agent_with_a_recent_beat_stays_offline_and_unselectable() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let loc_id = create_remote_location(&state, &cookie, json!(["ping"])).await;

    // Revoked, yet beat one second ago: liveness must not resurrect it.
    state
        .store
        .put_agent(&central::Agent {
            id: "agent-revoked".to_string(),
            location_id: loc_id.clone(),
            credential_hash: "$argon2id$stub".to_string(),
            enrolled_at: 0,
            last_seen: Some(unix_now()),
            revoked: true,
        })
        .unwrap();

    assert!(
        !public_names(&state).await.iter().any(|n| n == "Remote-1"),
        "a revoked agent never appears online in the public selector, however recent its beat"
    );
}

#[tokio::test]
async fn revoking_a_remote_agent_returns_the_location_to_not_enrolled() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let loc_id = create_remote_location(&state, &cookie, json!(["ping"])).await;
    state
        .store
        .put_agent(&central::Agent {
            id: "agent-live".to_string(),
            location_id: loc_id.clone(),
            credential_hash: "$argon2id$stub".to_string(),
            enrolled_at: 0,
            last_seen: Some(unix_now()),
            revoked: false,
        })
        .unwrap();

    let revoked = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{loc_id}/agent/revoke"),
            &cookie,
            "{}",
        ),
    )
    .await;

    assert_status(&revoked, StatusCode::OK);
    let body = json_body(revoked).await;
    assert_eq!(body["status"], "offline");
    assert_eq!(body["last_seen"], Value::Null);
    let agent = state.store.get_agent("agent-live").unwrap().unwrap();
    assert!(
        agent.revoked,
        "the credential must stop verifying immediately"
    );
    assert_eq!(agent.last_seen, None, "admin state returns to not-enrolled");
    assert!(
        !public_names(&state).await.iter().any(|n| n == "Remote-1"),
        "the revoked remote is not selectable publicly"
    );
}

#[tokio::test]
async fn revoked_recent_agent_stays_not_enrolled_after_restart_in_admin_api() {
    let db_path = temp_db_path();
    let state = test_state_at(db_path.clone());
    let cookie = setup_and_login(&state).await;
    let loc_id = create_remote_location(&state, &cookie, json!(["ping"])).await;
    state
        .store
        .put_agent(&central::Agent {
            id: "agent-revoked-recent".to_string(),
            location_id: loc_id.clone(),
            credential_hash: "$argon2id$stub".to_string(),
            enrolled_at: 0,
            last_seen: Some(unix_now()),
            revoked: true,
        })
        .unwrap();
    drop(state);

    let reopened = test_state_at(db_path);
    let admin = send(
        central::build(reopened),
        authed("GET", "/api/admin/locations", &cookie, ""),
    )
    .await;
    assert_status(&admin, StatusCode::OK);
    let body = json_body(admin).await;
    let remote = body
        .as_array()
        .unwrap()
        .iter()
        .find(|loc| loc["id"] == json!(loc_id))
        .expect("remote location is listed for admin");
    assert_eq!(remote["status"], "offline");
    assert_eq!(
        remote["last_seen"],
        Value::Null,
        "a revoked recent beat must derive not-enrolled after restart"
    );
}

// AC24 (crux): an invalid create is rejected with a field message and writes
// nothing — the store is unchanged after the rejected request.
#[tokio::test]
async fn invalid_create_is_rejected_with_no_partial_write() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    let rejected = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            &cookie,
            &json!({ "name": "", "geo_label": "DE", "kind": "local", "offered_methods": [] })
                .to_string(),
        ),
    )
    .await;
    assert_status(&rejected, StatusCode::UNPROCESSABLE_ENTITY);
    let error = json_body(rejected).await;
    assert_eq!(error["error"], "invalid_input");
    assert!(
        error["message"].as_str().unwrap().contains("name"),
        "the message names the offending field: {error}"
    );

    // No partial write: the location list is still empty.
    let list = send(
        central::build(state),
        authed("GET", "/api/admin/locations", &cookie, ""),
    )
    .await;
    let body = json_body(list).await;
    assert_eq!(
        body.as_array().unwrap().len(),
        0,
        "a rejected create wrote nothing"
    );
}

// AC24 (crux): a rejected EDIT leaves the existing row exactly as it was — no
// partial mutation of a valid record by an invalid update.
#[tokio::test]
async fn invalid_edit_leaves_the_record_unchanged() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            &cookie,
            &json!({ "name": "Original", "geo_label": "DE", "kind": "remote", "offered_methods": ["ping"] })
                .to_string(),
        ),
    )
    .await;
    let id = json_body(created).await["id"].as_str().unwrap().to_string();

    let rejected = send(
        central::build(state.clone()),
        authed(
            "PUT",
            &format!("/api/admin/locations/{id}"),
            &cookie,
            &json!({ "name": "", "geo_label": "DE", "kind": "remote", "offered_methods": ["ping"] })
                .to_string(),
        ),
    )
    .await;
    assert_status(&rejected, StatusCode::UNPROCESSABLE_ENTITY);

    let fetched = send(
        central::build(state),
        authed("GET", &format!("/api/admin/locations/{id}"), &cookie, ""),
    )
    .await;
    let body = json_body(fetched).await;
    assert_eq!(
        body["name"], "Original",
        "the rejected edit must not mutate the row"
    );
}

// AC24: a test IP whose address does not match its declared family is rejected
// with a field message, and nothing is written.
#[tokio::test]
async fn mismatched_ip_family_is_rejected() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            &cookie,
            &json!({ "name": "L", "geo_label": "DE", "kind": "local", "offered_methods": [] })
                .to_string(),
        ),
    )
    .await;
    let id = json_body(created).await["id"].as_str().unwrap().to_string();

    let rejected = send(
        central::build(state),
        authed(
            "POST",
            &format!("/api/admin/locations/{id}/test-ips"),
            &cookie,
            // A v6 address declared as v4.
            &json!({ "family": "v4", "address": "2001:db8::1" }).to_string(),
        ),
    )
    .await;
    assert_status(&rejected, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body_string(rejected).await.contains("family"));
}

// AC25: a settings change persists and reads back — including the exec params
// (the run-path effect of the cap is proven in run_api::from_settings).
#[tokio::test]
async fn settings_are_editable_and_persist() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    let updated = send(
        central::build(state.clone()),
        authed(
            "PUT",
            "/api/admin/settings",
            &cookie,
            &json!({
                "site_title": "My Looking Glass",
                "default_theme": "dark",
                "terms_url": "https://example.test/terms",
                "exec_max_concurrent": 4,
                "exec_timeout_secs": 20,
                "exec_max_output_kib": 128,
                "exec_rate_max": 10,
                "exec_rate_window_secs": 30
            })
            .to_string(),
        ),
    )
    .await;
    assert_status(&updated, StatusCode::OK);

    let read = send(
        central::build(state),
        authed("GET", "/api/admin/settings", &cookie, ""),
    )
    .await;
    let body = json_body(read).await;
    assert_eq!(body["site_title"], "My Looking Glass");
    assert_eq!(body["default_theme"], "dark");
    assert_eq!(body["exec_max_concurrent"], 4);
    assert_eq!(body["exec_rate_max"], 10);
}

#[tokio::test]
async fn settings_reject_non_https_branding_urls_before_persisting() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    for (field, value) in [
        ("logo_url", "http://example.test/logo.svg"),
        ("terms_url", "javascript:alert(1)"),
    ] {
        let rejected = send(
            central::build(state.clone()),
            authed(
                "PUT",
                "/api/admin/settings",
                &cookie,
                &json!({
                    "site_title": "Looking Glass",
                    "default_theme": "system",
                    field: value,
                    "exec_max_concurrent": 8,
                    "exec_timeout_secs": 30,
                    "exec_max_output_kib": 256,
                    "exec_rate_max": 20,
                    "exec_rate_window_secs": 60
                })
                .to_string(),
            ),
        )
        .await;
        assert_status(&rejected, StatusCode::UNPROCESSABLE_ENTITY);
    }

    let settings = send(
        central::build(state),
        authed("GET", "/api/admin/settings", &cookie, ""),
    )
    .await;
    let body = json_body(settings).await;
    assert_eq!(body["logo_url"], Value::Null);
    assert_eq!(body["terms_url"], Value::Null);
}

#[tokio::test]
async fn updated_run_limits_apply_without_restart() {
    let mut state = test_state();
    state.run = central::RunService::for_test(0, std::time::Duration::from_secs(30), 100);
    let cookie = setup_and_login(&state).await;

    let updated = send(
        central::build(state.clone()),
        authed(
            "PUT",
            "/api/admin/settings",
            &cookie,
            &json!({
                "site_title": "Looking Glass",
                "default_theme": "system",
                "exec_max_concurrent": 1,
                "exec_timeout_secs": 30,
                "exec_max_output_kib": 256,
                "exec_rate_max": 1,
                "exec_rate_window_secs": 60
            })
            .to_string(),
        ),
    )
    .await;
    assert_status(&updated, StatusCode::OK);

    let run = |client: &str, method: &str| {
        Request::builder()
            .uri(format!("/api/run/stream?method={method}&target=8.8.8.8"))
            .header("host", "lg.test")
            .header("origin", "https://lg.test")
            .header("x-forwarded-proto", "https")
            .header("x-forwarded-for", client)
            .extension(ConnectInfo(SocketAddr::new(TRUSTED_PROXY, 40000)))
            .body(Body::empty())
            .unwrap()
    };

    let first = send(central::build(state.clone()), run("203.0.113.10", "ping")).await;
    let busy =
        body_string(send(central::build(state.clone()), run("203.0.113.11", "ping")).await).await;
    assert!(busy.contains("node is busy"), "{busy}");
    let first = body_string(first).await;
    assert!(first.contains("event: done"), "{first}");

    let under_rate =
        body_string(send(central::build(state.clone()), run("203.0.113.12", "telnet")).await).await;
    assert!(under_rate.contains("not available"), "{under_rate}");
    let rate_limited =
        body_string(send(central::build(state), run("203.0.113.12", "telnet")).await).await;
    assert!(rate_limited.contains("too many requests"), "{rate_limited}");
}

// AC23/risk #6 end-to-end: deleting a location through the admin API cascades so
// the public catalogue no longer lists it or its children.
#[tokio::test]
async fn deleting_a_location_removes_it_from_the_public_api() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            &cookie,
            &json!({ "name": "Temp", "geo_label": "DE", "kind": "local", "offered_methods": ["ping"] })
                .to_string(),
        ),
    )
    .await;
    let id = json_body(created).await["id"].as_str().unwrap().to_string();
    send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{id}/test-ips"),
            &cookie,
            &json!({ "family": "v4", "address": "203.0.113.10" }).to_string(),
        ),
    )
    .await;

    let deleted = send(
        central::build(state.clone()),
        authed("DELETE", &format!("/api/admin/locations/{id}"), &cookie, ""),
    )
    .await;
    assert_status(&deleted, StatusCode::NO_CONTENT);

    let public = send(
        central::build(state),
        Request::builder()
            .uri("/api/locations")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let body = json_body(public).await;
    assert_eq!(
        body.as_array().unwrap().len(),
        0,
        "the deleted location is gone from public read"
    );
}
