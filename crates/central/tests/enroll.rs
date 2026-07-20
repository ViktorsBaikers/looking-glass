//! Agent enrollment integration (Slice 7): token single-use + expiry, credential
//! issued once and hashed at rest, cleartext refusal, the install command carrying
//! central's fingerprint with no manual edit, the protocol version in the handshake,
//! and no plaintext token/credential in any log line. The agent-side pin check lives
//! in `crates/agent/tests/enroll.rs`; this file proves the central surface.

mod common;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header::COOKIE, Request, StatusCode};
use serde_json::{json, Value};
use std::net::SocketAddr;

use central::{AppState, EnrollConfig, EnrollmentToken};
use common::{
    assert_status, body_string, captured_logs, cleartext_request, secure_request, send,
    session_cookie, test_state, CENTRAL_IDENTITY, CENTRAL_URL, SETUP_TOKEN, TRUSTED_PROXY,
};
use shared::protocol::{fingerprint, sha256_hex, PROTOCOL_VERSION};

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

fn authed(method: &str, uri: &str, cookie: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-forwarded-proto", "https")
        .header("cookie", cookie)
        .extension(ConnectInfo(SocketAddr::new(TRUSTED_PROXY, 40000)))
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn cleartext_authed(method: &str, uri: &str, cookie: &str, body: &str) -> Request<Body> {
    let mut request = cleartext_request(method, uri, body);
    request
        .headers_mut()
        .insert(COOKIE, cookie.parse().expect("valid session cookie"));
    request
}

async fn json_body(response: axum::http::Response<Body>) -> Value {
    serde_json::from_str(&body_string(response).await).expect("json body")
}

/// Create a remote location and mint an enrollment ticket for it. Returns the ticket
/// JSON and the location id.
async fn create_remote_and_ticket(state: &AppState, cookie: &str) -> (Value, String) {
    let location_id = create_remote_location(state, cookie).await;
    let ticket = request_ticket(state, cookie, &location_id).await;
    assert_status(&ticket, StatusCode::OK);
    (json_body(ticket).await, location_id)
}

async fn create_remote_location(state: &AppState, cookie: &str) -> String {
    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            cookie,
            &json!({ "name": "Remote", "geo_label": "DE", "kind": "remote", "offered_methods": [] })
                .to_string(),
        ),
    )
    .await;
    assert_status(&created, StatusCode::CREATED);
    json_body(created).await["id"].as_str().unwrap().to_string()
}

async fn request_ticket(
    state: &AppState,
    cookie: &str,
    location_id: &str,
) -> axum::http::Response<Body> {
    send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{location_id}/enroll"),
            cookie,
            "",
        ),
    )
    .await
}

// FR-071: even an authenticated admin must not receive a ticket unless the
// configured trusted proxy attests the external request was TLS.
#[tokio::test]
async fn cleartext_ticket_issuance_is_refused_before_a_ticket_is_exposed() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let location_id = create_remote_location(&state, &cookie).await;

    let refused = send(
        central::build(state.clone()),
        cleartext_authed(
            "POST",
            &format!("/api/admin/locations/{location_id}/enroll"),
            &cookie,
            "",
        ),
    )
    .await;

    assert_status(&refused, StatusCode::FORBIDDEN);
    let body = json_body(refused).await;
    assert_eq!(body["error"], "insecure_transport");
    assert!(
        body.get("token").is_none(),
        "refused response exposes no ticket"
    );
    assert!(
        body.get("install_command").is_none(),
        "refused response exposes no install command"
    );
    assert_eq!(
        state.enrollment_token_count(&location_id).unwrap(),
        0,
        "cleartext refusal must not persist a ticket"
    );

    let issued = request_ticket(&state, &cookie, &location_id).await;
    assert_status(&issued, StatusCode::OK);
    let ticket = json_body(issued).await;
    assert!(
        ticket["token"].as_str().is_some(),
        "secure request mints a ticket"
    );
    assert_eq!(
        state.enrollment_token_count(&location_id).unwrap(),
        1,
        "the secure request persists exactly one ticket"
    );
}

#[tokio::test]
async fn cleartext_ticket_refusal_precedes_location_lookup() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    let refused = send(
        central::build(state),
        cleartext_authed(
            "POST",
            "/api/admin/locations/not-a-location/enroll",
            &cookie,
            "",
        ),
    )
    .await;

    assert_status(&refused, StatusCode::FORBIDDEN);
    assert_eq!(json_body(refused).await["error"], "insecure_transport");
}

