mod common;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::net::SocketAddr;

use central::AppState;
use common::{
    assert_status, body_string, captured_logs, send, session_cookie, test_state, SETUP_TOKEN,
    TRUSTED_PROXY,
};
use shared::protocol::PROTOCOL_VERSION;

const PASSWORD: &str = "Slice13-Password-Do-Not-Log!";
const BAD_PASSWORD: &str = "Slice13-Bad-Password-Do-Not-Log!";

fn request(method: &str, uri: &str, body: &str, correlation_id: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-forwarded-proto", "https")
        .header("x-request-id", correlation_id)
        .extension(ConnectInfo(SocketAddr::new(TRUSTED_PROXY, 40000)))
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn authed(
    method: &str,
    uri: &str,
    cookie: &str,
    body: &str,
    correlation_id: &str,
) -> Request<Body> {
    let mut request = request(method, uri, body, correlation_id);
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    request
}

fn run_request(uri: &str, correlation_id: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("host", "central.test")
        .header("origin", "https://central.test")
        .header("x-forwarded-proto", "https")
        .header("x-request-id", correlation_id)
        .extension(ConnectInfo(SocketAddr::new(TRUSTED_PROXY, 40000)))
        .body(Body::empty())
        .unwrap()
}

async fn json_body(response: axum::http::Response<Body>) -> Value {
    serde_json::from_str(&body_string(response).await).expect("json body")
}

fn contains_field(logs: &str, name: &str, value: &str) -> bool {
    logs.contains(&format!("{name}={value}")) || logs.contains(&format!("{name}=\"{value}\""))
}

async fn setup_and_login(state: &AppState) -> String {
    let install = send(
        central::build(state.clone()),
        request(
            "POST",
            "/api/setup",
            &json!({ "setup_token": SETUP_TOKEN, "username": "alice", "password": PASSWORD })
                .to_string(),
            "corr-setup-13",
        ),
    )
    .await;
    assert_status(&install, StatusCode::CREATED);

    let rejected = send(
        central::build(state.clone()),
        request(
            "POST",
            "/api/auth/login",
            &json!({ "username": "alice", "password": BAD_PASSWORD }).to_string(),
            "corr-auth-reject-13",
        ),
    )
    .await;
    assert_status(&rejected, StatusCode::UNAUTHORIZED);

    let login = send(
        central::build(state.clone()),
        request(
            "POST",
            "/api/auth/login",
            &json!({ "username": "alice", "password": PASSWORD }).to_string(),
            "corr-auth-ok-13",
        ),
    )
    .await;
    assert_status(&login, StatusCode::NO_CONTENT);
    session_cookie(&login).expect("login sets a session cookie")
}

async fn remote_ticket(state: &AppState, cookie: &str) -> Value {
    let invalid = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            cookie,
            &json!({ "name": "", "geo_label": "DE", "kind": "remote", "offered_methods": [] })
                .to_string(),
            "corr-admin-reject-13",
        ),
    )
    .await;
    assert_status(&invalid, StatusCode::UNPROCESSABLE_ENTITY);

    let location = send(
        central::build(state.clone()),
        authed(
            "POST",
            "/api/admin/locations",
            cookie,
            &json!({ "name": "Remote", "geo_label": "DE", "kind": "remote", "offered_methods": [] })
                .to_string(),
            "corr-location-13",
        ),
    )
    .await;
    assert_status(&location, StatusCode::CREATED);
    let location_id = json_body(location).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let ticket = send(
        central::build(state.clone()),
        authed(
            "POST",
            &format!("/api/admin/locations/{location_id}/enroll"),
            cookie,
            "",
            "corr-ticket-13",
        ),
    )
    .await;
    assert_status(&ticket, StatusCode::OK);
    json_body(ticket).await
}

#[tokio::test]
async fn required_events_are_structured_correlated_and_secret_free() {
    let logs = captured_logs();
    let state = test_state();
    let cookie = setup_and_login(&state).await;

    let refused = send(
        central::build(state.clone()),
        run_request(
            "/api/run/stream?method=ping&target=10.0.0.1",
            "corr-run-reject-13",
        ),
    )
    .await;
    assert_status(&refused, StatusCode::OK);

    let ticket = remote_ticket(&state, &cookie).await;
    let token = ticket["token"].as_str().unwrap().to_string();
    let enrolled = send(
        central::build(state.clone()),
        request(
            "POST",
            "/api/enroll",
            &json!({ "protocol_version": PROTOCOL_VERSION, "token": token }).to_string(),
            "corr-enroll-13",
        ),
    )
    .await;
    assert_status(&enrolled, StatusCode::OK);
    let credential = json_body(enrolled).await["credential"]
        .as_str()
        .unwrap()
        .to_string();

    let captured = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    for needle in [
        "auth.setup",
        "auth.login",
        "command.run",
        "validation.rejected",
        "agent.enroll",
    ] {
        assert!(
            contains_field(&captured, "event", needle),
            "missing structured log {needle}\n{captured}"
        );
    }
    for correlation_id in [
        "corr-setup-13",
        "corr-auth-ok-13",
        "corr-auth-reject-13",
        "corr-run-reject-13",
        "corr-ticket-13",
        "corr-enroll-13",
        "corr-admin-reject-13",
    ] {
        assert!(
            contains_field(&captured, "correlation_id", correlation_id),
            "missing correlation id {correlation_id}\n{captured}"
        );
    }
    assert!(
        captured.contains("surface=\"admin.location\""),
        "{captured}"
    );
    for secret in [
        PASSWORD,
        BAD_PASSWORD,
        SETUP_TOKEN,
        token.as_str(),
        credential.as_str(),
    ] {
        assert!(
            !captured.contains(secret),
            "secret leaked into logs: {secret}\n{captured}"
        );
    }
}
