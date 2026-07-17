//! Public run endpoint (Slice 4): the Origin guard, the per-client exec rate
//! limit, the global-cap "node busy" refusal, and the SSE transport. The exec
//! engine's safety properties (kill-tree, timeout, cancel, incremental stream)
//! are proven directly in `shared::exec`; these tests cover central's transport
//! policy through the router.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use central::RunService;

mod common;
use common::{body_string, complete_setup, send, test_state};

const UNTRUSTED_PEER: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));

fn run_request(query: &str, host: &str, origin: Option<&str>, peer: IpAddr) -> Request<Body> {
    let mut builder = Request::builder()
        .method("GET")
        .uri(format!("/api/run/stream?{query}"))
        .header("host", host)
        .extension(ConnectInfo(SocketAddr::new(peer, 50000)));
    if let Some(origin) = origin {
        builder = builder.header("origin", origin);
    }
    builder.body(Body::empty()).unwrap()
}

fn content_type(response: &axum::http::Response<Body>) -> String {
    response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

// AC13 — a method the local location does not offer is rejected with a clear
// message, delivered in-band as an SSE error; no command runs.
#[tokio::test]
async fn unknown_method_streams_a_clear_refusal() {
    let state = test_state();
    complete_setup(&state).await;
    let app = central::build(state);
    let response = send(
        app,
        run_request(
            "method=telnet&target=8.8.8.8",
            "localhost",
            Some("http://localhost"),
            UNTRUSTED_PEER,
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(content_type(&response).contains("text/event-stream"));
    let body = body_string(response).await;
    assert!(
        body.contains("event: run-error"),
        "a refusal is a run-error event: {body}"
    );
    assert!(
        body.contains("not available on this location"),
        "clear non-technical method message: {body}"
    );
}

// The session-less CSRF equivalent: a cross-site Origin is refused outright.
#[tokio::test]
async fn cross_origin_run_is_refused() {
    let state = test_state();
    complete_setup(&state).await;
    let app = central::build(state);
    let response = send(
        app,
        run_request(
            "method=ping&target=8.8.8.8",
            "lg.example.com",
            Some("https://evil.test"),
            UNTRUSTED_PEER,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// A same-origin request (Origin host matches Host) passes the guard and opens an
// event stream — proof the built-in local node serves a run (AC27, transport).
#[tokio::test]
async fn same_origin_request_opens_an_event_stream() {
    let state = test_state();
    complete_setup(&state).await;
    let app = central::build(state);
    let response = send(
        app,
        run_request(
            "method=ping&target=8.8.8.8",
            "localhost",
            Some("http://localhost"),
            UNTRUSTED_PEER,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(content_type(&response).contains("text/event-stream"));
    // Drop without draining: closing the stream makes the engine kill the run.
}

// AC40 — at the global cap a run is refused with a clear "node busy" message and
// spawns no process. A saturated engine (cap 0) refuses every run.
#[tokio::test]
async fn node_busy_streams_a_refusal_without_spawning() {
    let mut state = test_state();
    state.run = RunService::for_test(0, Duration::from_secs(30), 100);
    complete_setup(&state).await;
    let app = central::build(state);

    let response = send(
        app,
        run_request(
            "method=ping&target=8.8.8.8",
            "localhost",
            Some("http://localhost"),
            UNTRUSTED_PEER,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("busy"), "node-busy message expected: {body}");
}

// AC39 — the per-client exec rate limit bounds run requests; over the limit a
// request is refused with a clear message (keyed on the client identity).
#[tokio::test]
async fn exec_rate_limit_refuses_repeat_requests() {
    let mut state = test_state();
    state.run = RunService::for_test(8, Duration::from_secs(30), 1);
    let app_state = state;
    complete_setup(&app_state).await;

    // First request is under the limit (its own refusal is the unknown method).
    let first = send(
        central::build(app_state.clone()),
        run_request(
            "method=telnet&target=8.8.8.8",
            "localhost",
            Some("http://localhost"),
            UNTRUSTED_PEER,
        ),
    )
    .await;
    let first_body = body_string(first).await;
    assert!(
        first_body.contains("not available"),
        "first request is method-refused, not rate-limited: {first_body}"
    );

    // Second request from the same client exceeds the limit.
    let second = send(
        central::build(app_state),
        run_request(
            "method=telnet&target=8.8.8.8",
            "localhost",
            Some("http://localhost"),
            UNTRUSTED_PEER,
        ),
    )
    .await;
    let second_body = body_string(second).await;
    assert!(
        second_body.contains("too many requests"),
        "second request is rate-limited: {second_body}"
    );
}

// AC41 — a rejected (non-public) target surfaces a clear error in-band; no run.
#[tokio::test]
async fn private_target_streams_a_clear_error() {
    let state = test_state();
    complete_setup(&state).await;
    let app = central::build(state);
    let response = send(
        app,
        run_request(
            "method=ping&target=10.0.0.1",
            "localhost",
            Some("http://localhost"),
            UNTRUSTED_PEER,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(
        body.contains("public address"),
        "clear non-technical target message: {body}"
    );
}
