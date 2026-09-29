//! Remote-node speedtest file server (FR-050/AC20).
//!
//! The command tunnel stays outbound-only, but speedtest downloads measure the
//! remote node itself, so once central assigns the node a data-plane origin the
//! agent serves it directly over HTTPS on :443, with a certificate it obtains
//! and renews itself ([`crate::acme`]). File serving mirrors central's
//! Slice-6 range behavior: `ServeFile` handles `Range` requests, while
//! `source_ref` is confined under a configured root before any filesystem read.

use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde_json::json;
use shared::files::resolve_within;
use shared::protocol::CertificateStatus;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::{watch, Semaphore};
use tokio_rustls::TlsAcceptor;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeFile;

use crate::acme::{Acme, CertResolver};

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

/// The optional bind override (default: port 443 on every address) and the
/// files root.
fn config_from_env() -> io::Result<(Option<SocketAddr>, PathBuf)> {
    config_from(|key| std::env::var(key).ok())
}

/// [`config_from_env`] reading each setting through `var`.
fn config_from(var: impl Fn(&str) -> Option<String>) -> io::Result<(Option<SocketAddr>, PathBuf)> {
    let bind = match var(ENV_DATA_BIND) {
        Some(value) if !value.is_empty() => Some(
            value
                .parse()
                .map_err(|error| io::Error::other(format!("invalid {ENV_DATA_BIND}: {error}")))?,
        ),
        _ => None,
    };
    // Empty is unset: an empty root would be the working directory, where the
    // agent keeps its credential and ACME account.
    let root = var(ENV_FILES_DIR)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_FILES_DIR));
    Ok((bind, root))
}

/// Start the data plane once central assigns an origin: listen on :443 (or
/// `LG_AGENT_DATA_BIND`) and serve HTTPS for the agent's life. A later origin
/// only re-issues the certificate. A listener that cannot bind (port taken, no
/// permission yet) or a files root that cannot be created is retried with
/// backoff until it works; meanwhile the failure is the certificate's last
/// error, reported again after an origin change voids it, so the admin sees why
/// the node has no HTTPS. An invalid setting is reported the same way but never
/// retried: only a restart can change it.
pub async fn run(
    origin: watch::Receiver<Option<String>>,
    status: watch::Sender<Option<CertificateStatus>>,
    acme: Acme,
) -> io::Result<()> {
    start(config_from_env(), origin, status, acme).await
}

/// [`run`] with the configuration already read.
async fn start(
    config: io::Result<(Option<SocketAddr>, PathBuf)>,
    origin: watch::Receiver<Option<String>>,
    status: watch::Sender<Option<CertificateStatus>>,
    acme: Acme,
) -> io::Result<()> {
    let mut assigned = origin.clone();
    if assigned.wait_for(Option::is_some).await.is_err() {
        return Ok(());
    }
    let (bind, root) = match config {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "agent speedtest data plane not started");
            loop {
                report_failure(&status, error.to_string());
                if assigned.changed().await.is_err() {
                    return Err(error);
                }
            }
        }
    };
    let mut retry = BIND_FIRST_RETRY;
    let listener = loop {
        let bound = match bind {
            Some(bind) => TcpListener::bind(bind).await,
            None => bind_default(443).await,
        };
        let error = match bound {
            Ok(listener) => match std::fs::create_dir_all(&root) {
                Ok(()) => break listener,
                Err(error) => format!(
                    "cannot create the files directory {}: {error}",
                    root.display()
                ),
            },
            Err(error) => format!("cannot listen for HTTPS: {error}"),
        };
        tracing::warn!(%error, ?retry, "data plane cannot start; retrying");
        report_failure(&status, error);
        tokio::select! {
            () = tokio::time::sleep(retry) => retry = (retry * 2).min(BIND_MAX_RETRY),
            // A new origin voided the report: retry and report it now.
            Ok(()) = assigned.changed() => {}
        }
    };
    serve_https(listener, root, origin, status, acme).await
}

