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
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde_json::json;
use shared::files::resolve_within;
use tower_http::services::ServeFile;

use crate::auth::{ApiError, ClientContext};
use crate::run_api::same_origin;
use crate::store::{LocationStatus, NodeKind};
use crate::AppState;

/// The browser speed-test upload cap (spec #1): 25 MB, refused with 413 beyond.
const UPLOAD_CAP_BYTES: u64 = 25 * 1024 * 1024;

pub fn routes(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/locations/{location_id}/files/{file_id}/download",
            get(download),
        )
        .route(
            "/api/locations/{location_id}/speedtest/upload",
            post(upload),
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
        Err(_) => return ApiError::Internal.into_response(),
    }

    let mut stream = request.into_body().into_data_stream();
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
/// to an *online* (public) location, matching the visitor-facing catalogue
/// (FR-026) — an offline or unknown location, or a file not owned by it, is a
/// plain 404. The request is handed to `tower-http`'s `ServeFile`, which reads
/// the `Range`/`If-Range` headers and answers `206` for a partial fetch.
async fn download(
    State(state): State<AppState>,
    AxumPath((location_id, file_id)): AxumPath<(String, String)>,
    request: Request<Body>,
) -> Response {
    let location = match state.store.get_location(&location_id) {
        Ok(Some(location)) if location.status == LocationStatus::Online => location,
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let file = match state.store.get_test_file(&file_id) {
        Ok(Some(file)) if file.location_id == location.id => file,
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // A traversal attempt is indistinguishable from a missing file: refuse both
    // with 404 rather than confirm what lies outside the served root.
    let Some(path) = resolve_within(&state.files_root, &file.source_ref) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match ServeFile::new(&path).try_call(request).await {
        Ok(response) => response.map(Body::new),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
