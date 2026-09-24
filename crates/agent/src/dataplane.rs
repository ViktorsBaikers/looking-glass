//! Remote-node speedtest file server (FR-050/AC20).
//!
//! The command tunnel stays outbound-only, but speedtest downloads measure the
//! remote node itself, so the agent can optionally expose a direct HTTP data
//! port. File serving mirrors central's Slice-6 range behavior: `ServeFile`
//! handles `Range` requests, while `source_ref` is confined under a configured
//! root before any filesystem read.

use std::io;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeFile;

const ENV_DATA_BIND: &str = "LG_AGENT_DATA_BIND";
const ENV_FILES_DIR: &str = "LG_AGENT_FILES_DIR";
const DEFAULT_FILES_DIR: &str = "data/files";

/// The browser speed-test upload cap (spec #1), identical to central's sink:
/// 25 MB, refused with 413 beyond.
const UPLOAD_CAP_BYTES: u64 = 25 * 1024 * 1024;
/// The data port is public and unauthenticated, so uploads are admitted before
/// their body is read and must finish within a deadline.
// ponytail: one global cap for the node, add a per-client limiter if abuse shows up.
const MAX_CONCURRENT_UPLOADS: usize = 4;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct DataPlaneState {
    root: Arc<Path>,
    uploads: Arc<Semaphore>,
}

pub fn routes(root: Arc<Path>) -> Router {
    Router::new()
        .route("/files/{*source_ref}", get(download))
        .route("/speedtest/upload", post(upload))
        // The console page measures this node cross-origin: allow the browser's
        // ranged file download and upload fetch (preflight included), exposing
        // the length/range headers the speed test reads (spec #1).
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::HEAD, Method::POST, Method::OPTIONS])
                .allow_headers(Any)
                .expose_headers([
                    header::CONTENT_LENGTH,
                    header::CONTENT_RANGE,
                    header::ACCEPT_RANGES,
                ]),
        )
        .with_state(DataPlaneState {
            root,
            uploads: Arc::new(Semaphore::new(MAX_CONCURRENT_UPLOADS)),
        })
}

pub fn config_from_env() -> io::Result<Option<(SocketAddr, PathBuf)>> {
    let bind = match std::env::var(ENV_DATA_BIND) {
        Ok(value) if !value.is_empty() => value
            .parse()
            .map_err(|error| io::Error::other(format!("invalid {ENV_DATA_BIND}: {error}")))?,
        _ => return Ok(None),
    };
    let root = std::env::var(ENV_FILES_DIR)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_FILES_DIR));
    Ok(Some((bind, root)))
}

pub async fn serve(bind: SocketAddr, root: PathBuf) -> io::Result<()> {
    std::fs::create_dir_all(&root)?;
    let listener = TcpListener::bind(bind).await?;
    tracing::info!(%bind, root = %root.display(), "agent speedtest data-plane listener up");
    axum::serve(listener, routes(Arc::from(root))).await
}

async fn download(
    State(state): State<DataPlaneState>,
    AxumPath(source_ref): AxumPath<String>,
    request: Request<Body>,
) -> Response {
    serve_file(&state.root, &source_ref, request).await
}

async fn serve_file(root: &Path, source_ref: &str, request: Request<Body>) -> Response {
    let Some(path) = resolve_within(root, source_ref) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match ServeFile::new(&path).try_call(request).await {
        Ok(response) => response.map(Body::new),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The remote node's speed-test upload sink (spec #1): streams the visitor's
/// upload and discards it — bytes never touch disk — capped at 25 MB (413
/// beyond), reporting the received byte count. The mirror of central's local
/// sink; the CORS layer above is what lets the console's browser reach it.
async fn upload(State(state): State<DataPlaneState>, request: Request<Body>) -> Response {
    let Ok(_permit) = state.uploads.try_acquire() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "error": "rate_limited",
                "message": "Too many uploads in progress. Try again shortly.",
            })),
        )
            .into_response();
    };
    match tokio::time::timeout(UPLOAD_TIMEOUT, count_upload(request.into_body())).await {
        Ok(response) => response,
        Err(_) => (
            StatusCode::REQUEST_TIMEOUT,
            Json(json!({
                "error": "upload_timeout",
                "message": "The upload took too long.",
            })),
        )
            .into_response(),
    }
}

async fn count_upload(body: Body) -> Response {
    let mut stream = body.into_data_stream();
    let mut total: u64 = 0;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                total += bytes.len() as u64;
                if total > UPLOAD_CAP_BYTES {
                    return (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        Json(json!({
                            "error": "payload_too_large",
                            "message": "Uploads are capped at 25 MB.",
                        })),
                    )
                        .into_response();
                }
            }
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "upload_failed",
                        "message": "The upload could not be read.",
                    })),
                )
                    .into_response()
            }
        }
    }
    (StatusCode::OK, Json(json!({ "bytes": total }))).into_response()
}

