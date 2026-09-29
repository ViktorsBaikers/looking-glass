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

use central::{AppState, EnrollConfig};
use common::{
    assert_status, authed, body_string, json_body, send, setup_and_login, temp_db_path, test_state,
    test_state_at, CENTRAL_IDENTITY, CENTRAL_URL, TRUSTED_PROXY,
};

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

// The agent serves its data plane over HTTPS on :443 with a certificate it
// obtains through TLS-ALPN-01, so only an https origin on port 443 is accepted.
#[tokio::test]
async fn remote_data_plane_origin_must_be_an_https_origin_on_port_443() {
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
        "http://remote.example.test",
        "http://remote.example.test:443",
        "https://remote.example.test:9443",
        "https://remote.example.test:80",
        "https://[2001:db8::10]:8443",
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
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "data-plane origin {origin:?} must be refused"
        );
    }

    for origin in [
        "https://remote.example.test",
        "https://remote.example.test:443",
        "https://203.0.113.10",
        "https://[2001:db8::10]",
    ] {
        let valid = send(
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
        assert_status(&valid, StatusCode::CREATED);
        assert_eq!(json_body(valid).await["data_plane_origin"], origin);
    }
}

// The certificate the remote agent reports over the tunnel reaches the
// admin location editor (issued, expires, last error), and never the public API.
#[tokio::test]
async fn the_admin_location_shows_the_remote_certificate_status() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let loc_id = create_remote_location(&state, &cookie, json!(["ping"])).await;
    let mut location = state.store.get_location(&loc_id).unwrap().unwrap();
    location.data_plane_origin = Some("https://node.example.test".to_string());
    assert!(state.store.update_location(&location).unwrap().is_some());
    state
        .store
        .put_agent(&central::Agent {
            id: "agent-cert".to_string(),
            location_id: loc_id.clone(),
            credential_hash: "$argon2id$stub".to_string(),
            enrolled_at: 0,
            last_seen: Some(unix_now()),
            revoked: false,
        })
        .unwrap();
    assert!(state
        .store
        .put_certificate_status(
            &loc_id,
            "https://node.example.test",
            &shared::protocol::CertificateStatus {
                issued_at: Some(1_790_000_000),
                expires_at: Some(1_790_518_400),
                last_error: Some("rate limited".to_string()),
            },
        )
        .unwrap());

    let admin = send(
        central::build(state.clone()),
        authed(
            "GET",
            &format!("/api/admin/locations/{loc_id}"),
            &cookie,
            "",
        ),
    )
    .await;
    assert_status(&admin, StatusCode::OK);
    assert_eq!(
        json_body(admin).await["certificate"],
        json!({"issued_at": 1_790_000_000u64, "expires_at": 1_790_518_400u64, "last_error": "rate limited"}),
        "the editor must see the agent's certificate status"
    );

    let public = send(
        central::build(state),
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
        .expect("the online remote is public");
    assert!(
        remote.get("certificate").is_none(),
        "certificate status is admin-only: {remote}"
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

// An origin stored before the https-on-443 rule (http://, or another port) is
// withheld from visitors, and the admin editor says why, so the admin re-saves it.
#[tokio::test]
async fn a_stored_origin_the_rule_now_refuses_is_withheld_and_flagged() {
    for legacy in ["http://node.example.test", "https://node.example.test:8443"] {
        let state = test_state();
        let cookie = setup_and_login(&state).await;
        let loc_id = create_remote_location(&state, &cookie, json!(["ping"])).await;
        let mut location = state.store.get_location(&loc_id).unwrap().unwrap();
        location.data_plane_origin = Some(legacy.to_string());
        assert!(state.store.update_location(&location).unwrap().is_some());
        state
            .store
            .put_agent(&central::Agent {
                id: "agent-legacy".to_string(),
                location_id: loc_id.clone(),
                credential_hash: "$argon2id$stub".to_string(),
                enrolled_at: 0,
                last_seen: Some(unix_now()),
                revoked: false,
            })
            .unwrap();
        // The agent got a certificate on 443 for the host: not a healthy origin.
        assert!(state
            .store
            .put_certificate_status(
                &loc_id,
                legacy,
                &shared::protocol::CertificateStatus {
                    issued_at: Some(1_790_000_000),
                    expires_at: Some(1_790_518_400),
                    last_error: None,
                },
            )
            .unwrap());

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
            .expect("the online remote is public");
        assert!(
            remote["data_plane_origin"].is_null(),
            "{legacy:?} must not reach visitors: {remote}"
        );

        let admin = send(
            central::build(state),
            authed(
                "GET",
                &format!("/api/admin/locations/{loc_id}"),
                &cookie,
                "",
            ),
        )
        .await;
        assert_status(&admin, StatusCode::OK);
        let admin = json_body(admin).await;
        assert_eq!(
            admin["data_plane_origin"], legacy,
            "the editor keeps the stored value to re-save"
        );
        let certificate = &admin["certificate"];
        assert!(
            certificate["issued_at"].is_null()
                && certificate["last_error"]
                    .as_str()
                    .is_some_and(|error| error.contains("Re-save")),
            "{legacy:?} must be flagged to the admin: {certificate}"
        );
    }
}

// F-215: a valid remote origin reaches visitors, or remote speed tests and files break.
#[tokio::test]
async fn a_valid_remote_origin_reaches_visitors() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let origin = "https://node.example.test";
    let created = send(
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
                "offered_methods": ["ping"]
            })
            .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    let loc_id = json_body(created).await["id"].as_str().unwrap().to_string();
    state
        .store
        .put_agent(&central::Agent {
            id: "agent-origin".to_string(),
            location_id: loc_id.clone(),
            credential_hash: "$argon2id$stub".to_string(),
            enrolled_at: 0,
            last_seen: Some(unix_now()),
            revoked: false,
        })
        .unwrap();

    let public = send(
        central::build(state),
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
        .expect("the online remote is public");
    assert_eq!(remote["data_plane_origin"], origin, "{remote}");
}

#[tokio::test]
async fn remote_test_file_source_ref_refuses_browser_normalizing_paths() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let loc_id = create_remote_location(&state, &cookie, json!(["ping"])).await;

    // The SPA refuses empty and `.` segments, so the server must too, or
    // the saved file has a dead download link.
    for source_ref in [
        "../secret.bin",
        "sub/../secret.bin",
        "/probe.bin",
        "a//b",
        "a/./b",
        "a/",
    ] {
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

// F-256: the refusal above stays narrow: a plain relative path is saved.
#[tokio::test]
async fn remote_test_file_source_ref_accepts_a_relative_path() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let loc_id = create_remote_location(&state, &cookie, json!(["ping"])).await;

    for source_ref in ["probe.bin", "sub/probe.bin"] {
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
        assert_status(&response, StatusCode::CREATED);
        assert_eq!(json_body(response).await["source_ref"], source_ref);
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

/// The reorder route reaches its handler (not `PUT /locations/{id}` with
/// id "order"), drives the public order, and refuses a stale id set.
#[tokio::test]
async fn reordering_locations_drives_the_public_order() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let mut ids = Vec::new();
    for name in ["Alpha", "Bravo", "Charlie"] {
        let created = send(
            central::build(state.clone()),
            authed(
                "POST",
                "/api/admin/locations",
                &cookie,
                &json!({ "name": name, "geo_label": "", "kind": "local", "offered_methods": ["ping"] })
                    .to_string(),
            ),
        )
        .await;
        assert_status(&created, StatusCode::CREATED);
        ids.push(json_body(created).await["id"].as_str().unwrap().to_string());
    }

    let order = json!({ "ids": [&ids[2], &ids[0], &ids[1]] }).to_string();
    let reordered = send(
        central::build(state.clone()),
        authed("PUT", "/api/admin/locations/order", &cookie, &order),
    )
    .await;
    assert_status(&reordered, StatusCode::NO_CONTENT);
    assert_eq!(public_names(&state).await, ["Charlie", "Alpha", "Bravo"]);

    let stale = json!({ "ids": [&ids[0], &ids[1]] }).to_string();
    let refused = send(
        central::build(state.clone()),
        authed("PUT", "/api/admin/locations/order", &cookie, &stale),
    )
    .await;
    assert_status(&refused, StatusCode::CONFLICT);
    assert_eq!(json_body(refused).await["error"], "stale_order");
    assert_eq!(public_names(&state).await, ["Charlie", "Alpha", "Bravo"]);
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
    let stored = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{id}/test-ips"),
            &cookie,
            &json!({ "family": "v4", "address": "192.0.2.1" }).to_string(),
        ),
    )
    .await;
    assert_status(&stored, StatusCode::CREATED);
    let stored = json_body(stored).await;
    let stored_id = stored["id"].as_str().unwrap();

    // A v6 address declared as v4, as a new row and as an edit of the stored one.
    let mismatched = json!({ "family": "v4", "address": "2001:db8::1" }).to_string();
    for (method, path) in [
        ("POST", format!("/api/admin/locations/{id}/test-ips")),
        ("PUT", format!("/api/admin/test-ips/{stored_id}")),
    ] {
        let rejected = send(
            central::build(state.clone()),
            authed(method, &path, &cookie, &mismatched),
        )
        .await;
        assert_status(&rejected, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body_string(rejected).await.contains("family"));
    }

    assert_eq!(
        serde_json::to_value(state.store.list_test_ips(&id).unwrap()).unwrap(),
        json!([stored]),
        "a refused test IP must leave the stored test IPs unchanged"
    );
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
    assert_eq!(body["terms_url"], "https://example.test/terms");
    assert_eq!(body["exec_max_concurrent"], 4);
    assert_eq!(body["exec_timeout_secs"], 20);
    assert_eq!(body["exec_max_output_kib"], 128);
    assert_eq!(body["exec_rate_max"], 10);
    assert_eq!(body["exec_rate_window_secs"], 30);
}

