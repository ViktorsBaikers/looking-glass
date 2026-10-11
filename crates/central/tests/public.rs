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
    assert_status, authed, body_string, captured_logs, complete_setup, json_body, send,
    setup_and_login, test_state, TRUSTED_PROXY,
};

const UNTRUSTED_PEER: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));

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
    // Each row is valid JSON that parses into GlobalSettings, so only
    // the read-side contract check can refuse it.
    for row in [
        json!({ "site_title": "" }),
        json!({ "site_title": "Looking Glass", "logo_url": "http://cdn.example.test/logo.svg" }),
    ] {
        let db_path = common::temp_db_path();
        let database = Database::create(&db_path).expect("create corrupt settings database");
        let write = database
            .begin_write()
            .expect("begin corrupt settings write");
        {
            let mut settings = write.open_table(SETTINGS).expect("open settings table");
            settings
                .insert("global", row.to_string().as_bytes())
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
        assert_eq!(
            json_body(response).await["error"],
            "internal_error",
            "{row}"
        );
    }
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

// A logo or terms URL saved under the older https check but refused by
// the strict one is served as null; the rest of the branding stays intact.
#[tokio::test]
async fn public_settings_drop_stored_urls_the_strict_check_refuses() {
    use redb::{Database, TableDefinition};

    const SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");
    const GOOD: &str = "https://cdn.example.test/ok";
    for bad in [
        "https://1.2.3/logo.png",
        "https://cdn.example.com:/logo.svg",
        "https://cdn.example.com:70000/logo.svg",
    ] {
        for (field, other) in [("logo_url", "terms_url"), ("terms_url", "logo_url")] {
            let db_path = common::temp_db_path();
            let database = Database::create(&db_path).expect("create settings database");
            let write = database.begin_write().expect("begin settings write");
            {
                let mut settings = write.open_table(SETTINGS).expect("open settings table");
                let row = json!({
                    "site_title": "Legacy Glass",
                    "default_theme": "dark",
                    "custom_block": "hello",
                    field: bad,
                    other: GOOD
                });
                settings
                    .insert("global", row.to_string().as_bytes())
                    .expect("write legacy settings");
            }
            write.commit().expect("commit legacy settings");
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

            assert_status(&response, StatusCode::OK);
            let body = json_body(response).await;
            assert_eq!(body[field], Value::Null, "{field} = {bad}");
            assert_eq!(body[other], GOOD, "{field} = {bad}");
            assert_eq!(body["site_title"], "Legacy Glass");
            assert_eq!(body["default_theme"], "dark");
            assert_eq!(body["custom_block"], "hello");
        }
    }

    // An overlong stored URL was never in contract, so it still fails closed.
    let long = |max: usize| format!("https://cdn.example.test/{}", "a".repeat(max));
    for (field, url) in [("logo_url", long(500)), ("terms_url", long(300))] {
        let db_path = common::temp_db_path();
        let database = Database::create(&db_path).expect("create settings database");
        let write = database.begin_write().expect("begin settings write");
        {
            let mut settings = write.open_table(SETTINGS).expect("open settings table");
            let row = json!({ "site_title": "Legacy Glass", field: url });
            settings
                .insert("global", row.to_string().as_bytes())
                .expect("write overlong settings");
        }
        write.commit().expect("commit overlong settings");
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

// F-257: a remote that is offline, or online with no connected agent, is
// refused exactly like an unknown id, so a visitor cannot tell them apart.
#[tokio::test]
async fn run_refuses_an_unavailable_remote_like_an_unknown_location() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let id = create_remote_location(&state, &cookie, json!(["ping"])).await;
    let unknown = run_body(
        &state,
        "method=ping&target=8.8.8.8&location=deadbeefdeadbeef",
    )
    .await;
    assert!(unknown.contains("location is not available"), "{unknown}");

    // A stale heartbeat leaves the remote offline; a fresh one makes it
    // online, but no tunnel is connected.
    for last_seen in [Some(0), Some(unix_now())] {
        state
            .store
            .put_agent(&Agent {
                id: "agent-remote".to_string(),
                location_id: id.clone(),
                credential_hash: "$argon2id$stub".to_string(),
                enrolled_at: 0,
                last_seen,
                revoked: false,
            })
            .unwrap();
        let body = run_body(&state, &format!("method=ping&target=8.8.8.8&location={id}")).await;
        assert_eq!(body, unknown, "last_seen {last_seen:?}");
    }
}

/// The run stream's body, or a marker when it is still open after 5 s (the run started).
async fn run_body(state: &AppState, query: &str) -> String {
    let response = send(central::build(state.clone()), run_get(query)).await;
    tokio::time::timeout(std::time::Duration::from_secs(5), body_string(response))
        .await
        .unwrap_or_else(|_| "<run still streaming after 5s>".to_string())
}

// F-212: a run without `location` is gated on the local location's offered set,
// exactly like a run naming it; it never falls back to a built-in method set.
#[tokio::test]
async fn run_without_location_is_gated_like_the_local_location() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let id = create_local_location(&state, &cookie, json!(["ping"])).await;
    // A remote's offering never widens what runs on central's own host, even
    // while it is online (F-258).
    let remote = create_remote_location(&state, &cookie, json!(["mtr"])).await;
    state
        .store
        .put_agent(&Agent {
            id: "agent-remote".to_string(),
            location_id: remote,
            credential_hash: "$argon2id$stub".to_string(),
            enrolled_at: 0,
            last_seen: Some(unix_now()),
            revoked: false,
        })
        .unwrap();

    let named = run_body(&state, &format!("method=mtr&target=8.8.8.8&location={id}")).await;
    assert!(named.contains("not available on this location"), "{named}");
    let bare = run_body(&state, "method=mtr&target=8.8.8.8").await;
    assert!(
        bare.contains("not available on this location"),
        "mtr without a location must be refused like mtr at the ping-only local location: {bare}"
    );
}

// F-212: with no local location configured, a run without `location` is refused.
#[tokio::test]
async fn run_without_location_is_refused_when_no_local_location_exists() {
    let state = test_state();
    complete_setup(&state).await;
    let bare = run_body(&state, "method=traceroute&target=8.8.8.8").await;
    assert!(
        bare.contains("not available on this location"),
        "a remote-only deployment must not run diagnostics on central: {bare}"
    );
}

// F-183: the visitor's method and location are logged as quoted values, so an
// encoded newline cannot start a forged log line, on the cross-origin refusal
// and on an in-band refusal alike.
#[tokio::test]
async fn run_log_fields_cannot_forge_a_log_line() {
    let logs = captured_logs();
    let state = test_state();
    complete_setup(&state).await;
    let query = "method=ping%0Aforged183m+admin_id%3Dx&target=8.8.8.8&location=L183%0Aforged183l+admin_id%3Dx";

    let mut cross_origin = run_get(query);
    cross_origin.headers_mut().remove("origin");
    let refused = send(central::build(state.clone()), cross_origin).await;
    assert_status(&refused, StatusCode::FORBIDDEN);
    let in_band = send(central::build(state), run_get(query)).await;
    assert_status(&in_band, StatusCode::OK);

    let captured = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(
        captured.contains(r#"method="ping\nforged183m admin_id=x""#)
            && captured.contains(r#"location="L183\nforged183l admin_id=x""#),
        "the visitor's values must be logged quoted and escaped\n{captured}"
    );
    assert!(
        !captured.lines().any(|line| line.starts_with("forged183")),
        "a visitor value started a forged log line\n{captured}"
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

    // A real file one level above the root, so an unconfined join would
    // serve it (200) wherever TMPDIR sits.
    let root_name = state.files_root.file_name().unwrap().to_str().unwrap();
    let secret_name = format!("{root_name}-outside.txt");
    std::fs::write(
        state.files_root.parent().unwrap().join(&secret_name),
        b"outside the root",
    )
    .expect("write file outside the root");

    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{id}/files"),
            &cookie,
            &json!({ "label": "Evil", "declared_size": "?", "source_ref": format!("../{secret_name}") })
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

// Spec #1 Location schema: the optional ASN is exposed in the public payload —
// a number where set, null where absent (old rows / no ASN configured).
#[tokio::test]
async fn public_locations_expose_the_optional_asn() {
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
    let _with_asn = create_local_location(&state, &cookie, json!(["ping"])).await;

    let public = send(
        central::build(state),
        Request::builder()
            .uri("/api/locations")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let body = json_body(public).await;
    let locations = body.as_array().unwrap();
    let vienna = locations.iter().find(|l| l["name"] == "Vienna").unwrap();
    assert_eq!(vienna["asn"], json!(64500));
    let frankfurt = locations.iter().find(|l| l["name"] == "Frankfurt").unwrap();
    assert_eq!(
        frankfurt["asn"],
        Value::Null,
        "a location without an ASN exposes null, never omits the field"
    );
}

// ----- Browser speed-test upload sink (spec #1) ---------------------------------

/// A same-origin public upload POST from an untrusted peer.
fn upload_request(location: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/api/locations/{location}/speedtest/upload"))
        .header("host", "localhost")
        .header("origin", "http://localhost")
        .extension(ConnectInfo(SocketAddr::new(UNTRUSTED_PEER, 50000)))
        .body(Body::from(body))
        .unwrap()
}

// Spec #1 upload sink: a local-node upload streams, is discarded, and reports
// the received byte count; a remote or unknown location has no central sink (404).
#[tokio::test]
async fn upload_sink_discards_the_body_and_reports_its_size() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let local = create_local_location(&state, &cookie, json!(["ping"])).await;
    let remote = create_remote_location(&state, &cookie, json!(["ping"])).await;

    let uploaded = send(
        central::build(state.clone()),
        upload_request(&local, vec![7u8; 4096]),
    )
    .await;
    assert_status(&uploaded, StatusCode::OK);
    assert_eq!(json_body(uploaded).await, json!({ "bytes": 4096 }));

    // The sink is the local node's: a remote location uploads to its agent's
    // data-plane, never to central.
    let remote_refused = send(
        central::build(state.clone()),
        upload_request(&remote, vec![7u8; 16]),
    )
    .await;
    assert_status(&remote_refused, StatusCode::NOT_FOUND);

    let unknown = send(
        central::build(state),
        upload_request("ghost", vec![7u8; 16]),
    )
    .await;
    assert_status(&unknown, StatusCode::NOT_FOUND);
}

// Spec #1 upload sink: beyond the 25 MB cap the upload is refused with 413.
#[tokio::test]
async fn upload_sink_refuses_more_than_25_mb() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let local = create_local_location(&state, &cookie, json!(["ping"])).await;

    let oversized = send(
        central::build(state),
        upload_request(&local, vec![0u8; 25 * 1024 * 1024 + 1]),
    )
    .await;
    assert_status(&oversized, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json_body(oversized).await["error"], "payload_too_large");
}

// Spec #1 upload sink: uploads count against the run rate limiter — with a
// one-request window the second upload is refused 429.
#[tokio::test]
async fn upload_sink_counts_against_the_run_rate_limit() {
    let mut state = test_state();
    state.run = central::RunService::for_test(8, std::time::Duration::from_secs(30), 1);
    let cookie = setup_and_login(&state).await;
    let local = create_local_location(&state, &cookie, json!(["ping"])).await;

    let first = send(
        central::build(state.clone()),
        upload_request(&local, vec![1u8; 64]),
    )
    .await;
    assert_status(&first, StatusCode::OK);

    let second = send(central::build(state), upload_request(&local, vec![1u8; 64])).await;
    assert_status(&second, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(json_body(second).await["error"], "rate_limited");
}

// F-152: the upload sink's permits are shared per client, keyed like the rate
// limiter (the trusted proxy's forwarded address): one visitor holding stalled
// uploads cannot take every permit, so another visitor behind the same proxy
// still uploads.
#[tokio::test]
async fn one_client_cannot_hold_every_upload_permit() {
    use futures_util::StreamExt;
    use tower::ServiceExt;

    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let local = create_local_location(&state, &cookie, json!(["ping"])).await;
    let app = central::build(state);
    let upload = |client: &str, body: Body| {
        Request::builder()
            .method("POST")
            .uri(format!("/api/locations/{local}/speedtest/upload"))
            .header("host", "localhost")
            .header("origin", "http://localhost")
            .header("x-forwarded-for", client)
            .extension(ConnectInfo(SocketAddr::new(TRUSTED_PROXY, 50000)))
            .body(body)
            .unwrap()
    };

    // Four uploads from one client, each sending one byte and then stalling;
    // a held one signals once the sink starts reading its body.
    let mut held = Vec::new();
    for _ in 0..4 {
        let (started, reading) = tokio::sync::oneshot::channel::<()>();
        let body = futures_util::stream::once(async move {
            let _ = started.send(());
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"x"))
        })
        .chain(futures_util::stream::pending());
        let mut task = tokio::spawn(
            app.clone()
                .oneshot(upload("203.0.113.66", Body::from_stream(body))),
        );
        tokio::select! {
            _ = reading => held.push(task),
            refused = &mut task => {
                assert_status(&refused.unwrap().unwrap(), StatusCode::TOO_MANY_REQUESTS)
            }
        }
    }

    let other = send(
        app.clone(),
        upload("198.51.100.9", Body::from(vec![7u8; 1024])),
    )
    .await;
    assert_status(&other, StatusCode::OK);

    // Ending the stalled uploads returns the client's share.
    for task in held {
        task.abort();
        let _ = task.await;
    }
    let again = send(app, upload("203.0.113.66", Body::from(vec![7u8; 1024]))).await;
    assert_status(&again, StatusCode::OK);
}

// Central serves only its local node's files. Converting a local location
// to remote hides it from the catalogue, so its download route must 404 too.
#[tokio::test]
async fn a_location_converted_to_remote_stops_serving_central_files() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let id = create_local_location(&state, &cookie, json!(["ping"])).await;
    std::fs::write(state.files_root.join("probe.bin"), b"0123").expect("write test file");

    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{id}/files"),
            &cookie,
            &json!({ "label": "Probe", "declared_size": "4 B", "source_ref": "probe.bin" })
                .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    let file_id = json_body(created).await["id"].as_str().unwrap().to_string();
    let url = format!("/api/locations/{id}/files/{file_id}/download");
    let download = || Request::builder().uri(&url).body(Body::empty()).unwrap();

    let served = send(central::build(state.clone()), download()).await;
    assert_status(&served, StatusCode::OK);

    let converted = send(
        central::build(state.clone()),
        authed(
            "PUT",
            &format!("/api/admin/locations/{id}"),
            &cookie,
            &json!({ "name": "Frankfurt", "geo_label": "DE", "kind": "remote", "offered_methods": ["ping"] })
                .to_string(),
        ),
    )
    .await;
    assert_status(&converted, StatusCode::OK);

    let hidden = send(central::build(state), download()).await;
    assert_status(&hidden, StatusCode::NOT_FOUND);
}