fn resolve_within(root: &Path, source_ref: &str) -> Option<PathBuf> {
    let relative = Path::new(source_ref);
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }

    let candidate = root.join(relative);
    match (candidate.canonicalize(), root.canonicalize()) {
        (Ok(resolved), Ok(canonical_root)) => {
            resolved.starts_with(canonical_root).then_some(candidate)
        }
        _ => Some(candidate),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "lg-agent-dataplane-{tag}-{}-{n}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    async fn body_string(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn remote_download_honors_a_range_request() {
        let root = temp_dir("range");
        std::fs::write(root.join("probe.bin"), b"0123456789ABCDEF").unwrap();

        let response = serve_file(
            &root,
            "probe.bin",
            Request::builder()
                .header("range", "bytes=4-7")
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok()),
            Some("bytes 4-7/16")
        );
        assert_eq!(body_string(response).await, "4567");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn remote_download_refuses_path_traversal() {
        let root = temp_dir("traversal");
        let response = serve_file(
            &root,
            "../secret.bin",
            Request::builder().body(Body::empty()).unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn remote_download_returns_not_found_for_a_missing_file() {
        let root = temp_dir("missing");
        let response = serve_file(
            &root,
            "missing/probe.bin",
            Request::builder().body(Body::empty()).unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn remote_download_refuses_nested_parent_traversal() {
        let root = temp_dir("nested-traversal");
        let response = serve_file(
            &root,
            "sub/../secret.bin",
            Request::builder().body(Body::empty()).unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn remote_download_refuses_an_absolute_path() {
        let root = temp_dir("absolute");
        let response = serve_file(
            &root,
            "/etc/passwd",
            Request::builder().body(Body::empty()).unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remote_download_refuses_a_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("symlink-root");
        let outside = temp_dir("symlink-outside");
        let target = outside.join("secret.bin");
        std::fs::write(&target, b"secret").unwrap();
        symlink(&target, root.join("escape.bin")).unwrap();

        let response = serve_file(
            &root,
            "escape.bin",
            Request::builder().body(Body::empty()).unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    // Spec #1 (Agent half): the data-plane upload sink counts and discards the
    // body, and refuses anything beyond the 25 MB cap with 413.
    #[tokio::test]
    async fn remote_upload_sink_counts_bytes_and_caps_at_25_mb() {
        let response = upload(
            State(upload_state()),
            Request::builder()
                .method("POST")
                .body(Body::from(vec![7u8; 4096]))
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_string(response).await, r#"{"bytes":4096}"#);

        let oversized = upload(
            State(upload_state()),
            Request::builder()
                .method("POST")
                .body(Body::from(vec![0u8; 25 * 1024 * 1024 + 1]))
                .unwrap(),
        )
        .await;
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    fn upload_state() -> DataPlaneState {
        DataPlaneState {
            root: Arc::from(Path::new("/nonexistent")),
            uploads: Arc::new(Semaphore::new(MAX_CONCURRENT_UPLOADS)),
        }
    }

    // The public upload sink admits a bounded number of uploads before reading
    // any body: once every slot is taken, the next upload is refused with 429.
    #[tokio::test]
    async fn remote_upload_sink_refuses_uploads_beyond_the_concurrency_cap() {
        let state = upload_state();
        let _held = state
            .uploads
            .clone()
            .try_acquire_many_owned(MAX_CONCURRENT_UPLOADS as u32)
            .unwrap();

        let refused = upload(
            State(state),
            Request::builder()
                .method("POST")
                .body(Body::from(vec![7u8; 16]))
                .unwrap(),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    // Spec #1: the data-plane answers the console's cross-origin preflights for
    // both the ranged download and the upload, and exposes the range headers.
    #[tokio::test]
    async fn data_plane_allows_cross_origin_download_and_upload_fetches() {
        use tower::ServiceExt;

        let root = temp_dir("cors");
        std::fs::write(root.join("probe.bin"), b"0123456789ABCDEF").unwrap();
        let app = routes(Arc::from(root.as_path()));

        let download_preflight = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/files/probe.bin")
                    .header("origin", "https://console.example")
                    .header("access-control-request-method", "GET")
                    .header("access-control-request-headers", "range")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(download_preflight.status(), StatusCode::OK);
        // `Any` origin answers with the wildcard — fine here because the speed
        // test fetch is uncredentialed.
        assert_eq!(
            download_preflight
                .headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("*")
        );

        let upload_preflight = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/speedtest/upload")
                    .header("origin", "https://console.example")
                    .header("access-control-request-method", "POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(upload_preflight.status(), StatusCode::OK);

        // The actual download exposes the length/range headers the speed test reads.
        let download = app
            .oneshot(
                Request::builder()
                    .uri("/files/probe.bin")
                    .header("origin", "https://console.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(download.status(), StatusCode::OK);
        let exposed = download
            .headers()
            .get("access-control-expose-headers")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        for header in ["content-length", "content-range", "accept-ranges"] {
            assert!(
                exposed.contains(header),
                "{header} must be exposed: {exposed}"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