#[tokio::test]
async fn settings_reject_non_https_branding_urls_before_persisting() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    for (field, value) in [
        ("logo_url", "http://example.test/logo.svg"),
        ("terms_url", "javascript:alert(1)"),
        // The strict check the SPA and enroll share.
        ("logo_url", "https://cdn.example.test:99999/logo.svg"),
        ("terms_url", "https://[not-ip]/terms"),
        // Shapes http's parser lets through and the SPA's URL parser refuses.
        ("logo_url", "https://[2001:db8::1]x:443/logo.png"),
        ("terms_url", "https://x[::1]:443/terms"),
        ("logo_url", "https://cdn.example.test:+443/logo.svg"),
        ("terms_url", "https://256.1.1.1/terms"),
        // Only IPv6 goes inside brackets.
        ("logo_url", "https://[192.0.2.1]/logo.png"),
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
        central::build(state.clone()),
        authed("GET", "/api/admin/settings", &cookie, ""),
    )
    .await;
    let body = json_body(settings).await;
    assert_eq!(body["logo_url"], Value::Null);
    assert_eq!(body["terms_url"], Value::Null);

    let accepted = send(
        central::build(state),
        authed(
            "PUT",
            "/api/admin/settings",
            &cookie,
            &json!({
                "site_title": "Looking Glass",
                "default_theme": "system",
                "logo_url": "https://[2001:db8::1]:8443/logo.png",
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
    assert_status(&accepted, StatusCode::OK);
}

#[tokio::test]
async fn updated_run_limits_apply_without_restart() {
    let mut state = test_state();
    state.run = central::RunService::for_test(0, std::time::Duration::from_secs(30), 100);
    let cookie = setup_and_login(&state).await;
    // A run without `location` is gated on the local location's offered set (F-212).
    let local = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            &cookie,
            &json!({"name": "Local", "geo_label": "DE", "kind": "local", "offered_methods": ["ping"]})
                .to_string(),
        ),
    )
    .await;
    assert_status(&local, StatusCode::CREATED);

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

// Spec #1 Location schema: an optional ASN (1–4294967295) round-trips through
// create/read/update and clears back to null; it is exposed in the admin payload.
#[tokio::test]
async fn asn_round_trips_and_clears() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            &cookie,
            &json!({ "name": "Vienna", "geo_label": "AT", "kind": "local", "offered_methods": ["ping"], "asn": 64500 })
                .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    let body = json_body(created).await;
    assert_eq!(body["asn"], json!(64500));
    let id = body["id"].as_str().unwrap().to_string();

    let fetched = send(
        central::build(state.clone()),
        authed("GET", &format!("/api/admin/locations/{id}"), &cookie, ""),
    )
    .await;
    assert_eq!(json_body(fetched).await["asn"], json!(64500));

    // The top of the range is valid.
    let boundary = send(
        central::build(state.clone()),
        authed(
            "PUT",
            &format!("/api/admin/locations/{id}"),
            &cookie,
            &json!({ "name": "Vienna", "geo_label": "AT", "kind": "local", "offered_methods": ["ping"], "asn": 4_294_967_295u64 })
                .to_string(),
        ),
    )
    .await;
    assert_status(&boundary, StatusCode::OK);
    assert_eq!(json_body(boundary).await["asn"], json!(4_294_967_295u64));

    // Omitting the field on update clears it (old rows / no ASN configured).
    let cleared = send(
        central::build(state),
        authed(
            "PUT",
            &format!("/api/admin/locations/{id}"),
            &cookie,
            &json!({ "name": "Vienna", "geo_label": "AT", "kind": "local", "offered_methods": ["ping"] })
                .to_string(),
        ),
    )
    .await;
    assert_status(&cleared, StatusCode::OK);
    assert_eq!(json_body(cleared).await["asn"], Value::Null);
}

// Spec #1 / AC of issue #4: an invalid ASN — 0, negative, beyond u32, a float,
// or a non-number — is refused with 400 `invalid_asn` and writes nothing.
#[tokio::test]
async fn invalid_asn_values_are_refused_with_invalid_asn() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    for invalid in [
        json!(0),
        json!(-1),
        json!(4_294_967_296u64),
        json!(64500.5),
        json!("64500"),
    ] {
        let rejected = send(
            central::build(state.clone()),
            authed(
                "POST",
                "/api/admin/locations",
                &cookie,
                &json!({ "name": "Bad", "geo_label": "AT", "kind": "local", "offered_methods": [], "asn": invalid })
                    .to_string(),
            ),
        )
        .await;
        assert_status(&rejected, StatusCode::BAD_REQUEST);
        let body = json_body(rejected).await;
        assert_eq!(body["error"], "invalid_asn", "asn {invalid}");
    }

    let list = send(
        central::build(state),
        authed("GET", "/api/admin/locations", &cookie, ""),
    )
    .await;
    assert_eq!(
        json_body(list).await.as_array().unwrap().len(),
        0,
        "a rejected ASN wrote nothing"
    );
}

