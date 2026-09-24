use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use tower::ServiceExt;

mod common;

fn app() -> Router {
    central::build(common::test_state())
}

async fn request(uri: &str, app: Router) -> axum::response::Response {
    app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_string(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn health_route_returns_ok() {
    let response = request("/health", app()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_string(response).await, "ok");
}

#[tokio::test]
async fn root_serves_embedded_spa_shell() {
    let response = request("/", app()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        body_string(response).await.contains("Looking Glass"),
        "embedded SPA shell should carry the app title"
    );
}

#[tokio::test]
async fn feature_routes_get_the_correlation_id_layer() {
    let features = Router::new().route("/api/ping", get(|| async { "pong" }));
    let response = request("/api/ping", central::with_routes(features)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().contains_key("x-request-id"),
        "the global layer must wrap routes composed before with_routes()"
    );
    assert_eq!(body_string(response).await, "pong");
}

#[tokio::test]
async fn feature_router_with_its_own_fallback_does_not_panic_on_merge() {
    let feature = Router::new()
        .route("/api/thing", get(|| async { "thing" }))
        .fallback(|| async { (StatusCode::NOT_FOUND, "feature fallback") });
    let app = central::with_routes(Router::new().merge(feature));

    let response = request("/api/thing", app).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_string(response).await, "thing");
}

#[tokio::test]
async fn unmatched_api_path_returns_404_not_the_spa_shell() {
    let response = request("/api/nonexistent", app()).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        !body_string(response).await.contains("Looking Glass"),
        "an /api miss must not fall through to the SPA shell"
    );
}

#[tokio::test]
async fn unmatched_non_api_path_serves_the_spa_shell() {
    let response = request("/some/spa/route", app()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        body_string(response).await.contains("Looking Glass"),
        "client-side routes must fall through to index.html"
    );
}

// Fingerprinted assets are cached for good and compressed; the shell and any
// miss (a chunk from a previous deploy) must revalidate, never be pinned.
#[tokio::test]
async fn spa_assets_are_compressed_and_cached_by_kind() {
    let shell = request("/", app()).await;
    assert_eq!(shell.headers()["cache-control"], "no-cache");
    let html = body_string(shell).await;
    let asset = html
        .split('"')
        .find(|part| part.starts_with("/_app/immutable/") && part.ends_with(".js"))
        .expect("the shell links a fingerprinted script")
        .to_string();

    let response = app()
        .oneshot(
            Request::builder()
                .uri(&asset)
                .header("accept-encoding", "gzip")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(response.headers()["content-encoding"], "gzip");

    let stale = request("/_app/immutable/chunks/gone.js", app()).await;
    assert_eq!(stale.headers()["cache-control"], "no-cache");
}
