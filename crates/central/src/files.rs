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

use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tower_http::services::ServeFile;

use crate::store::LocationStatus;
use crate::AppState;

pub fn routes(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/locations/{location_id}/files/{file_id}/download",
            get(download),
        )
        .with_state(state)
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

/// Resolve `source_ref` to a path confined to `root`, or `None` if it would
/// escape. Two independent guards: the relative path may contain only *normal*
/// components (rejecting `..`, an absolute root, or a Windows prefix before any
/// filesystem access), and — for a path that resolves — its canonical form must
/// still sit under the canonical root (defeating a symlink that points out).
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
        // A component-clean path that does not yet resolve (e.g. the file is
        // missing) cannot climb out of root; `ServeFile` will 404 it.
        _ => Some(candidate),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_within_rejects_parent_traversal() {
        let root = Path::new("/srv/files");
        assert!(resolve_within(root, "../../etc/passwd").is_none());
        assert!(resolve_within(root, "sub/../../escape").is_none());
    }

    #[test]
    fn resolve_within_rejects_an_absolute_path() {
        let root = Path::new("/srv/files");
        assert!(resolve_within(root, "/etc/passwd").is_none());
    }

    #[test]
    fn resolve_within_accepts_a_plain_relative_path() {
        let root = Path::new("/srv/files");
        assert_eq!(
            resolve_within(root, "downloads/1gb.bin"),
            Some(PathBuf::from("/srv/files/downloads/1gb.bin"))
        );
    }

    /// A fresh, created temp directory unique per test (process id + counter +
    /// nanos), matching the served-files-root convention in `tests/common`.
    fn unique_temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lg-files-test-{}-{}-{}-{}",
            tag,
            std::process::id(),
            n,
            nanos
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    // A symlink placed under root but pointing at a file outside it must not
    // resolve — the canonicalize/starts_with guard is what stops a range read
    // from following the link out of the served directory.
    #[cfg(unix)]
    #[test]
    fn resolve_within_rejects_a_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = unique_temp_dir("symlink-root");
        let outside = unique_temp_dir("symlink-outside");
        let target = outside.join("secret.bin");
        std::fs::write(&target, b"secret").expect("write outside target");
        symlink(&target, root.join("escape.bin")).expect("create escaping symlink");

        assert!(resolve_within(&root, "escape.bin").is_none());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    // A component-clean path that does not exist yet falls through to the
    // `Some(candidate)` arm; it must stay confined under root, not escape.
    #[test]
    fn resolve_within_confines_a_missing_file() {
        let root = unique_temp_dir("missing");

        let resolved = resolve_within(&root, "downloads/missing.bin")
            .expect("component-clean missing path stays confined");
        assert!(resolved.starts_with(&root));

        let _ = std::fs::remove_dir_all(&root);
    }
}