// The editor reads the detail route, so it must carry last_seen like the
// list does, or an enrolled-but-offline remote shows "Not enrolled".
#[tokio::test]
async fn location_detail_carries_last_seen() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let loc_id = create_remote_location(&state, &cookie, json!(["ping"])).await;
    state
        .store
        .put_agent(&central::Agent {
            id: "agent-lapsed".to_string(),
            location_id: loc_id.clone(),
            credential_hash: "$argon2id$stub".to_string(),
            enrolled_at: 0,
            last_seen: Some(1000),
            revoked: false,
        })
        .unwrap();

    let detail = send(
        central::build(state),
        authed(
            "GET",
            &format!("/api/admin/locations/{loc_id}"),
            &cookie,
            "",
        ),
    )
    .await;
    assert_status(&detail, StatusCode::OK);
    let body = json_body(detail).await;
    assert_eq!(body["status"], "offline");
    assert_eq!(body["last_seen"], json!(1000));
}

// facility_url is a public link href, so it gets the same strict
// https check as the logo and terms URLs, on create and on edit.
#[tokio::test]
async fn facility_url_must_be_a_strict_https_url() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let location = |facility_url: &str| {
        json!({
            "name": "Frankfurt",
            "geo_label": "DE",
            "kind": "local",
            "offered_methods": ["ping"],
            "facility_url": facility_url
        })
        .to_string()
    };

    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            &cookie,
            &location("https://dc.example.test/fra1"),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    let id = json_body(created).await["id"].as_str().unwrap().to_string();

    for bad in [
        "javascript:alert(document.domain)",
        "http://dc.example.test/fra1",
        "https://dc.example.test:99999/fra1",
        "https://[not-ip]/fra1",
        "https://[2001:db8::1]x:443/fra1",
        "https://x[::1]:443/fra1",
        "https://dc.example.test:+443/fra1",
        "https://dc.example.123/fra1",
        "https://[192.0.2.1]/fra1",
    ] {
        for (method, uri) in [
            ("POST", "/api/admin/locations".to_string()),
            ("PUT", format!("/api/admin/locations/{id}")),
        ] {
            let rejected = send(
                central::build(state.clone()),
                authed(method, &uri, &cookie, &location(bad)),
            )
            .await;
            assert_status(&rejected, StatusCode::UNPROCESSABLE_ENTITY);
        }
    }

    let list = send(
        central::build(state.clone()),
        authed("GET", "/api/admin/locations", &cookie, ""),
    )
    .await;
    let list = json_body(list).await;
    assert_eq!(
        list.as_array().unwrap().len(),
        1,
        "no rejected create wrote"
    );
    assert_eq!(list[0]["facility_url"], "https://dc.example.test/fra1");

    let bracketed_v6 = send(
        central::build(state),
        authed(
            "PUT",
            &format!("/api/admin/locations/{id}"),
            &cookie,
            &location("https://[2001:db8::1]:8443/fra1"),
        ),
    )
    .await;
    assert_status(&bracketed_v6, StatusCode::OK);
}