fn enroll_request(token: &str, version: u16, secure: bool) -> Request<Body> {
    let body = json!({ "protocol_version": version, "token": token }).to_string();
    if secure {
        secure_request("POST", "/api/enroll", &body)
    } else {
        cleartext_request("POST", "/api/enroll", &body)
    }
}

// AC7 (token half + credential half) + FR-021: the ticket carries a no-edit install
// command embedding central's real fingerprint, and enrolling with the token issues
// an agent credential over a versioned handshake.
#[tokio::test]
async fn a_valid_token_enrolls_and_issues_a_versioned_credential() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let (ticket, location_id) = create_remote_and_ticket(&state, &cookie).await;

    let token = ticket["token"].as_str().unwrap().to_string();
    let install_command = ticket["install_command"].as_str().unwrap();
    let agent_sha256 = ticket["agent_sha256"].as_str().unwrap();
    // The install command embeds central's fingerprint (over the trusted channel).
    assert_eq!(ticket["fingerprint"], fingerprint(CENTRAL_IDENTITY));
    assert!(install_command.contains(&fingerprint(CENTRAL_IDENTITY)));
    assert!(
        install_command.contains(&token),
        "command carries the token"
    );
    assert!(
        install_command.contains(agent_sha256),
        "command carries the expected agent checksum"
    );
    assert!(
        install_command.contains("https://downloads.example/lg-agent"),
        "command carries the configured agent binary URL"
    );
    assert!(
        install_command.contains("https://downloads.example/install-agent.sh"),
        "command carries the configured install script URL"
    );
    assert!(
        install_command.contains(ticket["install_script_sha256"].as_str().unwrap()),
        "command carries the expected installer checksum"
    );
    assert!(
        install_command.contains("sha256sum -c -"),
        "command verifies the installer before executing it"
    );
    assert!(
        install_command.contains("sudo env -i"),
        "normal non-root paste path must escalate with a scrubbed installer env: {install_command}"
    );
    assert!(
        !install_command.contains("sudo -E"),
        "normal paste path must not preserve ambient operator env: {install_command}"
    );
    assert!(
        install_command.contains("if [ \"$(id -u)\" -eq 0 ]; then env -i"),
        "root paste path must not require sudo and must scrub ambient env: {install_command}"
    );
    assert!(
        install_command.contains("workdir=$(mktemp -d -t lookingglass-agent.XXXXXXXXXX)"),
        "installer download must happen inside the root execution context: {install_command}"
    );
    assert!(
        install_command.contains("[ \"$(stat -c %u \"$workdir\")\" = 0 ]"),
        "root workflow must prove the temp path is root-owned: {install_command}"
    );
    assert!(
        !install_command.contains("curl -fsSL https://downloads.example/install-agent.sh |"),
        "command must not execute a remotely fetched installer before checking it: {install_command}"
    );
    for placeholder in ["<", ">", "REPLACE", "YOUR_", "TODO"] {
        assert!(
            !install_command.contains(placeholder),
            "install command must need no manual edit: {install_command}"
        );
    }

    let enrolled = send(
        central::build(state.clone()),
        enroll_request(&token, PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&enrolled, StatusCode::OK);
    let body = json_body(enrolled).await;
    assert_eq!(
        body["protocol_version"], PROTOCOL_VERSION,
        "handshake is versioned"
    );
    assert!(body["agent_id"].as_str().is_some());
    let credential = body["credential"].as_str().unwrap().to_string();

    // The credential is issued exactly once: one agent row, its hash Argon2id (not
    // the cleartext), and the response is the only place the cleartext appeared.
    let agents = state.store.list_agents(&location_id).unwrap();
    assert_eq!(agents.len(), 1, "exactly one agent enrolled");
    assert!(
        agents[0].credential_hash.starts_with("$argon2id$"),
        "credential is stored as a salted Argon2id hash"
    );
    assert_ne!(
        agents[0].credential_hash, credential,
        "cleartext is never stored"
    );
}

