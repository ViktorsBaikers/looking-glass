// Shared across test binaries; each binary uses a different subset, so the
// standard `tests/common/mod.rs` pattern needs this to avoid per-binary
// dead-code warnings.
#![allow(dead_code)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{Request, Response, StatusCode};
use std::time::Duration;

use central::{
    AppState, EnrollConfig, LoginLimiter, RunService, Store, TransportConfig, TunnelHub,
};
use tower::ServiceExt;

pub const TRUSTED_PROXY: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
pub const SETUP_TOKEN: &str = "test-setup-token-0123456789abcdef";
/// Fixed identity material so tests know central's fingerprint up front.
pub const CENTRAL_IDENTITY: &[u8] = b"test-central-identity-material";
pub const CENTRAL_URL: &str = "https://central.test:8443";

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn temp_db_path() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "lg-test-{}-{}-{}.redb",
        std::process::id(),
        n,
        nanos
    ));
    path
}

/// A fresh, created temp directory the served-files root points at, unique per
/// state so a download test can drop a file in and read it back.
pub fn temp_files_dir() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut path = std::env::temp_dir();
    path.push(format!("lg-files-{}-{}-{}", std::process::id(), n, nanos));
    std::fs::create_dir_all(&path).expect("create temp files dir");
    path
}

pub fn test_state() -> AppState {
    test_state_at(temp_db_path())
}

pub fn test_state_at(db_path: PathBuf) -> AppState {
    AppState {
        store: Store::open(db_path).expect("open temp store"),
        transport: TransportConfig::new([TRUSTED_PROXY]),
        login_limiter: Arc::new(LoginLimiter::default()),
        setup_token: Some(std::sync::Arc::from(SETUP_TOKEN)),
        run: RunService::for_test(8, Duration::from_secs(30), 100),
        files_root: Arc::from(temp_files_dir().as_path()),
        enroll: EnrollConfig::for_test(CENTRAL_URL, CENTRAL_IDENTITY.to_vec()),
        tunnel_hub: TunnelHub::new(),
    }
}

/// A request that arrives through the trusted proxy over attested TLS — the
/// normal admin path.
pub fn secure_request(method: &str, uri: &str, json: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-forwarded-proto", "https")
        .extension(ConnectInfo(SocketAddr::new(TRUSTED_PROXY, 40000)))
        .body(Body::from(json.to_string()))
        .unwrap()
}

/// A request over plain HTTP with no TLS attestation (direct, untrusted peer).
pub fn cleartext_request(method: &str, uri: &str, json: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .extension(ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)),
            40000,
        )))
        .body(Body::from(json.to_string()))
        .unwrap()
}

pub async fn send(app: axum::Router, request: Request<Body>) -> Response<Body> {
    app.oneshot(request).await.unwrap()
}

pub async fn complete_setup(state: &AppState) {
    let response = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            "/api/setup",
            &format!(
                r#"{{"setup_token":"{SETUP_TOKEN}","username":"alice","password":"correct-horse-battery-staple"}}"#
            ),
        ),
    )
    .await;
    assert_status(&response, StatusCode::CREATED);
}

pub async fn body_string(response: Response<Body>) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Read the session cookie value from a login/response's `Set-Cookie` header so
/// a follow-up request can present it.
pub fn session_cookie(response: &Response<Body>) -> Option<String> {
    let raw = response
        .headers()
        .get("set-cookie")?
        .to_str()
        .ok()?
        .to_string();
    Some(raw.split(';').next()?.to_string())
}

pub fn assert_status(response: &Response<Body>, expected: StatusCode) {
    assert_eq!(response.status(), expected, "unexpected status");
}

use std::sync::{Mutex, OnceLock};

static LOG_CAPTURE: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();

/// A shared buffer every `tracing` line in this test binary is written to, so a test
/// can assert a secret never appeared in a log line. Installed once as the global
/// subscriber; the returned buffer accumulates across the whole binary, so tests
/// assert the *absence* of their own unique secrets (never presence of a shared one).
pub fn captured_logs() -> Arc<Mutex<Vec<u8>>> {
    LOG_CAPTURE
        .get_or_init(|| {
            let buffer = Arc::new(Mutex::new(Vec::new()));
            let writer_buffer = Arc::clone(&buffer);
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(move || CaptureWriter(Arc::clone(&writer_buffer)))
                .finish();
            let _ = tracing::subscriber::set_global_default(subscriber);
            buffer
        })
        .clone()
}

struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
