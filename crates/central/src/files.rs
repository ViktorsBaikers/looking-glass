//! Direct-from-node download of a location's test files with HTTP range support
//! (FR-050/AC20). The file is served straight off the node under test — the
//! central container for the built-in local node — so a large speedtest download
//! can be resumed (a `Range` request returns `206 Partial Content` with a
//! `Content-Range`). Slice 10 reuses this same range server on the remote agent.
//!
//! Every served path is confined to the configured files root: a test file's
//! admin-set `source_ref` is treated as a path *relative to* that root, and any
//! attempt to climb out (a `..` component, an absolute path, or a symlink that
//! resolves outside) is refused — so the range server can never read a file the
//! operator did not place under the served directory (security.md, trust
//! boundary).

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use futures_util::StreamExt;
use serde_json::json;
use shared::files::resolve_within;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tower_http::services::ServeFile;

use crate::auth::{ApiError, ClientContext};
use crate::run_api::same_origin;
use crate::store::NodeKind;
use crate::AppState;

/// The browser speed-test upload cap (spec #1): 25 MB, refused with 413 beyond.
const UPLOAD_CAP_BYTES: u64 = 25 * 1024 * 1024;
/// The sink is public, so like the agent's it admits a bounded number of uploads
/// before reading any body, and each must finish within a deadline.
const MAX_CONCURRENT_UPLOADS: usize = 4;
/// One client's share of those, so a visitor holding stalled uploads leaves
/// permits for everyone else.
const MAX_UPLOADS_PER_CLIENT: usize = 2;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);

pub fn routes(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/locations/{location_id}/files/{file_id}/download",
            get(download),
        )
        .route(
            "/api/locations/{location_id}/speedtest/upload",
            post(upload).layer((
                Extension(Arc::new(Semaphore::new(MAX_CONCURRENT_UPLOADS))),
                Extension(Arc::new(ClientUploads::default())),
            )),
        )
        .with_state(state)
}

/// The local node's speed-test upload sink (spec #1): streams the visitor's
/// upload and discards it — bytes never touch disk — capped at 25 MB (413
/// beyond) and counted against the same per-client rate limiter as runs. The
/// response reports how many bytes were received. Remote locations upload to
/// their agent's data-plane instead, so a remote (or unknown) id is a 404.
async fn upload(
    State(state): State<AppState>,
    Extension(uploads): Extension<Arc<Semaphore>>,
    Extension(client_uploads): Extension<Arc<ClientUploads>>,
    ctx: ClientContext,
    headers: HeaderMap,
    AxumPath(location_id): AxumPath<String>,
    request: Request<Body>,
) -> Response {
    // The same unauthenticated-work posture as the run endpoint: a browser
    // request must prove same-origin, and the trusted-proxy client identity
    // keys the shared exec rate limit.
    if !same_origin(&headers) {
        return ApiError::Coded(
            StatusCode::FORBIDDEN,
            "cross_origin_refused",
            "The speed test upload must be started from this site.",
        )
        .into_response();
    }
    if let Some(client) = ctx.ip {
        if !state.run.rate_allow(client) {
            return ApiError::RateLimited.into_response();
        }
    }
    match state.store.get_location(&location_id) {
        Ok(Some(location)) if location.kind == NodeKind::Local => {}
        Ok(_) => return ApiError::NotFound.into_response(),
        Err(error) => return ApiError::from(error).into_response(),
    }
    let _share = match ctx.ip {
        Some(client) => match ClientUploads::claim(&client_uploads, client) {
            Some(share) => Some(share),
            None => return too_many_uploads(),
        },
        None => None,
    };
    let Ok(_permit) = uploads.try_acquire() else {
        return too_many_uploads();
    };
    match tokio::time::timeout(
        UPLOAD_TIMEOUT,
        count_upload(request.into_body(), &location_id),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => ApiError::Coded(
            StatusCode::REQUEST_TIMEOUT,
            "upload_timeout",
            "The upload took too long.",
        )
        .into_response(),
    }
}

fn too_many_uploads() -> Response {
    ApiError::Coded(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        "Too many uploads in progress. Try again shortly.",
    )
    .into_response()
}

/// Uploads in progress per client, keyed like the rate limiter: the
/// trusted-proxy client address, with native IPv6 grouped by its /64.
#[derive(Default)]
struct ClientUploads(Mutex<HashMap<IpAddr, usize>>);

/// One claimed upload of a client's share; dropping it returns the slot.
struct ClientShare {
    uploads: Arc<ClientUploads>,
    key: IpAddr,
}