#[tokio::test]
async fn enrollment_ticket_normalizes_uppercase_checksums() {
    let mut state = test_state();
    state.enroll = EnrollConfig::for_test_with_agent(
        CENTRAL_URL,
        CENTRAL_IDENTITY.to_vec(),
        "https://downloads.example/lg-agent",
        "ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789",
        "https://downloads.example/install-agent.sh",
        "FEDCBA9876543210FEDCBA9876543210FEDCBA9876543210FEDCBA9876543210",
    );
    let cookie = setup_and_login(&state).await;
    let (ticket, _location_id) = create_remote_and_ticket(&state, &cookie).await;
    let install_command = ticket["install_command"].as_str().unwrap();

    assert_eq!(
        ticket["agent_sha256"],
        "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
    );
    assert_eq!(
        ticket["install_script_sha256"],
        "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
    );
    assert!(install_command.contains("abcdef0123456789abcdef"));
    assert!(install_command.contains("fedcba9876543210fedcba"));
    assert!(!install_command.contains("ABCDEF"));
    assert!(!install_command.contains("FEDCBA"));
}

#[tokio::test]
async fn enrollment_ticket_uses_the_configured_api_origin_not_the_tunnel_default() {
    let mut state = test_state();
    state.enroll =
        EnrollConfig::for_test("https://api.central.example:443", CENTRAL_IDENTITY.to_vec())
            .with_tunnel_url("https://tunnel.central.example:8443");
    let cookie = setup_and_login(&state).await;
    let (ticket, _location_id) = create_remote_and_ticket(&state, &cookie).await;
    let install_command = ticket["install_command"].as_str().unwrap();

    assert!(install_command.contains("LG_CENTRAL_URL='https://api.central.example:443'"));
    assert!(install_command.contains("LG_TUNNEL_URL='https://tunnel.central.example:8443'"));
    assert!(!install_command.contains("https://localhost:8443"));
    assert_eq!(ticket["fingerprint"], fingerprint(CENTRAL_IDENTITY));
}

#[tokio::test]
async fn enrollment_ticket_allows_bracketed_ipv6_api_origins() {
    for central_url in ["https://[::1]", "https://[2001:db8::1]:8443"] {
        let mut state = test_state();
        state.enroll = EnrollConfig::for_test(central_url, CENTRAL_IDENTITY.to_vec());
        let cookie = setup_and_login(&state).await;
        let (ticket, _location_id) = create_remote_and_ticket(&state, &cookie).await;
        let install_command = ticket["install_command"].as_str().unwrap();

        assert!(
            install_command.contains(&format!("LG_CENTRAL_URL='{central_url}'")),
            "install command must carry the validated IPv6 API origin: {install_command}"
        );
    }
}

#[tokio::test]
async fn enrollment_ticket_refuses_non_https_api_origins() {
    for central_url in [
        "http://api.central.example",
        "https://",
        "https://bad host:8443",
        "https://api.central.example:notaport",
        "https://api.central.example:+8443",
        "https://api.central.example:0",
        "https://api.central.example/api/enroll",
        "https://api.central.example?x=1",
        "https://api.central.example#fragment",
        "https://[::1]:0",
        "https://[::1]:+8443",
        "https://[::1]/api/enroll",
        "https://[::1]?x=1",
        "https://[::1]#fragment",
        "https://[not-an-ip]:8443",
    ] {
        let mut state = test_state();
        state.enroll = EnrollConfig::for_test(central_url, CENTRAL_IDENTITY.to_vec());
        let cookie = setup_and_login(&state).await;
        let location_id = create_remote_location(&state, &cookie).await;

        let response = request_ticket(&state, &cookie, &location_id).await;
        assert_status(&response, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body_string(response).await.contains("LG_CENTRAL_URL"),
            "operator should see the central URL validation requirement for {central_url:?}"
        );
    }
}

#[tokio::test]
async fn enrollment_ticket_refuses_non_https_tunnel_origins() {
    for tunnel_url in [
        "http://tunnel.central.example:8443",
        "https://",
        "https://bad host:8443",
        "https://tunnel.central.example:notaport",
        "https://tunnel.central.example:+8443",
        "https://tunnel.central.example:0",
        "https://tunnel.central.example/tunnel",
        "https://tunnel.central.example?x=1",
        "https://tunnel.central.example#fragment",
        "https://[::1]:0",
        "https://[::1]:+8443",
        "https://[::1]/tunnel",
        "https://[::1]?x=1",
        "https://[::1]#fragment",
        "https://[not-an-ip]:8443",
    ] {
        let mut state = test_state();
        state.enroll =
            EnrollConfig::for_test("https://api.central.example", CENTRAL_IDENTITY.to_vec())
                .with_tunnel_url(tunnel_url);
        let cookie = setup_and_login(&state).await;
        let location_id = create_remote_location(&state, &cookie).await;

        let response = request_ticket(&state, &cookie, &location_id).await;
        assert_status(&response, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body_string(response).await.contains("LG_TUNNEL_URL"),
            "operator should see the tunnel URL validation requirement for {tunnel_url:?}"
        );
    }
}