/// The listener `start` opens when no bind is configured: every address on
/// `port` (443 there, 0 in tests).
async fn bind_default(port: u16) -> io::Result<TcpListener> {
    bind_every_address(TcpSocket::new_v6(), port).await
}

/// Listen on every address on `port`: the IPv6 socket `v6` on `[::]` taking
/// IPv4 too (a `net.ipv6.bindv6only=1` host would otherwise refuse it), or
/// `0.0.0.0` where IPv6 is unavailable. The caller makes the socket, so a test
/// can hand this very function one that starts IPv6-only.
async fn bind_every_address(v6: io::Result<TcpSocket>, port: u16) -> io::Result<TcpListener> {
    match v6.and_then(|socket| listen_dual_stack(socket, port)) {
        Ok(listener) => Ok(listener),
        Err(_) => TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await,
    }
}

/// `socket` listening on `[::]:port`, taking IPv4 too.
fn listen_dual_stack(socket: TcpSocket, port: u16) -> io::Result<TcpListener> {
    // What `TcpListener::bind` sets (with its backlog below), so a restart
    // can rebind at once.
    socket.set_reuseaddr(true)?;
    #[cfg(unix)]
    shared::files::accept_ipv4_too(&socket)?;
    socket.bind((Ipv6Addr::UNSPECIFIED, port).into())?;
    socket.listen(128)
}

/// Publish a startup failure as the certificate's last error, only when it is
/// news: the first failure, a different error, or the tunnel voided the report
/// because the origin changed.
fn report_failure(status: &watch::Sender<Option<CertificateStatus>>, error: String) {
    let failed = CertificateStatus {
        last_error: Some(error),
        ..CertificateStatus::default()
    };
    status.send_if_modified(|current| {
        if current.as_ref() == Some(&failed) {
            return false;
        }
        *current = Some(failed);
        true
    });
}

const BIND_FIRST_RETRY: Duration = Duration::from_secs(1);
const BIND_MAX_RETRY: Duration = Duration::from_secs(60);

/// Serve the data plane over HTTPS on `listener` with the certificate `acme`
/// keeps for the assigned origin.
pub async fn serve_https(
    listener: TcpListener,
    root: PathBuf,
    origin: watch::Receiver<Option<String>>,
    status: watch::Sender<Option<CertificateStatus>>,
    acme: Acme,
) -> io::Result<()> {
    std::fs::create_dir_all(&root)?;
    let resolver = Arc::new(CertResolver::default());
    tokio::spawn(crate::acme::manage(
        acme,
        origin,
        Arc::clone(&resolver),
        status,
    ));
    tracing::info!(
        bind = %listener.local_addr()?,
        root = %root.display(),
        "agent speedtest data plane listening (HTTPS)"
    );
    let tls = TlsAcceptor::from(Arc::new(crate::acme::server_config(resolver)));
    serve_with(
        listener,
        routes(Arc::from(root)),
        Some(tls),
        HEADER_READ_TIMEOUT,
        WRITE_IDLE_TIMEOUT,
        MAX_CONNECTIONS,
    )
    .await
}

/// The data port is public, so a request header (and a keep-alive connection's
/// next one) must arrive within this long, a response must keep being read (no
/// write accepted for `WRITE_IDLE_TIMEOUT` closes it), and at most this many
/// sockets are held; idle, slow or stalled peers cannot exhaust the agent's
/// descriptors or its slots.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONNECTIONS: usize = 256;