// The enroll asset URLs share the strict check, so a bracketed IPv4
// host is refused there too while a bracketed IPv6 host with a port is not.
#[tokio::test]
async fn enrollment_asset_urls_accept_only_ipv6_in_brackets() {
    for (agent_url, expected) in [
        (
            "https://[192.0.2.1]/lg-agent",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        ("https://[2001:db8::1]:8443/lg-agent", StatusCode::OK),
    ] {
        let mut state = test_state();
        state.enroll = EnrollConfig::for_test_with_agent(
            CENTRAL_URL,
            CENTRAL_IDENTITY.to_vec(),
            agent_url,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "https://downloads.example/install-agent.sh",
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        );
        let cookie = setup_and_login(&state).await;
        let id = create_remote_location(&state, &cookie, json!([])).await;
        let response = send(
            central::build(state),
            authed(
                "POST",
                &format!("/api/admin/locations/{id}/enroll"),
                &cookie,
                "",
            ),
        )
        .await;
        assert_status(&response, expected);
    }
}

// A body the JSON extractor refuses answers the {error, message}
// envelope the SPA parses, keeping axum's status.
#[tokio::test]
async fn body_extractor_rejections_answer_the_json_envelope() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let loc_id = create_remote_location(&state, &cookie, json!(["ping"])).await;

    for (uri, body, status) in [
        (
            format!("/api/admin/locations/{loc_id}/iperf"),
            json!({ "label": "x", "host": "h", "port": 70000, "cmd_incoming": "a", "cmd_outgoing": "b" })
                .to_string(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "/api/admin/locations".to_string(),
            "{".to_string(),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let rejected = send(
            central::build(state.clone()),
            authed("POST", &uri, &cookie, &body),
        )
        .await;
        assert_status(&rejected, status);
        assert_eq!(
            rejected
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/json"),
            "{uri}"
        );
        let body = json_body(rejected).await;
        assert_eq!(body["error"], "invalid_input");
        assert!(body["message"].as_str().is_some_and(|m| !m.is_empty()));
    }
}

// A child edit that answers 200 must also persist: the edit/delete race tests
// only expect 404, so a row check that refused every live child would pass them.
#[tokio::test]
async fn child_edits_answer_ok_and_persist() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            &cookie,
            &json!({ "name": "Frankfurt", "geo_label": "DE", "kind": "local", "offered_methods": [] })
                .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    let loc_id = json_body(created).await["id"].as_str().unwrap().to_string();

    for (route, list, create, edit, field, edited) in [
        (
            "test-ips",
            "test_ips",
            json!({ "family": "v4", "address": "203.0.113.10", "label": "a" }),
            json!({ "family": "v4", "address": "203.0.113.20", "label": "a" }),
            "address",
            "203.0.113.20",
        ),
        (
            "iperf",
            "iperf",
            json!({ "label": "a", "host": "h1", "port": 5201, "cmd_incoming": "i", "cmd_outgoing": "o" }),
            json!({ "label": "a", "host": "h2", "port": 5201, "cmd_incoming": "i", "cmd_outgoing": "o" }),
            "host",
            "h2",
        ),
        (
            "files",
            "files",
            json!({ "label": "a", "declared_size": "1 MB", "source_ref": "one.bin" }),
            json!({ "label": "b", "declared_size": "1 MB", "source_ref": "one.bin" }),
            "label",
            "b",
        ),
    ] {
        let created = send(
            central::build(state.clone()),
            authed(
                "POST",
                &format!("/api/admin/locations/{loc_id}/{route}"),
                &cookie,
                &create.to_string(),
            ),
        )
        .await;
        assert_status(&created, StatusCode::CREATED);
        let id = json_body(created).await["id"].as_str().unwrap().to_string();

        let edited_response = send(
            central::build(state.clone()),
            authed(
                "PUT",
                &format!("/api/admin/{route}/{id}"),
                &cookie,
                &edit.to_string(),
            ),
        )
        .await;
        assert_status(&edited_response, StatusCode::OK);
        let body = json_body(edited_response).await;
        assert_eq!(body["id"], id.as_str(), "{route}: edit keeps the id");
        assert_eq!(body[field], edited, "{route}: edit answers the new value");

        let detail = send(
            central::build(state.clone()),
            authed(
                "GET",
                &format!("/api/admin/locations/{loc_id}"),
                &cookie,
                "",
            ),
        )
        .await;
        let detail = json_body(detail).await;
        let rows = detail[list].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{route}: the edit replaced, not added");
        assert_eq!(rows[0]["id"], id.as_str(), "{route}: same row");
        assert_eq!(rows[0][field], edited, "{route}: the edit persisted");
    }
}
