//! Public looking-glass surface (Slice 6): the location-gated run path
//! (`runnable_methods`, AC13/binding), the detected visitor IP from the
//! trusted-proxy identity (AC19), iperf endpoints as display-only data with no
//! process spawned (AC21), and direct-from-node range file serving (AC20) with
//! path-traversal refused. Data is seeded through the admin HTTP API — the same
//! path an operator uses — then read through the public endpoints.

mod common;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};

use central::{Agent, AppState};
use common::{
    assert_status, body_string, complete_setup, secure_request, send, session_cookie, test_state,
    SETUP_TOKEN, TRUSTED_PROXY,
};

const PASSWORD: &str = "correct-horse-battery-staple";
const UNTRUSTED_PEER: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));

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

/// Create a local (online) location offering `methods`; returns its id.
async fn create_local_location(state: &AppState, cookie: &str, methods: Value) -> String {
    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            cookie,
            &json!({ "name": "Frankfurt", "geo_label": "DE", "kind": "local", "offered_methods": methods })
                .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    json_body(created).await["id"].as_str().unwrap().to_string()
}

async fn create_remote_location(state: &AppState, cookie: &str, methods: Value) -> String {
    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            cookie,
            &json!({ "name": "Remote", "geo_label": "DE", "kind": "remote", "offered_methods": methods })
                .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    json_body(created).await["id"].as_str().unwrap().to_string()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A same-origin public run GET from an untrusted peer.
fn run_get(query: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/run/stream?{query}"))
        .header("host", "localhost")
        .header("origin", "http://localhost")
        .extension(ConnectInfo(SocketAddr::new(UNTRUSTED_PEER, 50000)))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn public_settings_is_an_unauthenticated_five_field_projection_without_cache() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let location_id = create_local_location(&state, &cookie, json!(["ping"])).await;

    let updated = send(
        central::build(state.clone()),
        authed(
            "PUT",
            "/api/admin/settings",
            &cookie,
            &json!({
                "site_title": "Frankfurt Glass",
                "logo_url": "https://cdn.example.test/logo.svg",
                "default_theme": "dark",
                "terms_url": "https://example.test/terms",
                "custom_block": "Operated by Example",
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

    let locations_before = body_string(
        send(
            central::build(state.clone()),
            Request::builder()
                .uri("/api/locations")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;

    let response = send(
        central::build(state.clone()),
        Request::builder()
            .uri("/api/public/settings")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&response, StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let body = json_body(response).await;
    let fields = body.as_object().expect("public settings object");
    assert_eq!(fields.len(), 5);
    assert_eq!(body["site_title"], "Frankfurt Glass");
    assert_eq!(body["logo_url"], "https://cdn.example.test/logo.svg");
    assert_eq!(body["default_theme"], "dark");
    assert_eq!(body["terms_url"], "https://example.test/terms");
    assert_eq!(body["custom_block"], "Operated by Example");
    assert!(!body.to_string().contains("exec_max_concurrent"));

    let locations_after = body_string(
        send(
            central::build(state),
            Request::builder()
                .uri(format!("/api/locations?location={location_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(locations_after, locations_before);
}

#[tokio::test]
async fn corrupt_public_settings_fail_closed_without_cache() {
    use redb::{Database, TableDefinition};

    const SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");
    let db_path = common::temp_db_path();
    let database = Database::create(&db_path).expect("create corrupt settings database");
    let write = database
        .begin_write()
        .expect("begin corrupt settings write");
    {
        let mut settings = write.open_table(SETTINGS).expect("open settings table");
        settings
            .insert("global", &br#"{\"site_title\":\"\"}"#[..])
            .expect("write out-of-contract settings");
    }
    write.commit().expect("commit corrupt settings");
    drop(database);

    let state = common::test_state_at(db_path);
    complete_setup(&state).await;
    let response = send(
        central::build(state),
        Request::builder()
            .uri("/api/public/settings")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_status(&response, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(json_body(response).await["error"], "internal_error");
}

#[tokio::test]
async fn public_settings_enforces_the_custom_block_boundary() {
    use redb::{Database, TableDefinition};

    const SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");
    for (custom_block, expected_status) in [
        (None, StatusCode::OK),
        (Some("x".repeat(5000)), StatusCode::OK),
        (Some("x".repeat(5001)), StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let db_path = common::temp_db_path();
        let database = Database::create(&db_path).expect("create settings database");
        let write = database.begin_write().expect("begin settings write");
        {
            let mut settings = write.open_table(SETTINGS).expect("open settings table");
            let encoded = serde_json::to_vec(&json!({
                "site_title": "Looking Glass",
                "custom_block": custom_block
            }))
            .expect("encode valid stored settings");
            settings
                .insert("global", encoded.as_slice())
                .expect("write stored settings");
        }
        write.commit().expect("commit stored settings");
        drop(database);

        let state = common::test_state_at(db_path);
        complete_setup(&state).await;
        let response = send(
            central::build(state),
            Request::builder()
                .uri("/api/public/settings")
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_status(&response, expected_status);
        assert_eq!(
            response
                .headers()
                .get("cache-control")
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        let body = json_body(response).await;
        if expected_status == StatusCode::OK {
            assert_eq!(body["custom_block"], json!(custom_block));
        } else {
            assert_eq!(body["error"], "internal_error");
        }
    }
}

fn content_type(response: &axum::http::Response<Body>) -> String {
    response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

// Binding (Slice 5 doubt): the run gate uses the SELECTED location's
// runnable_methods(), not the hardcoded built-in set — a globally-valid method
// (ping) that the location does not offer is refused there.
#[tokio::test]
async fn run_is_gated_by_the_locations_runnable_methods() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    // The location offers only mtr — ping is a real method but not offered here.
    let id = create_local_location(&state, &cookie, json!(["mtr"])).await;

    let response = send(
        central::build(state),
        run_get(&format!("method=ping&target=8.8.8.8&location={id}")),
    )
    .await;
    assert_status(&response, StatusCode::OK);
    let body = body_string(response).await;
    assert!(
        body.contains("not available on this location"),
        "ping is refused at a location that offers only mtr: {body}"
    );
}

// Slice 11: an offered BGP method is now runnable and gated node-side on daemon
// presence. On a host without BIRD/FRR, the run surfaces the clear daemon-absent
// error (AC41) rather than the method-not-offered refusal — BGP is wired, not
// rejected as unavailable-method. (Fixed read-only template + grammar are proven
// in shared unit tests; daemon detection is unit-tested with an injected PATH.)
#[tokio::test]
async fn run_bgp_is_runnable_and_gated_on_daemon_presence() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let id = create_local_location(&state, &cookie, json!(["ping", "bgp"])).await;

    let response = send(
        central::build(state),
        run_get(&format!("method=bgp&target=8.8.8.8&location={id}")),
    )
    .await;
    assert_status(&response, StatusCode::OK);
    let body = body_string(response).await;
    assert!(
        !body.contains("not available on this location"),
        "BGP must no longer be refused as an unoffered method: {body}"
    );
    // No routing daemon on the test host → the clear daemon-absent error (AC41).
    // Where a daemon is installed, the run instead reaches exec and terminates.
    assert!(
        body.contains("routing daemon") || body.contains("event: done"),
        "BGP either reports no daemon or reaches execution and terminates: {body}"
    );
}

// An offered method at the selected location opens the run stream — the wired
// location path executes on the built-in local node (AC27, transport).
#[tokio::test]
async fn run_with_an_offered_method_opens_a_stream() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let id = create_local_location(&state, &cookie, json!(["ping"])).await;

    let response = send(
        central::build(state),
        run_get(&format!("method=ping&target=8.8.8.8&location={id}")),
    )
    .await;
    assert_status(&response, StatusCode::OK);
    assert!(content_type(&response).contains("text/event-stream"));
    // Drop without draining: closing the stream makes the engine kill the run.
}

// A run naming an unknown/offline location is refused with a clear message and
// no execution.
#[tokio::test]
async fn run_refuses_an_unknown_location() {
    let state = test_state();
    complete_setup(&state).await;
    let response = send(
        central::build(state),
        run_get("method=ping&target=8.8.8.8&location=deadbeefdeadbeef"),
    )
    .await;
    assert_status(&response, StatusCode::OK);
    let body = body_string(response).await;
    assert!(
        body.contains("location is not available"),
        "an unknown location is refused: {body}"
    );
}

// AC21 / FR-051: iperf endpoints are display-only — a page/data request returns
// the command strings verbatim and spawns nothing.
#[tokio::test]
async fn iperf_endpoints_are_display_only_data() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let id = create_local_location(&state, &cookie, json!(["ping"])).await;
    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{id}/iperf"),
            &cookie,
            &json!({
                "label": "Primary",
                "host": "fra.example.test",
                "port": 5201,
                "cmd_incoming": "iperf3 -c fra.example.test -p 5201",
                "cmd_outgoing": "iperf3 -c fra.example.test -p 5201 -R"
            })
            .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);

    let public = send(
        central::build(state),
        Request::builder()
            .uri("/api/locations")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let body = json_body(public).await;
    let iperf = &body.as_array().unwrap()[0]["iperf"][0];
    assert_eq!(iperf["cmd_incoming"], "iperf3 -c fra.example.test -p 5201");
    assert_eq!(
        iperf["cmd_outgoing"],
        "iperf3 -c fra.example.test -p 5201 -R"
    );
}

#[tokio::test]
async fn remote_iperf_endpoint_is_reported_as_display_only_data() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let id = create_remote_location(&state, &cookie, json!(["ping"])).await;
    state
        .store
        .put_agent(&Agent {
            id: "agent-remote".to_string(),
            location_id: id.clone(),
            credential_hash: "$argon2id$stub".to_string(),
            enrolled_at: 0,
            last_seen: Some(unix_now()),
            revoked: false,
        })
        .unwrap();
    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{id}/iperf"),
            &cookie,
            &json!({
                "label": "Remote iperf",
                "host": "remote.example.test",
                "port": 5201,
                "cmd_incoming": "iperf3 -c remote.example.test -p 5201",
                "cmd_outgoing": "iperf3 -c remote.example.test -p 5201 -R"
            })
            .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);

    let public = send(
        central::build(state),
        Request::builder()
            .uri("/api/locations")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let body = json_body(public).await;
    let iperf = &body.as_array().unwrap()[0]["iperf"][0];
    assert_eq!(iperf["host"], "remote.example.test");
    assert_eq!(
        iperf["cmd_incoming"],
        "iperf3 -c remote.example.test -p 5201"
    );
}

// AC21 / FR-051: the ONLY process-spawning path (the run endpoint → shared::exec)
// has no iperf method, so no request can spawn an iperf process — an "iperf" run
// is refused, streaming no output.
#[tokio::test]
async fn the_run_endpoint_never_spawns_iperf() {
    let state = test_state();
    complete_setup(&state).await;
    let response = send(
        central::build(state),
        run_get("method=iperf&target=8.8.8.8"),
    )
    .await;
    assert_status(&response, StatusCode::OK);
    let body = body_string(response).await;
    assert!(
        body.contains("not available"),
        "an iperf run is refused: {body}"
    );
    assert!(
        !body.contains("event: line"),
        "no command output is streamed for iperf — nothing ran: {body}"
    );
}

// AC19 / FR-042: the detected visitor IP comes from the trusted-proxy identity.
// Through a trusted proxy the forwarded client is reported; from an untrusted
// peer a spoofed forwarded header is ignored and the peer itself is reported.
#[tokio::test]
async fn visitor_ip_derives_from_the_trusted_proxy_identity() {
    let state = test_state();
    complete_setup(&state).await;

    let via_proxy = send(
        central::build(state.clone()),
        Request::builder()
            .uri("/api/visitor")
            .header("x-forwarded-for", "203.0.113.9")
            .extension(ConnectInfo(SocketAddr::new(TRUSTED_PROXY, 40000)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&via_proxy, StatusCode::OK);
    assert_eq!(json_body(via_proxy).await["ip"], "203.0.113.9");

    let spoofed = send(
        central::build(state),
        Request::builder()
            .uri("/api/visitor")
            .header("x-forwarded-for", "1.2.3.4")
            .extension(ConnectInfo(SocketAddr::new(UNTRUSTED_PEER, 50000)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        json_body(spoofed).await["ip"],
        "198.51.100.7",
        "a spoofed forwarded header from an untrusted peer is ignored"
    );
}

// AC20 / FR-050: a test file is served direct from the node and honors an HTTP
// range request — a partial fetch returns 206 with a Content-Range, and the full
// fetch returns every byte intact.
#[tokio::test]
async fn test_file_download_honors_a_range_request() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let id = create_local_location(&state, &cookie, json!(["ping"])).await;

    const CONTENT: &[u8] = b"0123456789ABCDEF";
    std::fs::write(state.files_root.join("probe.bin"), CONTENT).expect("write test file");

    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{id}/files"),
            &cookie,
            &json!({ "label": "Probe", "declared_size": "16 B", "source_ref": "probe.bin" })
                .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    let file_id = json_body(created).await["id"].as_str().unwrap().to_string();
    let url = format!("/api/locations/{id}/files/{file_id}/download");

    // Partial fetch → 206 with the exact Content-Range and only the requested bytes.
    let partial = send(
        central::build(state.clone()),
        Request::builder()
            .uri(&url)
            .header("range", "bytes=0-3")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&partial, StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        partial
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok()),
        Some("bytes 0-3/16")
    );
    assert_eq!(body_string(partial).await, "0123");

    // Full fetch → 200 with every byte intact.
    let full = send(
        central::build(state),
        Request::builder().uri(&url).body(Body::empty()).unwrap(),
    )
    .await;
    assert_status(&full, StatusCode::OK);
    assert_eq!(
        body_string(full).await.as_bytes(),
        CONTENT,
        "the full download is byte-identical"
    );
}

// security.md trust boundary: a test file whose source_ref tries to climb out of
// the served root is refused (404) — the range server never reaches outside it.
#[tokio::test]
async fn test_file_download_refuses_path_traversal() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let id = create_local_location(&state, &cookie, json!(["ping"])).await;

    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{id}/files"),
            &cookie,
            &json!({ "label": "Evil", "declared_size": "?", "source_ref": "../../../etc/passwd" })
                .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    let file_id = json_body(created).await["id"].as_str().unwrap().to_string();

    let response = send(
        central::build(state),
        Request::builder()
            .uri(format!("/api/locations/{id}/files/{file_id}/download"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&response, StatusCode::NOT_FOUND);
}