// ponytail: mirrors central's `serve_with` (the agent does not link central); fold
// into `shared` if a third listener appears.
async fn serve_with(
    listener: TcpListener,
    app: Router,
    tls: Option<TlsAcceptor>,
    header_read_timeout: Duration,
    write_idle_timeout: Duration,
    max_connections: usize,
) -> io::Result<()> {
    use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
    use hyper_util::server::conn::auto::Builder;
    use hyper_util::service::TowerToHyperService;

    let slots = Arc::new(Semaphore::new(max_connections));
    let mut builder = Builder::new(TokioExecutor::new()).http1_only();
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(header_read_timeout);
    loop {
        let slot = Arc::clone(&slots)
            .acquire_owned()
            .await
            .expect("connection slots are never closed");
        let (tcp, _peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "data-plane accept failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let service = TowerToHyperService::new(app.clone());
        let builder = builder.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let Some(tls) = tls else {
                let io = TokioIo::new(WriteIdle::new(tcp, write_idle_timeout));
                let _ = builder.serve_connection(io, service).await;
                return;
            };
            // The handshake is bounded like a request header, so a silent peer
            // cannot hold the slot. An `acme-tls/1` connection is only the CA's
            // TLS-ALPN-01 handshake (RFC 8737): close it without serving HTTP.
            let Ok(Ok(stream)) = tokio::time::timeout(header_read_timeout, tls.accept(tcp)).await
            else {
                return;
            };
            if stream.get_ref().1.alpn_protocol() == Some(b"acme-tls/1") {
                return;
            }
            let io = TokioIo::new(WriteIdle::new(stream, write_idle_timeout));
            let _ = builder.serve_connection(io, service).await;
        });
    }
}

