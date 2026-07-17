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

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tower_http::services::ServeFile;

const ENV_DATA_BIND: &str = "LG_AGENT_DATA_BIND";
const ENV_FILES_DIR: &str = "LG_AGENT_FILES_DIR";
const DEFAULT_FILES_DIR: &str = "data/files";

#[derive(Clone)]
struct DataPlaneState {
    root: Arc<Path>,
}

pub fn routes(root: Arc<Path>) -> Router {
    Router::new()
        .route("/files/{*source_ref}", get(download))
        .with_state(DataPlaneState { root })
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
}
