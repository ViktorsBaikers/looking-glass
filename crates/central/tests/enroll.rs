//! Agent enrollment integration (Slice 7): token single-use + expiry, credential
//! issued once and hashed at rest, cleartext refusal, the install command carrying
//! central's fingerprint with no manual edit, the protocol version in the handshake,
//! and no plaintext token/credential in any log line. The agent-side pin check lives
//! in `crates/agent/tests/enroll.rs`; this file proves the central surface.

mod common;

use axum::body::Body;
use axum::http::{header::COOKIE, Request, StatusCode};
use serde_json::{json, Value};

use central::{AppState, EnrollConfig, EnrollmentToken};
use common::{
    assert_status, authed, body_string, captured_logs, cleartext_request, json_body,
    secure_request, send, setup_and_login, test_state, CENTRAL_IDENTITY, CENTRAL_URL,
};
use shared::protocol::{fingerprint, identity_pin, sha256_hex, PROTOCOL_VERSION};

fn cleartext_authed(method: &str, uri: &str, cookie: &str, body: &str) -> Request<Body> {
    let mut request = cleartext_request(method, uri, body);
    request
        .headers_mut()
        .insert(COOKIE, cookie.parse().expect("valid session cookie"));
    request
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

// F-155: an agent pins every tunnel connect to the identity it enrolled by, so
// a tunnel certificate whose key differs from LG_CENTRAL_CERT's is logged as an
// error when central starts and enrollment is refused before the token is spent;
// once the config matches, the same token still enrolls.
#[tokio::test]
async fn a_tunnel_identity_that_differs_from_the_enrollment_pin_refuses_enrollment() {
    let logs = captured_logs();
    let mut state = test_state();
    let cookie = setup_and_login(&state).await;
    let (ticket, location_id) = create_remote_and_ticket(&state, &cookie).await;
    let token = ticket["token"].as_str().unwrap().to_string();

    let tunnel_pin = "f155-tunnel-pin-that-differs";
    state.enroll = state.enroll.clone().with_tunnel_pin(tunnel_pin);
    let logged = String::from_utf8_lossy(&logs.lock().unwrap()).to_string();
    assert!(
        logged
            .lines()
            .any(|line| line.contains("ERROR") && line.contains(tunnel_pin)),
        "a pin mismatch must be logged as an error at start-up"
    );

    let refused = send(
        central::build(state.clone()),
        enroll_request(&token, PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&refused, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(refused).await["error"], "identity_mismatch");
    assert!(
        state.store.list_agents(&location_id).unwrap().is_empty(),
        "a refused enrollment issues no credential"
    );

    state.enroll = state
        .enroll
        .clone()
        .with_tunnel_pin(&fingerprint(CENTRAL_IDENTITY));
    let enrolled = send(
        central::build(state.clone()),
        enroll_request(&token, PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&enrolled, StatusCode::OK);
}

// F-328: while the pins differ every ticket would be refused at redeem, so the
// admin is told why at mint time and no token is created.
#[tokio::test]
async fn a_ticket_is_not_minted_while_the_tunnel_identity_differs() {
    let mut state = test_state();
    let cookie = setup_and_login(&state).await;
    state.enroll = state.enroll.clone().with_tunnel_pin("f328-tunnel-pin");
    let location_id = create_remote_location(&state, &cookie).await;

    let refused = request_ticket(&state, &cookie, &location_id).await;
    assert_status(&refused, StatusCode::SERVICE_UNAVAILABLE);
    let body = json_body(refused).await;
    assert_eq!(body["error"], "identity_mismatch");
    assert!(
        body["message"].as_str().unwrap().contains("LG_TUNNEL_CERT"),
        "{body}"
    );
    assert!(body.get("token").is_none(), "{body}");
    assert_eq!(state.enrollment_token_count(&location_id).unwrap(), 0);
}

// F-347: with no tunnel listener (LG_TUNNEL_CERT/LG_TUNNEL_KEY unset or
// unloadable) no enrolled agent could ever connect, so no ticket is minted and
// an outstanding token is refused unspent; once the tunnel runs it enrolls.
#[tokio::test]
async fn enrollment_is_refused_while_no_tunnel_identity_is_loaded() {
    let mut state = test_state();
    let cookie = setup_and_login(&state).await;
    let (ticket, location_id) = create_remote_and_ticket(&state, &cookie).await;
    let token = ticket["token"].as_str().unwrap().to_string();
    state.enroll.tunnel_pin = None;

    let refused = request_ticket(&state, &cookie, &location_id).await;
    assert_status(&refused, StatusCode::SERVICE_UNAVAILABLE);
    let body = json_body(refused).await;
    assert_eq!(body["error"], "tunnel_unavailable");
    let message = body["message"].as_str().unwrap();
    assert!(
        message.contains("tunnel.key")
            && message.contains("LG_TUNNEL_CERT")
            && message.contains("LG_TUNNEL_KEY"),
        "{body}"
    );
    assert!(body.get("token").is_none(), "{body}");
    assert_eq!(state.enrollment_token_count(&location_id).unwrap(), 1);

    let refused = send(
        central::build(state.clone()),
        enroll_request(&token, PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&refused, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(refused).await["error"], "tunnel_unavailable");
    assert!(state.store.list_agents(&location_id).unwrap().is_empty());

    state.enroll = state
        .enroll
        .clone()
        .with_tunnel_pin(&fingerprint(CENTRAL_IDENTITY));
    let enrolled = send(
        central::build(state.clone()),
        enroll_request(&token, PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&enrolled, StatusCode::OK);
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

/// Serializes the tests that set the `LG_CENTRAL_*` env, which one process shares.
static CENTRAL_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `EnrollConfig::from_env` with only `vars` set among the central and tunnel
/// URL/identity env, keeping the test agent assets so a ticket can be minted.
fn config_from_env(vars: &[(&str, &str)], tunnel_pin: &str) -> EnrollConfig {
    let config = {
        let _env = CENTRAL_ENV
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for name in [
            "LG_CENTRAL_URL",
            "LG_CENTRAL_CERT",
            "LG_CENTRAL_IDENTITY",
            "LG_TUNNEL_URL",
        ] {
            std::env::remove_var(name);
        }
        for (name, value) in vars {
            std::env::set_var(name, value);
        }
        let config = EnrollConfig::from_env(Some(tunnel_pin)).expect("valid enrollment config");
        for (name, _) in vars {
            std::env::remove_var(name);
        }
        config
    };
    EnrollConfig {
        central_url: config.central_url,
        web_url: config.web_url,
        tunnel_url: config.tunnel_url,
        identity: config.identity,
        tunnel_pin: config.tunnel_pin,
        ..EnrollConfig::for_test(CENTRAL_URL, CENTRAL_IDENTITY.to_vec())
    }
}

/// A PEM certificate file for `LG_CENTRAL_CERT`, and the pin agents verify it by.
fn central_cert_file() -> (std::path::PathBuf, String) {
    let generated = rcgen::generate_simple_self_signed(vec!["api.central.example".to_string()])
        .expect("generate certificate");
    let path = common::temp_files_dir().join("central.pem");
    std::fs::write(&path, generated.cert.pem()).expect("write certificate");
    (path, identity_pin(generated.cert.der()))
}

// With no LG_CENTRAL_* env the tunnel listener serves enrollment, so the
// install command points the agent at the tunnel origin under the tunnel pin.
#[tokio::test]
async fn with_no_central_env_the_install_command_targets_the_tunnel() {
    let tunnel_pin = fingerprint(b"tunnel-certificate");
    let mut state = test_state();
    state.enroll = config_from_env(
        &[("LG_TUNNEL_URL", "https://lg.example.net:8443")],
        &tunnel_pin,
    );
    let cookie = setup_and_login(&state).await;
    let (ticket, location_id) = create_remote_and_ticket(&state, &cookie).await;
    let install_command = ticket["install_command"].as_str().unwrap();

    assert!(
        install_command.contains("LG_CENTRAL_URL='https://lg.example.net:8443'"),
        "{install_command}"
    );
    assert!(
        install_command.contains("LG_TUNNEL_URL='https://lg.example.net:8443'"),
        "{install_command}"
    );
    assert!(
        install_command.contains(&format!("LG_CENTRAL_FP='{tunnel_pin}'")),
        "{install_command}"
    );
    assert_eq!(ticket["fingerprint"], tunnel_pin);

    let token = ticket["token"].as_str().unwrap();
    let enrolled = send(
        central::build(state.clone()),
        enroll_request(token, PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&enrolled, StatusCode::OK);
    assert_eq!(state.store.list_agents(&location_id).unwrap().len(), 1);
}

// Any one LG_CENTRAL_* variable keeps the configured-origin behavior: without
// a certificate there is nothing an agent could pin, so no ticket is minted.
#[tokio::test]
async fn a_partial_central_env_does_not_fall_back_to_the_tunnel() {
    let tunnel_pin = fingerprint(b"tunnel-certificate");
    for var in [
        ("LG_CENTRAL_URL", "https://api.central.example"),
        ("LG_CENTRAL_IDENTITY", "operator-chosen-identity"),
    ] {
        let mut state = test_state();
        state.enroll = config_from_env(&[var], &tunnel_pin);
        assert_ne!(state.enroll.identity.fingerprint(), tunnel_pin, "{var:?}");
        let cookie = setup_and_login(&state).await;
        let location_id = create_remote_location(&state, &cookie).await;

        let refused = request_ticket(&state, &cookie, &location_id).await;
        assert_status(&refused, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body_string(refused).await.contains("LG_CENTRAL_CERT"),
            "{var:?}"
        );
    }
}

// LG_CENTRAL_URL and LG_CENTRAL_CERT still name the API origin and its
// certificate, and a certificate whose key differs from the tunnel's is refused.
#[tokio::test]
async fn configured_central_url_and_cert_override_the_tunnel_default() {
    let (cert, cert_pin) = central_cert_file();
    let cert = cert.to_str().unwrap();
    let vars = [
        ("LG_CENTRAL_URL", "https://api.central.example"),
        ("LG_CENTRAL_CERT", cert),
    ];

    let mut state = test_state();
    state.enroll = config_from_env(&vars, &fingerprint(b"tunnel-certificate"));
    let cookie = setup_and_login(&state).await;
    let location_id = create_remote_location(&state, &cookie).await;
    let refused = request_ticket(&state, &cookie, &location_id).await;
    assert_status(&refused, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(refused).await["error"], "identity_mismatch");

    state.enroll = config_from_env(&vars, &cert_pin);
    let (ticket, _location_id) = create_remote_and_ticket(&state, &cookie).await;
    let install_command = ticket["install_command"].as_str().unwrap();
    assert!(
        install_command.contains("LG_CENTRAL_URL='https://api.central.example'"),
        "{install_command}"
    );
    assert!(
        install_command.contains("LG_TUNNEL_URL='https://localhost:8443'"),
        "{install_command}"
    );
    assert_eq!(ticket["fingerprint"], cert_pin);
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

async fn enroll_status(state: &AppState, token: &str) -> StatusCode {
    send(
        central::build(state.clone()),
        enroll_request(token, PROTOCOL_VERSION, true),
    )
    .await
    .status()
}

fn token_of(ticket: &Value) -> String {
    ticket["token"].as_str().unwrap().to_string()
}

// Regenerate supersedes the earlier install command; only the newest works.
#[tokio::test]
async fn regenerating_a_ticket_invalidates_the_older_token() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let (first, location_id) = create_remote_and_ticket(&state, &cookie).await;
    let second = json_body(request_ticket(&state, &cookie, &location_id).await).await;

    assert_eq!(
        enroll_status(&state, &token_of(&first)).await,
        StatusCode::UNAUTHORIZED,
        "the superseded token must no longer enroll"
    );
    assert_eq!(
        enroll_status(&state, &token_of(&second)).await,
        StatusCode::OK
    );
    assert_eq!(state.store.list_agents(&location_id).unwrap().len(), 1);
}

// Revoke also withdraws every outstanding install command for the location.
#[tokio::test]
async fn revoking_the_agent_invalidates_outstanding_tokens() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let (ticket, location_id) = create_remote_and_ticket(&state, &cookie).await;

    let revoked = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{location_id}/agent/revoke"),
            &cookie,
            "",
        ),
    )
    .await;
    assert_status(&revoked, StatusCode::OK);

    assert_eq!(
        enroll_status(&state, &token_of(&ticket)).await,
        StatusCode::UNAUTHORIZED,
        "a token minted before the revoke must no longer enroll"
    );
    assert_eq!(state.store.list_agents(&location_id).unwrap().len(), 0);
    assert_eq!(state.enrollment_token_count(&location_id).unwrap(), 0);
}

// F-182: switching a remote location to local revokes its enrolled agent and
// withdraws its outstanding install command in the same save, since a local
// node has no agent and Revoke no longer applies to it.
#[tokio::test]
async fn switching_a_location_to_local_revokes_its_agent_and_tokens() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let (first, location_id) = create_remote_and_ticket(&state, &cookie).await;
    let enrolled = send(
        central::build(state.clone()),
        enroll_request(&token_of(&first), PROTOCOL_VERSION, true),
    )
    .await;
    assert_status(&enrolled, StatusCode::OK);
    let agent_id = json_body(enrolled).await["agent_id"]
        .as_str()
        .unwrap()
        .to_string();
    let outstanding = json_body(request_ticket(&state, &cookie, &location_id).await).await;

    let switched = send(
        central::build(state.clone()),
        authed(
            "PUT",
            &format!("/api/admin/locations/{location_id}"),
            &cookie,
            &json!({ "name": "Remote", "geo_label": "DE", "kind": "local", "offered_methods": [] })
                .to_string(),
        ),
    )
    .await;
    assert_status(&switched, StatusCode::OK);

    assert!(
        state.store.get_agent(&agent_id).unwrap().unwrap().revoked,
        "the agent of a location switched to local must be revoked"
    );
    assert_eq!(state.enrollment_token_count(&location_id).unwrap(), 0);
    assert_eq!(
        enroll_status(&state, &token_of(&outstanding)).await,
        StatusCode::UNAUTHORIZED,
        "an install command minted while remote must not enroll a local location"
    );
}

// Minting purges every expired or used token, whatever its location.
#[tokio::test]
async fn minting_purges_expired_and_used_tokens() {
    let state = test_state();
    let cookie = setup_and_login(&state).await;
    let expired_location = create_remote_location(&state, &cookie).await;
    state
        .store
        .put_enrollment_token(&EnrollmentToken {
            id: "expired".to_string(),
            location_id: expired_location.clone(),
            token_hash: sha256_hex(b"expired-token-value"),
            expires_at: 1,
            used_at: None,
        })
        .unwrap();
    let (used, used_location) = create_remote_and_ticket(&state, &cookie).await;
    assert_eq!(
        enroll_status(&state, &token_of(&used)).await,
        StatusCode::OK
    );

    let (_fresh, fresh_location) = create_remote_and_ticket(&state, &cookie).await;

    assert_eq!(
        state.enrollment_token_count(&expired_location).unwrap(),
        0,
        "the expired token is purged on mint"
    );
    assert_eq!(
        state.enrollment_token_count(&used_location).unwrap(),
        0,
        "the used token is purged on mint"
    );
    assert_eq!(state.enrollment_token_count(&fresh_location).unwrap(), 1);
}

// A token that outlived its location (a mint racing a delete) enrolls
// nothing, so no agent exists that no admin surface can revoke.
#[tokio::test]
async fn a_token_for_a_deleted_location_enrolls_nothing() {
    let state = test_state();
    let _cookie = setup_and_login(&state).await;
    state
        .store
        .put_enrollment_token(&EnrollmentToken {
            id: "orphan".to_string(),
            location_id: "deleted-location".to_string(),
            token_hash: sha256_hex(b"orphan-token-value"),
            expires_at: u64::MAX,
            used_at: None,
        })
        .unwrap();

    assert_eq!(
        enroll_status(&state, "orphan-token-value").await,
        StatusCode::UNAUTHORIZED
    );
    assert!(state.store.all_agents().unwrap().is_empty());
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
    assert_eq!(json_body(response).await["error"], "insecure_transport");
    // A client claiming https itself is still cleartext: only a trusted proxy
    // (or central's own tunnel TLS) makes the transport secure.
    let mut forged = enroll_request(&token, PROTOCOL_VERSION, false);
    forged
        .headers_mut()
        .insert("x-forwarded-proto", "https".parse().unwrap());
    let response = send(central::build(state.clone()), forged).await;
    assert_status(&response, StatusCode::FORBIDDEN);
    assert_eq!(json_body(response).await["error"], "insecure_transport");
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
    let (ticket, location_id) = create_remote_and_ticket(&state, &cookie).await;
    let token = ticket["token"].as_str().unwrap().to_string();

    let response = send(
        central::build(state.clone()),
        enroll_request(&token, PROTOCOL_VERSION + 1, true),
    )
    .await;
    assert_status(&response, StatusCode::UNPROCESSABLE_ENTITY);

    // The refusal writes nothing — no agent, and the token is not consumed.
    assert_eq!(state.store.list_agents(&location_id).unwrap().len(), 0);
    assert_eq!(enroll_status(&state, &token).await, StatusCode::OK);
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