/// A connection that fails its pending write once the kernel has accepted no
/// write for `limit`, so a client that requests a file and never reads it
/// cannot hold a slot. Each accepted write restarts the clock.
// ponytail: the kernel wakes a blocked writer only after a chunk of its
// autotuned send queue drains (0.5-0.9 MB on Linux loopback), so a reader
// below roughly that chunk per `limit` (~17-31 KB/s at 30 s) is closed too;
// the 10 s browser speed test never reaches the limit. Upgrade path:
// TCP_NOTSENT_LOWAT or a send-queue check (SIOCOUTQ) via `shared`'s libc.
// A fixed SO_SNDBUF would bound the chunk but cap throughput at SNDBUF/RTT.
struct WriteIdle<S> {
    inner: S,
    limit: Duration,
    stalled: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<S> WriteIdle<S> {
    fn new(inner: S, limit: Duration) -> Self {
        Self {
            inner,
            limit,
            stalled: None,
        }
    }

    fn progress<T>(
        &mut self,
        cx: &mut Context<'_>,
        poll: Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        if poll.is_ready() {
            self.stalled = None;
            return poll;
        }
        let limit = self.limit;
        let stalled = self
            .stalled
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(limit)));
        match stalled.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(io::ErrorKind::TimedOut.into())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for WriteIdle<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for WriteIdle<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.progress(cx, poll)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        self.progress(cx, poll)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let poll = Pin::new(&mut self.inner).poll_flush(cx);
        self.progress(cx, poll)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let poll = Pin::new(&mut self.inner).poll_shutdown(cx);
        self.progress(cx, poll)
    }
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

    // F-293: a directory under the files root is a 404 however the URL spells
    // it, not a 200 whose body then fails.
    #[tokio::test]
    async fn remote_download_returns_not_found_for_a_directory() {
        use tower::ServiceExt;

        let root = temp_dir("directory");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let app = routes(Arc::from(root.as_path()));
        for uri in ["/files/sub", "/files/sub/", "/files/sub/."] {
            let response = app
                .clone()
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    // F-302: a name the filesystem rejects (a component over 255 bytes) is a
    // 404 like any other unservable name, not a 500.
    #[tokio::test]
    async fn remote_download_returns_not_found_for_a_name_the_filesystem_rejects() {
        use tower::ServiceExt;

        let root = temp_dir("name-too-long");
        let response = routes(Arc::from(root.as_path()))
            .oneshot(
                Request::get(format!("/files/{}", "a".repeat(256)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

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

    // The public data port closes a peer that never finishes its request
    // header instead of holding the socket forever.
    #[tokio::test]
    async fn data_plane_closes_a_header_less_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_with(
            listener,
            routes(Arc::from(temp_dir("header-less").as_path())),
            None,
            Duration::from_millis(300),
            Duration::from_millis(300),
            4,
        ));
        let mut peer = tokio::net::TcpStream::connect(addr).await.unwrap();
        peer.write_all(b"GET /files/x HTTP/1.1\r\nHost: x\r\n")
            .await
            .unwrap();
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), peer.read_to_end(&mut rest))
            .await
            .expect("the data plane must close a header-less connection")
            .unwrap();
    }

    // On the HTTPS path: a peer that never finishes its TLS
    // handshake is closed within the same deadline instead of holding a slot.
    #[tokio::test]
    async fn data_plane_closes_a_silent_tls_peer() {
        use tokio::io::AsyncReadExt;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let tls = TlsAcceptor::from(Arc::new(crate::acme::server_config(Arc::default())));
        tokio::spawn(serve_with(
            listener,
            routes(Arc::from(temp_dir("silent-tls").as_path())),
            Some(tls),
            Duration::from_millis(300),
            Duration::from_millis(300),
            4,
        ));
        let mut peer = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), peer.read_to_end(&mut rest))
            .await
            .expect("the data plane must close a silent TLS peer")
            .unwrap();
    }

    // F-231 (RFC 8737): a connection negotiated with ALPN `acme-tls/1` exists
    // only for the CA's handshake, so the data plane closes it without serving
    // HTTP, while a browser's `http/1.1` connection is still served.
    #[tokio::test]
    async fn data_plane_serves_no_http_on_an_acme_tls_connection() {
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let generated = rcgen::generate_simple_self_signed(vec!["node.example".into()]).unwrap();
        let der = generated.cert.der().clone();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            generated.signing_key.serialize_der(),
        ));
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut server = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![der.clone()], key)
            .unwrap();
        server.alpn_protocols = vec![b"http/1.1".to_vec(), b"acme-tls/1".to_vec()];
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_with(
            listener,
            routes(Arc::from(temp_dir("acme-alpn").as_path())),
            Some(TlsAcceptor::from(Arc::new(server))),
            Duration::from_secs(5),
            Duration::from_secs(5),
            4,
        ));

        let mut roots = rustls::RootCertStore::empty();
        roots.add(der).unwrap();
        let exchange = |alpn: &'static [u8]| {
            let mut client = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots.clone())
                .with_no_client_auth();
            client.alpn_protocols = vec![alpn.to_vec()];
            async move {
                let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
                let mut tls = tokio_rustls::TlsConnector::from(Arc::new(client))
                    .connect(ServerName::try_from("node.example").unwrap(), tcp)
                    .await
                    .expect("the handshake completes");
                assert_eq!(tls.get_ref().1.alpn_protocol(), Some(alpn));
                // The write may race the server's close; only the reply matters.
                let _ = tls
                    .write_all(b"GET /files/x HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                    .await;
                let mut reply = Vec::new();
                let read =
                    tokio::time::timeout(Duration::from_secs(3), tls.read_to_end(&mut reply)).await;
                assert!(read.is_ok(), "the connection must end, not hang");
                reply
            }
        };

        let acme = exchange(b"acme-tls/1").await;
        assert!(
            acme.is_empty(),
            "HTTP served over acme-tls/1: {:?}",
            String::from_utf8_lossy(&acme)
        );
        let browser = exchange(b"http/1.1").await;
        assert!(browser.starts_with(b"HTTP/1.1 404"), "{browser:?}");
    }

    // The public data port holds at most `max_connections` sockets; a
    // connection beyond the cap waits until a slot frees instead of being served.
    #[tokio::test]
    async fn data_plane_holds_at_most_max_connections() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_with(
            listener,
            routes(Arc::from(temp_dir("cap").as_path())),
            None,
            Duration::from_secs(30),
            Duration::from_secs(30),
            1,
        ));
        let mut holder = tokio::net::TcpStream::connect(addr).await.unwrap();
        holder
            .write_all(b"GET /files/x HTTP/1.1\r\nHost: x\r\n")
            .await
            .unwrap();

        let mut fresh = tokio::net::TcpStream::connect(addr).await.unwrap();
        fresh
            .write_all(b"GET /files/x HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut reply = Vec::new();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), fresh.read_to_end(&mut reply))
                .await
                .is_err(),
            "a connection beyond the cap was served while the slot was held: {reply:?}"
        );

        drop(holder);
        tokio::time::timeout(Duration::from_secs(3), fresh.read_to_end(&mut reply))
            .await
            .expect("the waiting connection is served once the slot frees")
            .unwrap();
        assert!(reply.starts_with(b"HTTP/1.1 404"), "{reply:?}");
    }

    // F-246: readers that request a big file and never read it are closed once
    // no response byte is accepted for the write-idle deadline, so they cannot
    // hold every slot: a fresh visitor and the CA's TLS-ALPN-01 validation for
    // an origin change still get through.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stalled_readers_cannot_keep_visitors_and_the_ca_out() {
        use crate::acme::fake::{temp_dir, FakeCa};
        use rustls::pki_types::ServerName;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let ca = FakeCa::new(90 * 86_400);
        let dir = temp_dir("stalled");
        let files = dir.join("files");
        std::fs::create_dir_all(&files).unwrap();
        std::fs::File::create(files.join("big.bin"))
            .unwrap()
            .set_len(64 << 20)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        ca.state().port = addr.port();
        let (origin, origin_rx) = watch::channel(Some("https://127.0.0.1".to_string()));
        let (status_tx, mut status) = watch::channel(None);
        let resolver = Arc::new(CertResolver::default());
        tokio::spawn(crate::acme::manage(
            ca.acme(&dir),
            origin_rx,
            Arc::clone(&resolver),
            status_tx,
        ));
        let tls = TlsAcceptor::from(Arc::new(crate::acme::server_config(resolver)));
        tokio::spawn(serve_with(
            listener,
            routes(Arc::from(files.as_path())),
            Some(tls),
            Duration::from_secs(5),
            Duration::from_millis(500),
            4,
        ));
        tokio::time::timeout(
            Duration::from_secs(20),
            status.wait_for(|s| s.as_ref().is_some_and(|s| s.issued_at.is_some())),
        )
        .await
        .expect("the first certificate")
        .unwrap();

        let connect = || async {
            let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
            tokio_rustls::TlsConnector::from(Arc::new(ca.client_config()))
                .connect(ServerName::try_from("127.0.0.1").unwrap(), tcp)
                .await
                .unwrap()
        };
        let mut stalled = Vec::new();
        for _ in 0..4 {
            let mut reader = connect().await;
            reader
                .write_all(b"GET /files/big.bin HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .unwrap();
            stalled.push(reader);
        }

        let visitor = tokio::time::timeout(Duration::from_secs(5), async {
            let mut visitor = connect().await;
            visitor
                .write_all(b"GET /files/none HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut reply = Vec::new();
            let _ = visitor.read_to_end(&mut reply).await;
            reply
        })
        .await;
        assert!(
            visitor
                .as_ref()
                .is_ok_and(|reply| reply.starts_with(b"HTTP/1.1 404")),
            "a fresh visitor was kept out by stalled readers: {visitor:?}"
        );

        let before = ca.state().validations.len();
        origin.send_replace(Some("https://node2.example.test".into()));
        let validated = tokio::time::timeout(Duration::from_secs(10), async {
            while !ca.state().validations[before..].contains(&true) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        assert!(
            validated.is_ok(),
            "the TLS-ALPN-01 validation was kept out by stalled readers: {:?}",
            ca.state().validations
        );
        drop(stalled);
        let _ = std::fs::remove_dir_all(dir);
    }

    // F-246: the write-idle deadline is on progress, not on the whole
    // response, so a slow reader that keeps reading downloads the whole file.
    // F-350: an in-memory pipe on virtual time, so a loaded host cannot starve
    // the reader past the limit.
    #[tokio::test(start_paused = true)]
    async fn a_slow_but_progressing_download_completes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let limit = Duration::from_millis(300);
        let size = 4 << 20;
        let (server, mut reader) = tokio::io::duplex(64 * 1024);
        let writer = tokio::spawn(async move {
            let mut server = WriteIdle::new(server, limit);
            server.write_all(&vec![0u8; size]).await?;
            server.shutdown().await
        });
        let started = tokio::time::Instant::now();
        let mut received = 0;
        let mut chunk = vec![0u8; 32 * 1024];
        loop {
            let n = reader.read(&mut chunk).await.unwrap();
            if n == 0 {
                break;
            }
            received += n;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(received, size, "the download was cut off");
        writer.await.unwrap().unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed > limit * 3,
            "the reader was not slow enough to prove anything: {elapsed:?}"
        );
    }

    // F-359: the default listener takes IPv4 too, even from a socket that
    // starts IPv6-only as every new one does on a `net.ipv6.bindv6only=1` host
    // (macOS and Linux default to dual-stack, so the test hands the default
    // path a socket set IPv6-only: a plain `[::]` bind of it refuses IPv4).
    // F-374: a duplicate of that socket proves the listener is it, bound and
    // made dual-stack; a fresh socket can reuse a dropped one's fd number.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(deprecated)] // std's IPV6_V6ONLY accessors
    async fn the_default_listener_takes_ipv4_too() {
        use std::os::fd::{FromRawFd, IntoRawFd};

        // SAFETY: each wrapper takes sole ownership of the one live socket fd.
        let v6_only = unsafe {
            std::net::TcpListener::from_raw_fd(TcpSocket::new_v6().unwrap().into_raw_fd())
        };
        v6_only.set_only_v6(true).unwrap();
        assert!(v6_only.only_v6().unwrap(), "the socket starts IPv6-only");
        let witness = v6_only.try_clone().unwrap();
        let socket = unsafe { TcpSocket::from_raw_fd(v6_only.into_raw_fd()) };
        let listener = bind_every_address(Ok(socket), 0).await.unwrap();
        let bound = listener.local_addr().unwrap();
        assert!(bound.is_ipv6(), "not listening on IPv6: {bound}");
        assert_eq!(
            witness.local_addr().ok(),
            Some(SocketAddr::from((Ipv6Addr::UNSPECIFIED, bound.port()))),
            "the listener is not the socket it was given"
        );
        assert!(!witness.only_v6().unwrap(), "the socket was left IPv6-only");
        let v4 = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, bound.port())).await;
        assert!(v4.is_ok(), "IPv4 refused by an IPv6-only start: {v4:?}");
    }

    // Once central assigns an origin the data plane serves the speed
    // test over HTTPS with the certificate the agent obtained: a ranged download
    // and an upload, each over a handshake a browser trusting the CA accepts.
    #[tokio::test]
    async fn the_data_plane_serves_downloads_and_uploads_over_https() {
        use crate::acme::fake::{temp_dir, FakeCa, Node};

        let ca = FakeCa::new(6 * 86_400);
        let dir = temp_dir("https");
        std::fs::create_dir_all(dir.join("files")).unwrap();
        std::fs::write(dir.join("files/probe.bin"), b"0123456789ABCDEF").unwrap();
        let mut node = Node::start(&ca, &dir, "https://127.0.0.1").await;
        let status = node.status_until(|status| status.issued_at.is_some()).await;

        let (leaf, download) = node
            .https(
                &ca,
                "127.0.0.1",
                b"GET /files/probe.bin HTTP/1.1\r\nHost: x\r\nRange: bytes=4-7\r\nConnection: close\r\n\r\n",
            )
            .await;
        assert!(download.starts_with(b"HTTP/1.1 206"), "{download:?}");
        assert!(download.ends_with(b"4567"), "{download:?}");
        assert!(
            rustls::server::ParsedCertificate::try_from(&leaf).is_ok(),
            "the issued certificate is served"
        );
        let _ = status;

        let mut upload =
            b"POST /speedtest/upload HTTP/1.1\r\nHost: x\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n"
                .to_vec();
        upload.extend([7u8; 4096]);
        let (_, reply) = node.https(&ca, "127.0.0.1", &upload).await;
        assert!(reply.starts_with(b"HTTP/1.1 200"), "{reply:?}");
        assert!(reply.ends_with(br#"{"bytes":4096}"#), "{reply:?}");
        node.stop().await;
        let _ = std::fs::remove_dir_all(dir);
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

    fn last_error(status: &Option<CertificateStatus>) -> &str {
        status
            .as_ref()
            .and_then(|status| status.last_error.as_deref())
            .unwrap_or_default()
    }

    /// Whether `status` shows one that satisfies `accept` within `secs`.
    async fn reports(
        status: &mut watch::Receiver<Option<CertificateStatus>>,
        secs: u64,
        accept: impl FnMut(&Option<CertificateStatus>) -> bool,
    ) -> bool {
        tokio::time::timeout(Duration::from_secs(secs), status.wait_for(accept))
            .await
            .is_ok_and(|seen| seen.is_ok())
    }

    // F-272: a bad data-plane setting cannot heal, so it is reported (again
    // after a new origin voids the report) and the data plane waits, not retries.
    #[tokio::test]
    async fn an_invalid_data_plane_setting_is_reported_for_each_origin() {
        let dir = temp_dir("bad-config");
        let ca = crate::acme::fake::FakeCa::new(90 * 86_400);
        let (origin, origin_rx) = watch::channel(None);
        let (status_tx, mut status) = watch::channel(None);
        let void = status_tx.clone();
        let invalid = io::Error::other("invalid LG_AGENT_DATA_BIND: probe");
        let task = tokio::spawn(start(Err(invalid), origin_rx, status_tx, ca.acme(&dir)));
        let invalid = |status: &Option<CertificateStatus>| {
            last_error(status) == "invalid LG_AGENT_DATA_BIND: probe"
        };

        origin.send_replace(Some("https://a.example.test".into()));
        assert!(
            reports(&mut status, 3, invalid).await,
            "the invalid setting must be reported: {:?}",
            *status.borrow()
        );

        // The tunnel voids the report when the origin changes.
        void.send_replace(None);
        origin.send_replace(Some("https://b.example.test".into()));
        assert!(
            reports(&mut status, 3, invalid).await,
            "the invalid setting must be reported for the new origin: {:?}",
            *status.borrow()
        );
        assert!(!task.is_finished(), "the data plane waits for a new origin");
        task.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    // F-272: a files root that cannot be created replaces the stale bind error
    // and is retried with the bind's backoff; once it can be, the node serves.
    // Each wait ends on the state; the deadlines are headroom for a loaded host
    // (the CA's validation polls back off 250 ms, 500 ms, 1 s, ...).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_uncreatable_files_root_is_reported_and_retried() {
        let dir = temp_dir("files-root");
        let blocker = dir.join("plain-file");
        std::fs::write(&blocker, b"x").unwrap();
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = busy.local_addr().unwrap();
        let ca = crate::acme::fake::FakeCa::new(90 * 86_400);
        ca.state().port = addr.port();
        let (origin, origin_rx) = watch::channel(None);
        let (status_tx, mut status) = watch::channel(None);
        let config = Ok((Some(addr), blocker.join("files")));
        let task = tokio::spawn(start(config, origin_rx, status_tx, ca.acme(&dir)));

        origin.send_replace(Some("https://a.example.test".into()));
        assert!(
            reports(&mut status, 30, |status| {
                last_error(status).starts_with("cannot listen for HTTPS")
            })
            .await,
            "{:?}",
            *status.borrow()
        );

        drop(busy);
        assert!(
            reports(&mut status, 30, |status| {
                last_error(status).starts_with("cannot create the files directory")
            })
            .await,
            "the files-root error must replace the stale bind error: {:?}",
            *status.borrow()
        );
        assert!(!task.is_finished(), "the files root is retried");

        std::fs::remove_file(&blocker).unwrap();
        assert!(
            reports(&mut status, 60, |status| {
                status
                    .as_ref()
                    .is_some_and(|status| status.issued_at.is_some() && status.last_error.is_none())
            })
            .await,
            "once the root can be created the node serves: {:?}",
            *status.borrow()
        );
        assert!(blocker.join("files").is_dir());
        assert!(!task.is_finished(), "the data plane keeps serving");
        task.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    // F-281: an empty LG_AGENT_FILES_DIR means unset, never the working
    // directory, where the agent keeps its credential. Cargo runs this from
    // crates/agent, whose Cargo.toml stands in for a secret there.
    #[tokio::test]
    async fn an_empty_files_dir_serves_the_default_root_not_the_working_directory() {
        use tower::ServiceExt;

        let (_, root) = config_from(|key| (key == ENV_FILES_DIR).then(String::new)).unwrap();
        assert!(Path::new("Cargo.toml").is_file());
        let app = routes(Arc::from(root.as_path()));
        for uri in ["/files/agent-credential.json", "/files/Cargo.toml"] {
            let response = app
                .clone()
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        }
        assert_eq!(root, PathBuf::from(DEFAULT_FILES_DIR));
    }

    // F-283: `[::]` alone refuses IPv4 on a `net.ipv6.bindv6only=1` host; the
    // default listener takes both families.
    #[tokio::test]
    async fn the_default_listener_takes_ipv4_and_ipv6() {
        let listener = bind_default(0).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        for host in [
            std::net::IpAddr::from(Ipv4Addr::LOCALHOST),
            Ipv6Addr::LOCALHOST.into(),
        ] {
            let connected = tokio::net::TcpStream::connect((host, port)).await;
            assert!(connected.is_ok(), "{host} refused: {connected:?}");
        }
    }

    // F-382: `start`'s own default bind keeps its IPv4 fallback. This host's
    // dual-stack default hides a plain `[::]` bind from the test above, so an
    // IPv6-only listener takes `[::]:port` first: the default bind must still
    // listen on `0.0.0.0:port`, where a plain `[::]` bind fails. The port is
    // reserved on IPv4 first: Linux hands a v6-only `[::]:0` a port an IPv4
    // socket may already hold, where the fallback could not listen (F-383).
    #[cfg(unix)]
    #[tokio::test]
    #[allow(deprecated)] // std's IPV6_V6ONLY accessors
    async fn the_default_bind_falls_back_to_ipv4_when_ipv6_is_taken() {
        use std::os::fd::{FromRawFd, IntoRawFd};

        let reserved = std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let port = reserved.local_addr().unwrap().port();
        // SAFETY: each wrapper takes sole ownership of the one live socket fd.
        let v6_only = unsafe {
            std::net::TcpListener::from_raw_fd(TcpSocket::new_v6().unwrap().into_raw_fd())
        };
        v6_only.set_only_v6(true).unwrap();
        let taken = unsafe { TcpSocket::from_raw_fd(v6_only.into_raw_fd()) };
        taken.bind((Ipv6Addr::UNSPECIFIED, port).into()).unwrap();
        let _taken = taken.listen(1).unwrap();
        drop(reserved);

        let listener = bind_default(port).await;
        let bound = listener.as_ref().map(|l| l.local_addr().unwrap());
        assert_eq!(
            bound.ok(),
            Some(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))),
            "no IPv4 fallback: {listener:?}"
        );
        let v4 = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await;
        assert!(v4.is_ok(), "IPv4 refused: {v4:?}");
    }
}