impl ClientUploads {
    fn claim(uploads: &Arc<Self>, client: IpAddr) -> Option<ClientShare> {
        let key = match client.to_canonical() {
            IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (u128::MAX << 64))),
            v4 => v4,
        };
        let mut held = uploads.0.lock().expect("upload share mutex");
        let count = held.entry(key).or_default();
        if *count >= MAX_UPLOADS_PER_CLIENT {
            return None;
        }
        *count += 1;
        Some(ClientShare {
            uploads: Arc::clone(uploads),
            key,
        })
    }
}

impl Drop for ClientShare {
    fn drop(&mut self) {
        let mut held = self.uploads.0.lock().expect("upload share mutex");
        if let Some(count) = held.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.key);
            }
        }
    }
}

async fn count_upload(body: Body, location_id: &str) -> Response {
    let mut stream = body.into_data_stream();
    let mut total: u64 = 0;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                total += bytes.len() as u64;
                if total > UPLOAD_CAP_BYTES {
                    return ApiError::Coded(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "payload_too_large",
                        "Uploads are capped at 25 MB.",
                    )
                    .into_response();
                }
            }
            Err(_) => {
                return ApiError::Coded(
                    StatusCode::BAD_REQUEST,
                    "upload_failed",
                    "The upload could not be read.",
                )
                .into_response()
            }
        }
    }
    tracing::debug!(location_id = %location_id, bytes = total, "speedtest upload discarded");
    Json(json!({ "bytes": total })).into_response()
}