#[tokio::test]
async fn enrollment_ticket_refuses_missing_or_malformed_checksums() {
    for (agent_sha256, installer_sha256) in [
        (
            "",
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        ),
        (
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "",
        ),
        (
            "not-a-sha",
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        ),
        (
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "not-a-sha",
        ),
    ] {
        let mut state = test_state();
        state.enroll = EnrollConfig::for_test_with_agent(
            CENTRAL_URL,
            CENTRAL_IDENTITY.to_vec(),
            "https://downloads.example/lg-agent",
            agent_sha256,
            "https://downloads.example/install-agent.sh",
            installer_sha256,
        );
        let cookie = setup_and_login(&state).await;
        let location_id = create_remote_location(&state, &cookie).await;

        let response = request_ticket(&state, &cookie, &location_id).await;
        assert_status(&response, StatusCode::UNPROCESSABLE_ENTITY);
    }
}

#[tokio::test]
async fn enrollment_ticket_refuses_non_https_asset_urls() {
    for (agent_url, installer_url) in [
        (
            "http://downloads.example/lg-agent",
            "https://downloads.example/install-agent.sh",
        ),
        (
            "file:///tmp/lg-agent",
            "https://downloads.example/install-agent.sh",
        ),
        ("https://", "https://downloads.example/install-agent.sh"),
        (
            "https:///lg-agent",
            "https://downloads.example/install-agent.sh",
        ),
        ("https://downloads.example/lg-agent", "https://"),
        (
            "https://downloads.example:notaport/lg-agent",
            "https://downloads.example/install-agent.sh",
        ),
        (
            "https://downloads.example/lg-agent",
            "https://[not-an-ip]/install-agent.sh",
        ),
        (
            "https://downloads.example/lg-agent",
            "file:///tmp/install-agent.sh",
        ),
    ] {
        let mut state = test_state();
        state.enroll = EnrollConfig::for_test_with_agent(
            CENTRAL_URL,
            CENTRAL_IDENTITY.to_vec(),
            agent_url,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            installer_url,
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        );
        let cookie = setup_and_login(&state).await;
        let location_id = create_remote_location(&state, &cookie).await;

        let response = request_ticket(&state, &cookie, &location_id).await;
        assert_status(&response, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body_string(response).await.contains("must be valid https URLs"),
            "operator should see the URL validation requirement for {agent_url:?} / {installer_url:?}"
        );
    }
}

#[tokio::test]
async fn enrollment_ticket_quotes_shell_metacharacter_urls() {
    let mut state = test_state();
    state.enroll = EnrollConfig::for_test_with_agent(
        CENTRAL_URL,
        CENTRAL_IDENTITY.to_vec(),
        "https://downloads.example/lg-agent;touch$IFS/tmp/pwn",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "https://downloads.example/install-agent.sh;touch$IFS/tmp/pwn",
        "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
    );
    let cookie = setup_and_login(&state).await;
    let (ticket, _location_id) = create_remote_and_ticket(&state, &cookie).await;
    let install_command = ticket["install_command"].as_str().unwrap();

    assert!(
        install_command.contains("'https://downloads.example/lg-agent;touch$IFS/tmp/pwn'"),
        "agent URL must be shell-quoted: {install_command}"
    );
    assert!(
        install_command.contains("'https://downloads.example/install-agent.sh;touch$IFS/tmp/pwn'"),
        "installer URL must be shell-quoted: {install_command}"
    );
}