/// Serve one of a location's test files with range support. The file must belong
/// to a local location: central serves only its built-in node, which is always
/// live, while a remote's files come from its agent's data plane. The persisted
/// `status` is not consulted (a local-to-remote edit keeps it `online`), so a
/// remote or unknown location, or a file not owned by it, is a plain 404. The request is handed to `tower-http`'s `ServeFile`, which reads
/// the `Range`/`If-Range` headers and answers `206` for a partial fetch.
async fn download(
    State(state): State<AppState>,
    AxumPath((location_id, file_id)): AxumPath<(String, String)>,
    request: Request<Body>,
) -> Response {
    let location = match state.store.get_location(&location_id) {
        Ok(Some(location)) if location.kind == NodeKind::Local => location,
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return ApiError::from(error).into_response(),
    };

    let file = match state.store.get_test_file(&file_id) {
        Ok(Some(file)) if file.location_id == location.id => file,
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return ApiError::from(error).into_response(),
    };

    // A traversal attempt is indistinguishable from a missing file: refuse both
    // with 404 rather than confirm what lies outside the served root.
    let Some(path) = resolve_within(&state.files_root, &file.source_ref) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // An attachment, so an operator-placed `.html` file is saved, never
    // rendered as a page on the admin origin.
    match ServeFile::new(&path).try_call(request).await {
        Ok(response) => {
            let mut response = response.map(Body::new);
            response.headers_mut().insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment"),
            );
            response
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enroll::EnrollConfig;
    use crate::store::{Location, LocationStatus, Store, TestFile};
    use crate::{LoginLimiter, RunService, TransportConfig, TunnelHub};
    use axum::body::Bytes;
    use tokio::sync::mpsc;
    use tower::ServiceExt;

    fn local_state() -> AppState {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lg-files-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(dir.join("db")).unwrap();
        store
            .put_location(&Location {
                id: "local-1".to_string(),
                name: "Local".to_string(),
                geo_label: "DE".to_string(),
                map_query: None,
                facility: None,
                facility_url: None,
                kind: NodeKind::Local,
                data_plane_origin: None,
                asn: None,
                offered_methods: vec![],
                status: LocationStatus::Online,
                created_at: 0,
            })
            .unwrap();
        AppState {
            store,
            transport: TransportConfig::new([]),
            login_limiter: Arc::new(LoginLimiter::default()),
            setup_token: None,
            run: RunService::for_test(8, Duration::from_secs(30), 100),
            files_root: Arc::from(dir.as_path()),
            enroll: EnrollConfig::for_test("https://central.test:8443", b"identity".to_vec()),
            tunnel_hub: TunnelHub::new(),
        }
    }

    // An operator-placed test file is a download, never a page the
    // browser renders on the admin origin.
    #[tokio::test]
    async fn test_files_are_served_as_attachments() {
        let state = local_state();
        std::fs::write(state.files_root.join("probe.html"), "<script>1</script>").unwrap();
        state
            .store
            .put_test_file(&TestFile {
                id: "file-1".to_string(),
                location_id: "local-1".to_string(),
                label: "probe".to_string(),
                declared_size: "18 B".to_string(),
                source_ref: "probe.html".to_string(),
            })
            .unwrap();
        let response = routes(state)
            .oneshot(
                Request::get("/api/locations/local-1/files/file-1/download")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let disposition = response
            .headers()
            .get("content-disposition")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(disposition.starts_with("attachment"), "{disposition:?}");
    }

    // F-281: an empty files root (LG_FILES_DIR="") is the working directory to
    // the filesystem; the shared resolver refuses it, so nothing there is
    // served. Cargo runs this from crates/central, whose Cargo.toml stands in
    // for a secret there.
    #[tokio::test]
    async fn an_empty_files_root_serves_nothing() {
        let mut state = local_state();
        state.files_root = Arc::from(std::path::Path::new(""));
        assert!(std::path::Path::new("Cargo.toml").is_file());
        state
            .store
            .put_test_file(&TestFile {
                id: "file-1".to_string(),
                location_id: "local-1".to_string(),
                label: "probe".to_string(),
                declared_size: "1 B".to_string(),
                source_ref: "Cargo.toml".to_string(),
            })
            .unwrap();
        let response = routes(state)
            .oneshot(
                Request::get("/api/locations/local-1/files/file-1/download")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // F-293: a source_ref naming a directory under the files root is a 404,
    // not a 200 whose body then fails.
    #[tokio::test]
    async fn a_directory_source_ref_is_not_found() {
        let state = local_state();
        std::fs::create_dir_all(state.files_root.join("sub")).unwrap();
        state
            .store
            .put_test_file(&TestFile {
                id: "file-1".to_string(),
                location_id: "local-1".to_string(),
                label: "probe".to_string(),
                declared_size: "1 B".to_string(),
                source_ref: "sub".to_string(),
            })
            .unwrap();
        let response = routes(state)
            .oneshot(
                Request::get("/api/locations/local-1/files/file-1/download")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // F-302: a source_ref the filesystem rejects (a component over 255
    // bytes) is a 404 like a missing file, not a 500.
    #[tokio::test]
    async fn a_source_ref_the_filesystem_rejects_is_not_found() {
        let state = local_state();
        state
            .store
            .put_test_file(&TestFile {
                id: "file-1".to_string(),
                location_id: "local-1".to_string(),
                label: "probe".to_string(),
                declared_size: "1 B".to_string(),
                source_ref: "a".repeat(256),
            })
            .unwrap();
        let response = routes(state)
            .oneshot(
                Request::get("/api/locations/local-1/files/file-1/download")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// An upload whose body sends one chunk and then never finishes.
    fn trickled_upload() -> (Request<Body>, mpsc::Sender<Result<Bytes, std::io::Error>>) {
        let (tx, rx) = mpsc::channel(1);
        tx.try_send(Ok(Bytes::from_static(b"x"))).unwrap();
        let request = Request::builder()
            .method("POST")
            .uri("/api/locations/local-1/speedtest/upload")
            .header("host", "localhost")
            .header("origin", "http://localhost")
            .body(Body::from_stream(
                tokio_stream::wrappers::ReceiverStream::new(rx),
            ))
            .unwrap();
        (request, tx)
    }

    // The public upload sink bounds how long one upload may take, like the
    // agent's, so a trickled body cannot hold its connection open forever.
    #[tokio::test(start_paused = true)]
    async fn a_slow_upload_is_bounded() {
        let (request, _sender) = trickled_upload();
        let response = tokio::time::timeout(
            Duration::from_secs(120),
            routes(local_state()).oneshot(request),
        )
        .await
        .expect("a slow upload must be bounded")
        .unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    }

    // At most four uploads read their bodies at once; the next is refused
    // before any of its body is read.
    #[tokio::test(start_paused = true)]
    async fn concurrent_uploads_are_capped() {
        let router = routes(local_state());
        let mut held = Vec::new();
        for _ in 0..4 {
            let (request, sender) = trickled_upload();
            held.push((sender, tokio::spawn(router.clone().oneshot(request))));
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(1)).await;

        let (request, _sender) = trickled_upload();
        let refused = tokio::time::timeout(Duration::from_secs(1), router.oneshot(request))
            .await
            .expect("an upload beyond the cap must be refused at once")
            .unwrap();
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        drop(held);
    }
}