// AC8: a reused token is refused with no credential — single-use is enforced at the
// enrollment endpoint, not just the store.
#[tokio::test]
async fn a_reused_token_is_refused_with_no_second_credential() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let (ticket, location_id) = create_remote_and_ticket(&state, &cookie).await;
    let token = ticket["token"].as_str().unwrap().to_string();

    let first = send(
        central::build(state.clone()),
        enroll_request(&token, PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&first, StatusCode::OK);

    let second = send(
        central::build(state.clone()),
        enroll_request(&token, PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&second, StatusCode::UNAUTHORIZED);

    assert_eq!(
        state.store.list_agents(&location_id).unwrap().len(),
        1,
        "the reused token must not issue a second agent credential"
    );
}

// AC8: a token past its TTL is refused with no credential. An expired token is
// planted directly (the 15-minute TTL can't be waited out in a test).
#[tokio::test]
async fn an_expired_token_is_refused_with_no_credential() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let (_ticket, location_id) = create_remote_and_ticket(&state, &cookie).await;

    let raw = "expired-token-value";
    state
        .store
        .put_enrollment_token(&EnrollmentToken {
            id: "expired".to_string(),
            location_id: location_id.clone(),
            token_hash: sha256_hex(raw.as_bytes()),
            expires_at: 1, // 1970 — long past
            used_at: None,
        })
        .unwrap();

    let response = send(
        central::build(state.clone()),
        enroll_request(raw, PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&response, StatusCode::UNAUTHORIZED);
    // No agent was issued for the expired token (only whatever the ticket flow made,
    // which was never enrolled).
    assert_eq!(state.store.list_agents(&location_id).unwrap().len(), 0);
}

// AC34 / FR-071: enrollment over cleartext (no attested TLS) is refused and issues
// no credential — the token is not even consumed.
#[tokio::test]
async fn cleartext_enrollment_is_refused() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let (ticket, location_id) = create_remote_and_ticket(&state, &cookie).await;
    let token = ticket["token"].as_str().unwrap().to_string();

    let response = send(
        central::build(state.clone()),
        enroll_request(&token, PROTOCOL_VERSION, false),
    )
    .await;
    assert_status(&response, StatusCode::FORBIDDEN);
    assert_eq!(
        state.store.list_agents(&location_id).unwrap().len(),
        0,
        "cleartext enrollment issues no credential"
    );

    // The token was not consumed, so a later secure enrollment still works.
    let ok = send(
        central::build(state.clone()),
        enroll_request(&token, PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&ok, StatusCode::OK);
}

// The handshake is versioned: a request from a peer speaking a different protocol
// version is refused before any credential is issued.
#[tokio::test]
async fn a_mismatched_protocol_version_is_refused() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let (ticket, _location_id) = create_remote_and_ticket(&state, &cookie).await;
    let token = ticket["token"].as_str().unwrap().to_string();

    let response = send(
        central::build(state.clone()),
        enroll_request(&token, PROTOCOL_VERSION + 1, true),
    )
    .await;
    assert_status(&response, StatusCode::UNPROCESSABLE_ENTITY);
}

// Enrollment applies only to remote locations — a local node has no agent to enroll.
#[tokio::test]
async fn enrollment_ticket_refused_for_a_local_location() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let created = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            &cookie,
            &json!({ "name": "Local", "geo_label": "DE", "kind": "local", "offered_methods": [] })
                .to_string(),
        ),
    )
    .await;
    let id = json_body(created).await["id"].as_str().unwrap().to_string();

    let ticket = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{id}/enroll"),
            &cookie,
            "",
        ),
    )
    .await;
    assert_status(&ticket, StatusCode::UNPROCESSABLE_ENTITY);
}

// The enroll endpoint is agent-facing (token-authed), not session-gated, but it is
// setup-gated: it is refused before an admin exists.
#[tokio::test]
async fn enroll_is_refused_before_setup() {
    let state = test_state();
    let response = send(
        central::build(state),
        enroll_request("any-token", PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&response, StatusCode::FORBIDDEN);
}

// FR-064 / no-plaintext-in-logs: minting a token and enrolling with it must not
// write the raw token or the issued credential into any log line.
#[tokio::test]
async fn no_plaintext_token_or_credential_in_logs() {
    let logs = captured_logs();
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let (ticket, _location_id) = create_remote_and_ticket(&state, &cookie).await;
    let token = ticket["token"].as_str().unwrap().to_string();

    let enrolled = send(
        central::build(state.clone()),
        enroll_request(&token, PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&enrolled, StatusCode::OK);
    let credential = json_body(enrolled).await["credential"]
        .as_str()
        .unwrap()
        .to_string();

    let captured = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    // Guard against a vacuous pass: the enroll path did log (its non-secret line),
    // so the buffer really saw this flow — the secret-absence checks below mean it.
    assert!(
        captured.contains("agent enrolled"),
        "log capture must have recorded the enrollment event"
    );
    assert!(
        !captured.contains(&token),
        "the raw enrollment token must never appear in a log line"
    );
    assert!(
        !captured.contains(&credential),
        "the issued credential must never appear in a log line"
    );
}
