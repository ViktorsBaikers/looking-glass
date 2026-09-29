use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use axum::{
    extract::Request,
    handler::Handler,
    http::{header, HeaderValue, StatusCode, Uri},
    middleware::{from_fn, from_fn_with_state, map_response, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use rust_embed::{EmbeddedFile, RustEmbed};
use tower::ServiceBuilder;
use tower_http::{
    compression::CompressionLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod admin_api;
mod administrators;
mod auth;
mod enroll;
mod files;
mod installer;
mod observability;
mod ratelimit;
mod run_api;
mod session;
mod store;
mod stream;
mod tunnel;

pub use enroll::EnrollConfig;
pub use ratelimit::{LoginLimiter, TransportConfig};
pub use run_api::RunService;
pub use session::{RedbSessionStore, COOKIE_NAME as SESSION_COOKIE_NAME};
pub use store::{
    Administrator, Agent, EnrollmentToken, RemoveAdministratorError, Store, StoreError,
};
// The authenticated relay hub this slice produces — the seam the remote-run path
// (Slice 10) submits diagnostics through.
pub use tunnel::{NotConnected, RelayEvent, SubmitError, TunnelHub};

const DEFAULT_DB_PATH: &str = "data/lookingglass.redb";
const DEFAULT_FILES_DIR: &str = "data/files";

/// Shared, cheaply-cloneable application state: the store, the trusted-proxy
/// config, the login limiter, and — while no admin exists — the one-time setup
/// token that create-admin requires (`None` once setup is closed).
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub transport: TransportConfig,
    pub login_limiter: Arc<LoginLimiter>,
    pub setup_token: Option<Arc<str>>,
    pub run: RunService,
    /// Directory the local node serves downloadable test files from. A test
    /// file's `source_ref` is resolved *within* this root (no traversal out),
    /// so the range file server never reaches outside it.
    pub files_root: Arc<std::path::Path>,
    /// Where enrolling agents reach central's HTTPS API endpoint and the identity
    /// they pin it by — the source of the fingerprint the install command carries.
    pub enroll: EnrollConfig,
    /// Live authenticated agent connections, shared by admin revoke and the tunnel
    /// listener so revoke can drop a connected agent immediately.
    pub tunnel_hub: TunnelHub,
}

#[derive(RustEmbed)]
#[folder = "../../frontend/build"]
struct Spa;

/// The production router: opens the store at `LG_DB_PATH` (default
/// `data/lookingglass.redb`) and derives trusted-proxy config from the
/// environment.
pub fn app() -> Router {
    let path = std::env::var("LG_DB_PATH").unwrap_or_else(|_| DEFAULT_DB_PATH.to_string());
    // Store::open creates a missing data dir owner-only.
    let store = Store::open(&path).expect("open redb store");
    let installed = store.is_installed().unwrap_or_else(|error| {
        tracing::error!(%error, "could not read the install state; offering first-run setup");
        false
    });
    let setup_token = if installed {
        None
    } else {
        let (token, _source) =
            setup_token_for_startup(&path).expect("prepare first-run setup token");
        Some(Arc::from(token.as_str()))
    };
    let settings = store.settings().unwrap_or_else(|error| {
        tracing::error!(%error, "could not read the saved settings; using defaults");
        Default::default()
    });
    let files_dir = files_dir_from(std::env::var("LG_FILES_DIR").ok());
    let _ = std::fs::create_dir_all(&files_dir);

    // Enrollment pins the HTTPS API origin that serves `/api/enroll`. The tunnel
    // listener is a separate TLS/WebSocket socket and must not become LG_CENTRAL_URL.
    let tunnel_identity = tunnel::TunnelIdentity::from_env();
    let mut enroll = EnrollConfig::from_env().expect("invalid enrollment configuration");
    if let Some(identity) = &tunnel_identity {
        enroll = enroll.with_tunnel_pin(identity.fingerprint());
    }

    tokio::spawn(session::purge_expired(session::RedbSessionStore::new(
        &store,
    )));

    let tunnel_hub = tunnel::TunnelHub::new();
    let state = AppState {
        run: RunService::from_settings(&settings),
        store: store.clone(),
        transport: TransportConfig::from_env(),
        login_limiter: Arc::new(LoginLimiter::default()),
        setup_token,
        files_root: Arc::from(std::path::Path::new(&files_dir)),
        enroll,
        tunnel_hub: tunnel_hub.clone(),
    };
    let router = build(state);

    if let Some(identity) = tunnel_identity {
        let bind = tunnel::bind_addr();
        tokio::spawn(async move {
            if let Err(error) = tunnel::serve(bind, identity, store, tunnel_hub).await {
                tracing::error!(%error, "agent tunnel listener stopped");
            }
        });
    }
    router
}

/// The files root from `LG_FILES_DIR`'s value. Empty is unset, as on the agent.
fn files_dir_from(value: Option<String>) -> String {
    value
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_FILES_DIR.to_string())
}

fn setup_token_for_startup(db_path: &str) -> io::Result<(String, String)> {
    let (token, source) = setup_token(db_path)?;
    tracing::info!(
        event = "auth.setup_token",
        correlation_id = "startup",
        outcome = "available",
        setup_token_source = %source,
        "first-run setup token available"
    );
    Ok((token, source))
}

fn setup_token(db_path: &str) -> io::Result<(String, String)> {
    if let Ok(token) = std::env::var("LG_SETUP_TOKEN") {
        if !token.is_empty() {
            return Ok((token, "env:LG_SETUP_TOKEN".to_string()));
        }
    }

    let token = installer::generate_setup_token();
    let token_path = setup_token_path(db_path);
    match create_setup_token_file(&token_path, &token) {
        Ok(()) => Ok((token, token_path.display().to_string())),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let token = read_existing_setup_token(&token_path)?;
            Ok((token, token_path.display().to_string()))
        }
        Err(error) => Err(error),
    }
}

fn setup_token_path(db_path: &str) -> PathBuf {
    Path::new(db_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("setup-token")
}

#[cfg(unix)]
fn create_setup_token_file(path: &Path, token: &str) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    verify_setup_token_file(path)
}

#[cfg(not(unix))]
fn create_setup_token_file(path: &Path, token: &str) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()
}

fn read_existing_setup_token(path: &Path) -> io::Result<String> {
    verify_setup_token_file(path)?;
    let token = std::fs::read_to_string(path)?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "setup token file is empty",
        ));
    }
    Ok(token)
}

#[cfg(unix)]
fn verify_setup_token_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "setup token path must be a regular file",
        ));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode != 0o600 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "setup token file must be mode 0600",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_setup_token_file(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_file() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "setup token path must be a regular file",
        ))
    }
}

/// Compose the full router from an explicit state — the seam tests inject an
/// isolated store through.
pub fn build(state: AppState) -> Router {
    let session_layer = session::session_layer(session::RedbSessionStore::new(&state.store));
    let admin = api_routes(state.clone())
        .layer(session_layer)
        .layer(from_fn(refuse_cross_origin_writes));
    let run = run_api::routes(state.clone());
    let files = files::routes(state.clone());
    let public = admin_api::public_routes(state.clone());
    let api = admin
        .merge(run)
        .merge(files)
        .merge(public)
        .merge(administrators::activation_routes(state.clone()))
        .layer(from_fn_with_state(state.clone(), installer::require_setup));
    with_routes(api)
}

fn api_routes(state: AppState) -> Router {
    Router::new()
        .route("/api/setup/status", get(installer::setup_status))
        .route("/api/setup", post(installer::create_admin))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/admin/me", get(auth::me))
        .merge(admin_api::admin_routes())
        .merge(administrators::admin_routes())
        .merge(enroll::routes())
        .with_state(state)
}

/// Refuses a state-changing admin request that a browser sent from another
/// origin: `SameSite=Strict` still attaches the session cookie to a
/// form posted from a same-site sibling origin. The origin test is the public
/// run endpoint's [`run_api::same_origin`]; a request with no browser origin
/// signal at all (an agent, curl) passes, as the session still gates it.
async fn refuse_cross_origin_writes(request: Request, next: Next) -> Response {
    let headers = request.headers();
    let from_browser = headers.contains_key(header::ORIGIN)
        || headers.contains_key(header::REFERER)
        || headers.contains_key("sec-fetch-site");
    if !request.method().is_safe() && from_browser && !run_api::same_origin(headers) {
        return auth::ApiError::Coded(
            StatusCode::FORBIDDEN,
            "cross_origin_refused",
            "This action must be started from this site.",
        )
        .into_response();
    }
    next.run(request).await
}

pub fn with_routes(features: Router) -> Router {
    features
        .route("/health", get(health))
        // Compression wraps only the SPA: Test file downloads must reach the
        // browser byte-for-byte or the Speed test would measure compression.
        .fallback(serve_spa.layer(CompressionLayer::new()))
        .layer(map_response(harden))
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
                .layer(
                    TraceLayer::new_for_http().make_span_with(|request: &Request| {
                        tracing::debug_span!(
                            "request",
                            method = %request.method(),
                            path = %loggable_path(request.uri().path()),
                        )
                    }),
                )
                .layer(PropagateRequestIdLayer::x_request_id()),
        )
}

/// The request path as the trace span records it: the segment after
/// `activate` is a live activation token and is masked, and the query (a
/// visitor's run target) is left out.
fn loggable_path(path: &str) -> String {
    let mut after_activate = false;
    path.split('/')
        .map(|segment| {
            let shown = if after_activate { "{token}" } else { segment };
            after_activate = segment == "activate";
            shown
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Browser hardening on every response.
async fn harden(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, CSP.clone());
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // Not `no-referrer`: same-origin requests keep their full Referer.
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    response
}

/// Scripts may come only from this origin or be one of the embedded build's
/// inline scripts (the theme and SvelteKit bootstraps), admitted by hash so the
/// list always matches the build that is actually served.
static CSP: LazyLock<HeaderValue> = LazyLock::new(|| {
    let mut hashes = std::collections::BTreeSet::new();
    for path in Spa::iter().filter(|path| path.ends_with(".html")) {
        let Some(file) = Spa::get(&path) else {
            continue;
        };
        let html = String::from_utf8_lossy(&file.data);
        for tag in html.split("<script").skip(1) {
            let Some((attributes, rest)) = tag.split_once('>') else {
                continue;
            };
            let Some((script, _)) = rest.split_once("</script>") else {
                continue;
            };
            if !attributes.contains("src=") {
                hashes.insert(format!(" 'sha256-{}'", sha256_base64(script.as_bytes())));
            }
        }
    }
    let policy = format!(
        "script-src 'self'{}; object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
        hashes.into_iter().collect::<String>()
    );
    HeaderValue::try_from(policy).expect("the CSP is plain ASCII")
});

/// Padded base64 of a SHA-256 digest, the form CSP script hashes take.
fn sha256_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let hex = shared::protocol::sha256_hex(bytes);
    let digest: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("sha256_hex is hex"))
        .collect();
    let mut out = String::new();
    for chunk in digest.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

/// Serve the web surface on `listener`, bounded against idle and slow peers: a
/// request header (and a keep-alive connection's next one) must arrive within
/// [`HEADER_READ_TIMEOUT`], a request body or a response that makes no progress
/// for [`IDLE_TIMEOUT`] ends its connection, a request body ends at
/// [`REQUEST_BODY_DEADLINE`] however it trickles, and at most [`MAX_CONNECTIONS`]
/// sockets are held, so unauthenticated peers cannot pile up until accept fails
/// with EMFILE or hold every slot. One peer address other than a trusted proxy
/// holds at most [`MAX_CONNECTIONS_PER_PEER`] of them; behind a trusted proxy,
/// one forwarded client has at most that many requests in progress, each
/// counted until its response body has been sent.
pub async fn serve(listener: tokio::net::TcpListener, app: Router) -> io::Result<()> {
    let limits = Limits {
        header_read: HEADER_READ_TIMEOUT,
        idle: IDLE_TIMEOUT,
        body: REQUEST_BODY_DEADLINE,
        connections: MAX_CONNECTIONS,
        per_client: MAX_CONNECTIONS_PER_PEER,
    };
    serve_with(listener, app, limits, TransportConfig::from_env(), |peer| {
        peer.ip()
    })
    .await
}

const HEADER_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// How long a request body may go without a frame, or a response write stay
/// blocked, before the connection is dropped. It measures progress, not length:
/// a slow transfer or a quiet run stream is never cut by it.
const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// The most a whole request body may take, however slowly it trickles in. Longer
/// than the speed-test upload's own 30 s bound, which answers 408 first.
const REQUEST_BODY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(40);
const MAX_CONNECTIONS: usize = 512;
/// The share of [`MAX_CONNECTIONS`] one peer address (an IPv6 /64) may hold, so
/// a single address cannot take every slot. A trusted proxy is exempt, but each
/// client it forwards may have only this many requests in progress.
const MAX_CONNECTIONS_PER_PEER: usize = 32;

/// The web listener's bounds (see [`serve`]); tests shrink them.
#[derive(Clone, Copy)]
struct Limits {
    header_read: std::time::Duration,
    idle: std::time::Duration,
    body: std::time::Duration,
    connections: usize,
    per_client: usize,
}

async fn serve_with(
    listener: tokio::net::TcpListener,
    app: Router,
    limits: Limits,
    transport: TransportConfig,
    peer_ip: impl Fn(std::net::SocketAddr) -> std::net::IpAddr,
) -> io::Result<()> {
    use axum::extract::{ConnectInfo, Request};
    use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
    use hyper_util::server::conn::auto::Builder;
    use hyper_util::service::TowerToHyperService;
    use tower::ServiceExt;
    use tower_http::timeout::RequestBodyTimeout;

    let slots = Arc::new(tokio::sync::Semaphore::new(limits.connections));
    let peers = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    // HTTP/1 only (axum's default features): no version sniffing ahead of the
    // header timer. hyper re-arms that timer while a keep-alive connection idles.
    let mut builder = Builder::new(TokioExecutor::new()).http1_only();
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(limits.header_read);
    loop {
        // A full house leaves further peers in the kernel backlog, not in our fd table.
        let slot = Arc::clone(&slots)
            .acquire_owned()
            .await
            .expect("connection slots are never closed");
        let (tcp, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "web listener accept failed");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };
        let ip = peer_ip(peer);
        let peer_slot = match transport.connection_key(ip) {
            None => None,
            Some(key) => match PeerSlot::claim(&peers, key, limits.per_client) {
                Some(share) => Some(share),
                None => {
                    tracing::debug!(%peer, "web listener refused a peer over its connection share");
                    continue;
                }
            },
        };
        let app = RequestBodyTimeout::new(app.clone(), limits.idle);
        let (transport, clients) = (transport.clone(), Arc::clone(&peers));
        let service = TowerToHyperService::new(tower::service_fn(move |request: Request<_>| {
            // Every client of a trusted proxy arrives from its address, so each
            // one it forwards holds requests in progress, not connections, to
            // the share a direct peer gets. Over it, the request is refused
            // before its body is read, which ends the connection.
            let share = transport
                .forwarded_client_key(ip, request.headers())
                .map(|key| PeerSlot::claim(&clients, key, limits.per_client));
            let run_stream = request.uri().path() == "/api/run/stream";
            let mut request =
                request.map(|body| body_deadline(axum::body::Body::new(body), limits.body));
            request.extensions_mut().insert(ConnectInfo(peer));
            let app = app.clone();
            async move {
                match share {
                    None => app.oneshot(request).await,
                    Some(None) => {
                        tracing::debug!(%peer, "web listener refused a forwarded client over its share");
                        // An EventSource cannot read a non-200 body, so a run
                        // stream is refused in-band with run_api's rate-limit text.
                        Ok(if run_stream {
                            stream::sse_refusal(
                                "too many requests — please wait a moment and try again"
                                    .to_string(),
                            )
                            .into_response()
                        } else {
                            auth::ApiError::RateLimited.into_response()
                        })
                    }
                    Some(Some(share)) => Ok(hold_until_sent(app.oneshot(request).await?, share)),
                }
            }
        }));
        let builder = builder.clone();
        tokio::spawn(async move {
            let _slot = (slot, peer_slot);
            let io = TokioIo::new(WriteDeadline {
                io: tcp,
                stall: limits.idle,
                blocked: None,
            });
            if let Err(error) = builder.serve_connection(io, service).await {
                tracing::debug!(%peer, %error, "web connection ended");
            }
        });
    }
}

/// `body`, failing once `limit` has passed since the request arrived, however
/// slowly it trickles, so a slow-rate body cannot hold its connection slot.
fn body_deadline(body: axum::body::Body, limit: std::time::Duration) -> axum::body::Body {
    use futures_util::StreamExt;
    use std::future::Future;
    let mut data = body.into_data_stream();
    let mut expired = Box::pin(tokio::time::sleep(limit));
    axum::body::Body::from_stream(futures_util::stream::poll_fn(move |cx| {
        match data.poll_next_unpin(cx) {
            std::task::Poll::Pending if expired.as_mut().poll(cx).is_ready() => {
                std::task::Poll::Ready(Some(Err(axum::Error::new(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "request body deadline passed",
                )))))
            }
            poll => poll,
        }
    }))
}

/// `response`, holding `share` until its body has been sent or dropped, so a
/// client that reads its downloads slowly still counts them. The wrapped body
/// keeps the length hyper would have sent; it drops trailers, which no handler
/// here sends.
fn hold_until_sent(
    response: axum::response::Response,
    share: PeerSlot,
) -> axum::response::Response {
    use axum::body::HttpBody;
    use futures_util::StreamExt;
    let (mut parts, body) = response.into_parts();
    if body.is_end_stream() {
        return axum::response::Response::from_parts(parts, body);
    }
    if let Some(length) = body.size_hint().exact() {
        parts
            .headers
            .entry(axum::http::header::CONTENT_LENGTH)
            .or_insert(length.into());
    }
    let mut data = body.into_data_stream();
    let mut share = Some(share);
    let body = axum::body::Body::from_stream(futures_util::stream::poll_fn(move |cx| {
        let poll = data.poll_next_unpin(cx);
        if let std::task::Poll::Ready(None) = poll {
            drop(share.take());
        }
        poll
    }));
    axum::response::Response::from_parts(parts, body)
}

/// One live connection (or, behind a trusted proxy, one request in progress)
/// counted against its peer or client key; dropping it returns the share.
struct PeerSlot {
    peers: Arc<std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, usize>>>,
    key: std::net::IpAddr,
}

impl PeerSlot {
    /// Count one more against `key`, or `None` if it already holds `share`.
    fn claim(
        peers: &Arc<std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, usize>>>,
        key: std::net::IpAddr,
        share: usize,
    ) -> Option<Self> {
        let mut counts = peers.lock().expect("peer connection counts");
        let count = counts.entry(key).or_insert(0);
        if *count >= share {
            return None;
        }
        *count += 1;
        Some(Self {
            peers: Arc::clone(peers),
            key,
        })
    }
}

impl Drop for PeerSlot {
    fn drop(&mut self) {
        let mut counts = self.peers.lock().expect("peer connection counts");
        if let Some(count) = counts.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.key);
            }
        }
    }
}

/// A socket whose writes fail once one has stayed blocked for `stall`, so a peer
/// that stops reading a response cannot hold its connection slot. Any write that
/// completes resets the clock.
struct WriteDeadline {
    io: tokio::net::TcpStream,
    stall: std::time::Duration,
    blocked: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
}

impl WriteDeadline {
    fn progress<T>(
        &mut self,
        cx: &mut std::task::Context<'_>,
        poll: std::task::Poll<io::Result<T>>,
    ) -> std::task::Poll<io::Result<T>> {
        use std::future::Future;
        if poll.is_ready() {
            self.blocked = None;
            return poll;
        }
        let stall = self.stall;
        let blocked = self
            .blocked
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(stall)));
        match blocked.as_mut().poll(cx) {
            std::task::Poll::Ready(()) => {
                std::task::Poll::Ready(Err(io::ErrorKind::TimedOut.into()))
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl tokio::io::AsyncRead for WriteDeadline {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for WriteDeadline {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        let poll = std::pin::Pin::new(&mut self.io).poll_write(cx, buf);
        self.progress(cx, poll)
    }

    fn poll_write_vectored(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> std::task::Poll<io::Result<usize>> {
        let poll = std::pin::Pin::new(&mut self.io).poll_write_vectored(cx, bufs);
        self.progress(cx, poll)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let poll = std::pin::Pin::new(&mut self.io).poll_flush(cx);
        self.progress(cx, poll)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let poll = std::pin::Pin::new(&mut self.io).poll_shutdown(cx);
        self.progress(cx, poll)
    }
}

pub fn init_tracing() {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn serve_spa(uri: Uri) -> Response {
    let requested = uri.path().trim_start_matches('/');

    if requested == "api" || requested.starts_with("api/") {
        return auth::ApiError::NotFound.into_response();
    }

    let requested = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };

    if let Some(file) = Spa::get(requested) {
        return embedded_response(requested, file);
    }
    // `index.html` is the prerendered `/`; other client routes get the
    // route-agnostic shell so they don't preload the homepage's chunks.
    match Spa::get("200.html") {
        Some(file) => embedded_response("200.html", file),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Vite fingerprints everything under `_app/immutable/`, so those bytes never
/// change at a URL and can be cached for good. Everything else — the shell and
/// any miss that fell back to it — must revalidate so a new deploy is picked up.
fn embedded_response(path: &str, file: EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("_app/immutable/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_ref()),
            (header::CACHE_CONTROL, cache),
        ],
        file.data.into_owned(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc as StdArc, Mutex};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // F-298: an empty LG_FILES_DIR is unset, as the agent treats its own, so
    // local test files are served from the default root, not 404ed.
    #[test]
    fn an_empty_files_dir_falls_back_to_the_default() {
        assert_eq!(files_dir_from(Some(String::new())), DEFAULT_FILES_DIR);
        assert_eq!(files_dir_from(None), DEFAULT_FILES_DIR);
        assert_eq!(files_dir_from(Some("/srv/files".to_string())), "/srv/files");
    }

    #[test]
    fn generated_setup_token_file_is_owner_only_and_secret_free_in_logs() {
        let _env = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LG_SETUP_TOKEN");
        let root = temp_dir("generated");
        std::fs::create_dir_all(&root).unwrap();
        let db_path = root.join("lookingglass.redb");
        let token_path = root.join("setup-token");
        let (logs, _guard) = captured_logs();

        let (token, source) = setup_token_for_startup(db_path.to_str().unwrap()).unwrap();

        assert_eq!(source, token_path.display().to_string());
        assert_eq!(std::fs::read_to_string(&token_path).unwrap().trim(), token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let captured = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        assert!(
            contains_field(&captured, "event", "auth.setup_token"),
            "{captured}"
        );
        assert!(
            contains_field(&captured, "correlation_id", "startup"),
            "{captured}"
        );
        assert!(!captured.contains(&token), "setup token leaked into logs");
    }

    #[test]
    fn env_setup_token_is_not_written_or_logged() {
        let _env = ENV_LOCK.lock().unwrap();
        let root = temp_dir("env");
        std::fs::create_dir_all(&root).unwrap();
        let db_path = root.join("lookingglass.redb");
        let token_path = root.join("setup-token");
        let secret = format!(
            "env-token-secret-slice13-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        std::env::set_var("LG_SETUP_TOKEN", &secret);
        let (logs, _guard) = captured_logs();

        let (token, source) = setup_token_for_startup(db_path.to_str().unwrap()).unwrap();

        std::env::remove_var("LG_SETUP_TOKEN");
        assert_eq!(token, secret);
        assert_eq!(source, "env:LG_SETUP_TOKEN");
        assert!(!token_path.exists());
        let captured = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        assert!(
            contains_field(&captured, "event", "auth.setup_token"),
            "{captured}"
        );
        assert!(
            !captured.contains(&secret),
            "env setup token leaked into logs"
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_unsafe_setup_token_paths_are_rejected() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let _env = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LG_SETUP_TOKEN");

        let symlink_root = temp_dir("symlink");
        std::fs::create_dir_all(&symlink_root).unwrap();
        let symlink_db = symlink_root.join("lookingglass.redb");
        let symlink_path = symlink_root.join("setup-token");
        let target = symlink_root.join("target-token");
        std::fs::write(&target, "existing-secret").unwrap();
        symlink(&target, &symlink_path).unwrap();
        let error = setup_token(symlink_db.to_str().unwrap()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);

        let loose_root = temp_dir("loose");
        std::fs::create_dir_all(&loose_root).unwrap();
        let loose_db = loose_root.join("lookingglass.redb");
        let loose_path = loose_root.join("setup-token");
        std::fs::write(&loose_path, "existing-secret\n").unwrap();
        let mut permissions = std::fs::metadata(&loose_path).unwrap().permissions();
        permissions.set_mode(0o644);
        std::fs::set_permissions(&loose_path, permissions).unwrap();
        let error = setup_token(loose_db.to_str().unwrap()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    // A restart before the first administrator exists reuses the token
    // file the first start wrote; an empty token file is refused.
    #[cfg(unix)]
    #[test]
    fn a_restart_reuses_the_existing_setup_token_file() {
        let _env = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LG_SETUP_TOKEN");
        let root = temp_dir("restart");
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("lookingglass.redb");
        let db = db.to_str().unwrap();

        let (first, first_source) = setup_token(db).unwrap();
        let (again, again_source) = setup_token(db).unwrap();
        assert_eq!(again, first, "a restart keeps the printed setup token");
        assert_eq!(again_source, first_source);

        let empty_root = temp_dir("empty");
        std::fs::create_dir_all(&empty_root).unwrap();
        let empty = empty_root.join("setup-token");
        std::fs::write(&empty, "\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&empty, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let error =
            setup_token(empty_root.join("lookingglass.redb").to_str().unwrap()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    /// A child process that inherits none of this process's LG_* environment:
    /// sibling tests set LG_* variables unlocked, so a child sets its own (F-375).
    fn child_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
        let mut command = std::process::Command::new(program);
        command
            .env_clear()
            .envs(std::env::vars_os().filter(|(key, _)| !key.to_string_lossy().starts_with("LG_")));
        command
    }

    // Central refuses to start on an LG_TUNNEL_URL the agent
    // cannot dial, instead of serving install commands that carry it.
    #[test]
    fn app_refuses_to_start_with_an_undialable_tunnel_url() {
        let root = temp_dir("bad-tunnel-url");
        std::fs::create_dir_all(&root).unwrap();
        let output = child_command(std::env::current_exe().unwrap())
            .args(["tests::owner_only_store_child", "--exact", "--ignored"])
            .env("LG_DB_PATH", root.join("data").join("lookingglass.redb"))
            .env("LG_FILES_DIR", root.join("files"))
            .env("LG_TUNNEL_URL", "https://tunnel.central.example")
            .output()
            .unwrap();
        let logs = String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "central started with a portless LG_TUNNEL_URL: {logs}"
        );
        assert!(logs.contains("invalid enrollment configuration"), "{logs}");
    }

    // F-371: startup still falls back on a store record it cannot read (a
    // first-run setup token, default settings), but the cause reaches the log.
    #[test]
    fn startup_store_read_failures_are_logged_with_their_cause() {
        let root = temp_dir("startup-store-error");
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("data").join("lookingglass.redb");
        {
            let store = store::Store::open(&db).unwrap();
            let txn = store.database().begin_write().unwrap();
            txn.open_table(store::SETUP)
                .unwrap()
                .insert("state", b"{".as_slice())
                .unwrap();
            txn.open_table(store::SETTINGS)
                .unwrap()
                .insert("global", b"{".as_slice())
                .unwrap();
            txn.commit().unwrap();
        }
        let cause = serde_json::from_slice::<serde_json::Value>(b"{")
            .unwrap_err()
            .to_string();

        let output = child_command(std::env::current_exe().unwrap())
            .args(["tests::owner_only_store_child", "--exact", "--ignored"])
            .env("LG_DB_PATH", &db)
            .env("LG_FILES_DIR", root.join("files"))
            .env("RUST_LOG", "info")
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        let logs = String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "central failed to start: {logs}");
        let logged = logs
            .lines()
            .filter(|line| line.contains("ERROR") && line.contains(&cause))
            .count();
        assert_eq!(logged, 2, "{logs}");
    }

    /// The child half of the owner-only store test: runs the real `app()` in a
    /// re-exec'd copy of this test binary. Ignored so a normal run skips it.
    #[tokio::test]
    #[ignore = "child half of store_and_new_data_dir_are_owner_only_under_umask_022"]
    async fn owner_only_store_child() {
        init_tracing();
        let _ = app();
    }

    // The store holds the session-cookie signing key and live session
    // ids, so the production open keeps it owner-only under a permissive umask,
    // and tightens a store an older release left world-readable.
    #[cfg(unix)]
    #[test]
    fn store_and_new_data_dir_are_owner_only_under_umask_022() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("owner-only-store");
        std::fs::create_dir_all(&root).unwrap();
        let data = root.join("data");
        let db = data.join("lookingglass.redb");
        let start = || {
            let status = child_command("sh")
                .args(["-c", "umask 022 && exec \"$0\" \"$@\""])
                .arg(std::env::current_exe().unwrap())
                .args(["tests::owner_only_store_child", "--exact", "--ignored"])
                .env("LG_DB_PATH", &db)
                .env("LG_FILES_DIR", root.join("files"))
                .status()
                .unwrap();
            assert!(status.success(), "central failed to start: {status}");
        };
        let mode = |path: &Path| {
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            format!("{:o}", mode & 0o777)
        };

        start();
        assert_eq!(mode(&db), "600", "new store file");
        assert_eq!(mode(&data), "700", "new data dir");

        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();
        start();
        assert_eq!(mode(&db), "600", "existing store tightened on start");
    }

    const BIG_RESPONSE: usize = 32 * 1024 * 1024;

    fn listener_app() -> Router {
        use axum::extract::ConnectInfo;
        use axum::response::sse::{Event, Sse};
        use futures_util::StreamExt;
        Router::new()
            .route("/health", get(health))
            .route(
                "/peer",
                get(
                    |ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>| async move {
                        peer.ip().to_string()
                    },
                ),
            )
            .route(
                "/body",
                post(|body: axum::body::Bytes| async move { body.len().to_string() }),
            )
            // Larger than the loopback socket buffers, so an unread response blocks.
            .route("/big", get(|| async { vec![0u8; BIG_RESPONSE] }))
            // Two events with three idle periods of silence between them.
            .route(
                "/sse",
                get(|| async {
                    Sse::new(futures_util::stream::iter(["first", "second"]).then(
                        |data| async move {
                            if data == "second" {
                                tokio::time::sleep(std::time::Duration::from_millis(900)).await;
                            }
                            Ok::<_, std::convert::Infallible>(Event::default().data(data))
                        },
                    ))
                }),
            )
    }

    /// A listener with a 300 ms idle timeout, a 2 s body deadline and a header
    /// timer long enough that it never decides these tests.
    async fn idle_listener(max_connections: usize) -> std::net::SocketAddr {
        use std::time::Duration;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let limits = Limits {
            header_read: Duration::from_secs(10),
            idle: Duration::from_millis(300),
            body: Duration::from_secs(2),
            connections: max_connections,
            per_client: max_connections,
        };
        tokio::spawn(serve_with(
            listener,
            listener_app(),
            limits,
            TransportConfig::default(),
            |peer: std::net::SocketAddr| peer.ip(),
        ));
        addr
    }

    async fn health_answers(addr: std::net::SocketAddr, within: std::time::Duration) -> bool {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let attempt = async {
            let mut stream = tokio::net::TcpStream::connect(addr).await.ok()?;
            stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .await
                .ok()?;
            let mut reply = Vec::new();
            stream.read_to_end(&mut reply).await.ok()?;
            Some(reply.starts_with(b"HTTP/1.1 200"))
        };
        matches!(tokio::time::timeout(within, attempt).await, Ok(Some(true)))
    }

    // F-151: peers that send a complete header and then stall the body are
    // closed after the idle timeout, so they cannot hold every slot.
    #[tokio::test]
    async fn web_listener_closes_connections_that_stall_the_request_body() {
        use tokio::io::AsyncWriteExt;
        let addr = idle_listener(2).await;
        let mut stalled = Vec::new();
        for _ in 0..2 {
            let mut peer = tokio::net::TcpStream::connect(addr).await.unwrap();
            peer.write_all(b"POST /body HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\n{\"user")
                .await
                .unwrap();
            stalled.push(peer);
        }
        assert!(
            health_answers(addr, std::time::Duration::from_secs(3)).await,
            "stalled request bodies must not hold every connection slot"
        );
        drop(stalled);
    }

    // F-151: peers that never read a response are closed once the write has
    // been blocked for the idle timeout, so they cannot hold every slot.
    #[tokio::test]
    async fn web_listener_closes_connections_that_never_read_the_response() {
        use tokio::io::AsyncWriteExt;
        let addr = idle_listener(2).await;
        let mut unread = Vec::new();
        for _ in 0..2 {
            let mut peer = tokio::net::TcpStream::connect(addr).await.unwrap();
            peer.write_all(b"GET /big HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .unwrap();
            unread.push(peer);
        }
        assert!(
            health_answers(addr, std::time::Duration::from_secs(3)).await,
            "unread responses must not hold every connection slot"
        );
        drop(unread);
    }

    // The idle timeout cuts only connections that stop making progress: an
    // upload and a download that each outlast it in small steps, and an SSE
    // stream that stays quiet for three idle periods, all complete.
    #[tokio::test]
    async fn web_listener_keeps_slow_transfers_and_quiet_streams() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;
        use tokio::time::{sleep, timeout, Duration};
        let addr = idle_listener(8).await;

        let upload = async {
            let mut peer = TcpStream::connect(addr).await.unwrap();
            peer.write_all(
                b"POST /body HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 10\r\n\r\n",
            )
            .await
            .unwrap();
            for _ in 0..10 {
                sleep(Duration::from_millis(100)).await;
                peer.write_all(b"x").await.unwrap();
            }
            let mut reply = Vec::new();
            peer.read_to_end(&mut reply).await.unwrap();
            String::from_utf8_lossy(&reply).into_owned()
        };
        let download = async {
            let mut peer = TcpStream::connect(addr).await.unwrap();
            peer.write_all(b"GET /big HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut chunk = vec![0u8; 1024 * 1024];
            let mut total = 0;
            for step in 0.. {
                if step < 10 {
                    sleep(Duration::from_millis(100)).await;
                }
                let mut filled = 0;
                while filled < chunk.len() {
                    match peer.read(&mut chunk[filled..]).await.unwrap() {
                        0 => break,
                        n => filled += n,
                    }
                }
                total += filled;
                if filled < chunk.len() {
                    break;
                }
            }
            total
        };
        let stream = async {
            let mut peer = TcpStream::connect(addr).await.unwrap();
            peer.write_all(b"GET /sse HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut reply = Vec::new();
            peer.read_to_end(&mut reply).await.unwrap();
            String::from_utf8_lossy(&reply).into_owned()
        };

        let (upload, download, stream) = timeout(Duration::from_secs(10), async {
            tokio::join!(upload, download, stream)
        })
        .await
        .expect("slow transfers and quiet streams complete");
        assert!(
            upload.starts_with("HTTP/1.1 200") && upload.ends_with("\r\n\r\n10"),
            "{upload}"
        );
        assert!(
            download > BIG_RESPONSE,
            "download cut after {download} bytes"
        );
        assert!(
            stream.contains("data: first") && stream.contains("data: second"),
            "{stream}"
        );
    }

    // The web listener closes a peer that never finishes its request
    // header, and a keep-alive connection left idle, instead of holding either
    // socket forever. Handlers still see the peer address.
    #[tokio::test]
    async fn web_listener_closes_header_less_and_idle_connections() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::time::{timeout, Duration};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let limits = Limits {
            header_read: Duration::from_millis(300),
            idle: Duration::from_millis(300),
            body: Duration::from_secs(10),
            connections: 8,
            per_client: 8,
        };
        tokio::spawn(serve_with(
            listener,
            listener_app(),
            limits,
            TransportConfig::default(),
            |peer: std::net::SocketAddr| peer.ip(),
        ));

        let mut header_less = tokio::net::TcpStream::connect(addr).await.unwrap();
        header_less
            .write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n")
            .await
            .unwrap();
        let mut idle = tokio::net::TcpStream::connect(addr).await.unwrap();
        idle.write_all(b"GET /peer HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();

        let mut rest = Vec::new();
        timeout(Duration::from_secs(3), header_less.read_to_end(&mut rest))
            .await
            .expect("a header-less connection must be closed by the server")
            .unwrap();
        let mut reply = Vec::new();
        timeout(Duration::from_secs(3), idle.read_to_end(&mut reply))
            .await
            .expect("an idle keep-alive connection must be closed by the server")
            .unwrap();
        let reply = String::from_utf8_lossy(&reply);
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert!(
            reply.ends_with("127.0.0.1"),
            "peer address reaches handlers: {reply}"
        );
    }

    // The web listener holds at most `max_connections` sockets; a further
    // peer waits in the backlog until a slot frees instead of costing a descriptor.
    #[tokio::test]
    async fn web_listener_holds_at_most_max_connections() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::time::{timeout, Duration, Instant};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let limits = Limits {
            header_read: Duration::from_millis(400),
            idle: Duration::from_millis(400),
            body: Duration::from_secs(10),
            connections: 2,
            per_client: 2,
        };
        tokio::spawn(serve_with(
            listener,
            listener_app(),
            limits,
            TransportConfig::default(),
            |peer: std::net::SocketAddr| peer.ip(),
        ));

        let mut holders = Vec::new();
        for _ in 0..2 {
            let mut holder = tokio::net::TcpStream::connect(addr).await.unwrap();
            holder
                .write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n")
                .await
                .unwrap();
            holders.push(holder);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        let started = Instant::now();
        let mut fresh = tokio::net::TcpStream::connect(addr).await.unwrap();
        fresh
            .write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut reply = Vec::new();
        timeout(Duration::from_secs(3), fresh.read_to_end(&mut reply))
            .await
            .expect("the waiting connection is served once a slot frees")
            .unwrap();
        assert!(reply.starts_with(b"HTTP/1.1 200"));
        assert!(
            started.elapsed() >= Duration::from_millis(250),
            "a connection beyond the cap must wait for a free slot, served after {:?}",
            started.elapsed()
        );
        drop(holders);
    }

    const ATTACKER: std::net::IpAddr = std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 66));

    /// A listener with 4 slots, 2 per peer (or forwarded client) and a 10 s idle
    /// timeout and body deadline, standing in for a body trickled just fast
    /// enough to outlive the production idle timeout.
    /// Loopback has one address, so connections from the local ports in the
    /// returned set are seen as coming from [`ATTACKER`].
    async fn two_peer_listener(
        transport: TransportConfig,
    ) -> (
        std::net::SocketAddr,
        StdArc<Mutex<std::collections::HashSet<u16>>>,
    ) {
        use std::time::Duration;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ports = StdArc::new(Mutex::new(std::collections::HashSet::new()));
        let attacker_ports = StdArc::clone(&ports);
        let limits = Limits {
            header_read: Duration::from_secs(10),
            idle: Duration::from_secs(10),
            body: Duration::from_secs(10),
            connections: 4,
            per_client: 2,
        };
        tokio::spawn(serve_with(
            listener,
            listener_app(),
            limits,
            transport,
            move |peer: std::net::SocketAddr| {
                let attacker = attacker_ports.lock().unwrap().contains(&peer.port());
                if attacker {
                    ATTACKER
                } else {
                    peer.ip()
                }
            },
        ));
        (addr, ports)
    }

    async fn connect_as_attacker(
        addr: std::net::SocketAddr,
        ports: &Mutex<std::collections::HashSet<u16>>,
    ) -> tokio::net::TcpStream {
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        ports
            .lock()
            .unwrap()
            .insert(socket.local_addr().unwrap().port());
        socket.connect(addr).await.unwrap()
    }

    /// `ATTACKER` opens `count` connections that each hold a request body open,
    /// sending the extra header lines `headers`.
    async fn stall_as_attacker(
        addr: std::net::SocketAddr,
        ports: &Mutex<std::collections::HashSet<u16>>,
        count: usize,
        headers: &str,
    ) -> Vec<tokio::net::TcpStream> {
        use tokio::io::AsyncWriteExt;
        let mut held = Vec::new();
        for _ in 0..count {
            let mut peer = connect_as_attacker(addr, ports).await;
            let request =
                format!("POST /body HTTP/1.1\r\nHost: x\r\n{headers}Content-Length: 100\r\n\r\n{{");
            // A refused connection may already be closed; that is the point.
            let _ = peer.write_all(request.as_bytes()).await;
            held.push(peer);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        held
    }

    /// Whether `ATTACKER` gets a /health answer on a fresh connection sending the
    /// extra header lines `headers`, retrying until `within` runs out.
    async fn attacker_served(
        addr: std::net::SocketAddr,
        ports: &Mutex<std::collections::HashSet<u16>>,
        headers: &str,
        within: std::time::Duration,
    ) -> bool {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let deadline = tokio::time::Instant::now() + within;
        let request =
            format!("GET /health HTTP/1.1\r\nHost: x\r\n{headers}Connection: close\r\n\r\n");
        while tokio::time::Instant::now() < deadline {
            let attempt = async {
                let mut stream = connect_as_attacker(addr, ports).await;
                stream.write_all(request.as_bytes()).await.ok()?;
                let mut reply = Vec::new();
                stream.read_to_end(&mut reply).await.ok()?;
                Some(reply.starts_with(b"HTTP/1.1 200"))
            };
            if let Ok(Some(true)) = tokio::time::timeout_at(deadline, attempt).await {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        false
    }

    // F-173: one peer address holding request bodies open gets at most its
    // share of the slots, so another address is still served; the share frees
    // as that peer's connections end.
    #[tokio::test]
    async fn web_listener_caps_connections_per_peer_address() {
        let (addr, ports) = two_peer_listener(TransportConfig::default()).await;
        let held = stall_as_attacker(addr, &ports, 4, "").await;
        assert!(
            health_answers(addr, std::time::Duration::from_secs(3)).await,
            "one peer address must not hold every connection slot"
        );
        drop(held);
        assert!(
            attacker_served(addr, &ports, "", std::time::Duration::from_secs(3)).await,
            "a peer's share must free as its connections end"
        );
    }

    // F-173: a trusted reverse proxy carries many clients, so it is not held to
    // the per-peer share.
    #[tokio::test]
    async fn web_listener_exempts_trusted_proxies_from_the_per_peer_cap() {
        let (addr, ports) = two_peer_listener(TransportConfig::new([ATTACKER])).await;
        let held = stall_as_attacker(addr, &ports, 3, "").await;
        assert!(
            attacker_served(addr, &ports, "", std::time::Duration::from_secs(3)).await,
            "a trusted proxy must be able to use more than a single peer's share"
        );
        drop(held);
    }

    // F-213: behind a trusted proxy every client shares the proxy's address, so
    // one forwarded client's stalled request bodies get only a single client's
    // share of the slots: another client of the same proxy is still served, and
    // the share frees as that client's requests end.
    #[tokio::test]
    async fn one_client_behind_a_trusted_proxy_cannot_hold_every_slot() {
        use std::time::Duration;
        let (addr, ports) = two_peer_listener(TransportConfig::new([ATTACKER])).await;
        let held = stall_as_attacker(addr, &ports, 4, "X-Forwarded-For: 198.51.100.7\r\n").await;
        assert!(
            attacker_served(
                addr,
                &ports,
                "X-Forwarded-For: 198.51.100.8\r\n",
                Duration::from_secs(3)
            )
            .await,
            "one client behind the proxy must not hold every connection slot"
        );
        drop(held);
        assert!(
            attacker_served(
                addr,
                &ports,
                "X-Forwarded-For: 198.51.100.7\r\n",
                Duration::from_secs(3)
            )
            .await,
            "a forwarded client's share must free as its requests end"
        );
    }

    // F-241: a client byte that is not UTF-8 in `X-Forwarded-For` does not
    // take its requests out of its forwarded share: the proxy-appended address
    // still keys them, so another client of the proxy is still served.
    #[tokio::test]
    async fn a_non_utf8_forwarded_for_byte_does_not_escape_the_clients_share() {
        use tokio::io::AsyncWriteExt;
        let (addr, ports) = two_peer_listener(TransportConfig::new([ATTACKER])).await;
        let mut held = Vec::new();
        for _ in 0..4 {
            let mut peer = connect_as_attacker(addr, &ports).await;
            let request = b"POST /body HTTP/1.1\r\nHost: x\r\nX-Forwarded-For: \xff, 198.51.100.7\r\nContent-Length: 100\r\n\r\n{";
            // A refused connection may already be closed; that is the point.
            let _ = peer.write_all(request).await;
            held.push(peer);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            attacker_served(
                addr,
                &ports,
                "X-Forwarded-For: 198.51.100.8\r\n",
                std::time::Duration::from_secs(3)
            )
            .await,
            "a non-UTF-8 byte must not let one forwarded client hold every slot"
        );
        drop(held);
    }

    // F-242: a forwarded client over its share that opens a run stream is told
    // in-band as SSE, the way run_api delivers every refusal, since an
    // EventSource cannot read a non-200 body; other paths keep the 429.
    #[tokio::test]
    async fn a_forwarded_client_over_its_share_is_refused_a_run_stream_in_band() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::time::{timeout, Duration};
        let (addr, ports) = two_peer_listener(TransportConfig::new([ATTACKER])).await;
        let client = "X-Forwarded-For: 198.51.100.7\r\n";
        let held = stall_as_attacker(addr, &ports, 2, client).await;
        let mut replies = Vec::new();
        for path in ["/api/run/stream?method=ping&target=192.0.2.1", "/health"] {
            let mut peer = connect_as_attacker(addr, &ports).await;
            let request =
                format!("GET {path} HTTP/1.1\r\nHost: x\r\n{client}Connection: close\r\n\r\n");
            peer.write_all(request.as_bytes()).await.unwrap();
            let mut reply = Vec::new();
            timeout(Duration::from_secs(5), peer.read_to_end(&mut reply))
                .await
                .expect("the refusal ends")
                .unwrap();
            replies.push(String::from_utf8_lossy(&reply).to_ascii_lowercase());
        }
        drop(held);
        let (stream, health) = (&replies[0], &replies[1]);
        assert!(stream.starts_with("http/1.1 200"), "{stream}");
        assert!(
            stream.contains("content-type: text/event-stream"),
            "{stream}"
        );
        assert!(
            stream.contains("event: run-error\ndata: too many requests"),
            "{stream}"
        );
        assert!(health.starts_with("http/1.1 429"), "{health}");
    }

    // F-213: a body trickled just fast enough to dodge the idle timeout still
    // ends at the absolute body deadline, so it cannot hold its slot.
    #[tokio::test]
    async fn web_listener_ends_trickled_request_bodies_at_the_body_deadline() {
        use tokio::io::AsyncWriteExt;
        use tokio::time::{sleep, Duration};
        let addr = idle_listener(2).await;
        for _ in 0..2 {
            let mut peer = tokio::net::TcpStream::connect(addr).await.unwrap();
            tokio::spawn(async move {
                peer.write_all(b"POST /body HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\n")
                    .await
                    .unwrap();
                // One byte per 100 ms: 10 s for the whole body, never idle for 300 ms.
                for _ in 0..100 {
                    sleep(Duration::from_millis(100)).await;
                    if peer.write_all(b"x").await.is_err() {
                        break;
                    }
                }
            });
        }
        sleep(Duration::from_millis(100)).await;
        assert!(
            health_answers(addr, Duration::from_secs(5)).await,
            "trickled request bodies must not hold every connection slot"
        );
    }

    // F-213: a forwarded client's request counts against its share until its
    // response body has been sent, so downloads it reads slowly (never stalled
    // long enough for the write deadline) take only its share of the slots;
    // a download whose connection drops frees its place.
    #[tokio::test]
    async fn slow_read_downloads_hold_only_one_forwarded_clients_share() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::time::{sleep, Duration};
        let (addr, ports) = two_peer_listener(TransportConfig::new([ATTACKER])).await;
        let mut readers = Vec::new();
        for _ in 0..4 {
            let mut peer = connect_as_attacker(addr, &ports).await;
            peer.write_all(
                b"GET /big HTTP/1.1\r\nHost: x\r\nX-Forwarded-For: 198.51.100.7\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
            // 1 KiB per 100 ms: always progressing, done in about an hour.
            readers.push(tokio::spawn(async move {
                let mut chunk = [0u8; 1024];
                while peer.read(&mut chunk).await.is_ok_and(|read| read > 0) {
                    sleep(Duration::from_millis(100)).await;
                }
            }));
        }
        sleep(Duration::from_millis(100)).await;
        assert!(
            attacker_served(
                addr,
                &ports,
                "X-Forwarded-For: 198.51.100.8\r\n",
                Duration::from_secs(3)
            )
            .await,
            "one client's slow-read downloads must not hold every connection slot"
        );
        for reader in &readers {
            reader.abort();
        }
        assert!(
            attacker_served(
                addr,
                &ports,
                "X-Forwarded-For: 198.51.100.7\r\n",
                Duration::from_secs(3)
            )
            .await,
            "a dropped download must return its client's share"
        );
    }

    /// Read one response off a keep-alive connection: its head, then its body
    /// by Content-Length, or chunked up to the last chunk, or none after HEAD.
    async fn read_response(peer: &mut tokio::net::TcpStream, head: bool) -> (String, Vec<u8>) {
        use tokio::io::AsyncReadExt;
        let mut raw = Vec::new();
        while !raw.ends_with(b"\r\n\r\n") {
            raw.push(peer.read_u8().await.unwrap());
        }
        let text = String::from_utf8(raw).unwrap();
        let length = text.lines().find_map(|line| {
            let line = line.to_ascii_lowercase();
            line.strip_prefix("content-length: ")
                .map(|value| value.parse::<usize>().unwrap())
        });
        let mut body = Vec::new();
        match length {
            _ if head => {}
            Some(length) => {
                body.resize(length, 0);
                peer.read_exact(&mut body).await.unwrap();
            }
            None => {
                while !body.ends_with(b"0\r\n\r\n") {
                    body.push(peer.read_u8().await.unwrap());
                }
            }
        }
        (text, body)
    }

    // F-213: counting to the end of the body holds a share no longer than
    // that: one forwarded client runs more than its share of downloads, event
    // streams and HEAD requests in turn on one keep-alive connection, and each
    // is answered in full, with its length.
    #[tokio::test]
    async fn finished_responses_return_a_forwarded_clients_share() {
        use tokio::io::AsyncWriteExt;
        use tokio::time::{timeout, Duration};
        let (addr, ports) = two_peer_listener(TransportConfig::new([ATTACKER])).await;
        let mut peer = connect_as_attacker(addr, &ports).await;
        let exchanges = async {
            for _ in 0..3 {
                for (method, path) in [("HEAD", "/big"), ("GET", "/big"), ("GET", "/sse")] {
                    let request = format!(
                        "{method} {path} HTTP/1.1\r\nHost: x\r\nX-Forwarded-For: 198.51.100.7\r\n\r\n"
                    );
                    peer.write_all(request.as_bytes()).await.unwrap();
                    let (head, body) = read_response(&mut peer, method == "HEAD").await;
                    assert!(head.starts_with("HTTP/1.1 200"), "{method} {path}: {head}");
                    if path == "/big" {
                        let length = format!("content-length: {BIG_RESPONSE}\r\n");
                        assert!(head.to_ascii_lowercase().contains(&length), "{head}");
                    } else {
                        let body = String::from_utf8_lossy(&body);
                        assert!(body.contains("data: second"), "{body}");
                    }
                    if method == "GET" && path == "/big" {
                        assert_eq!(body.len(), BIG_RESPONSE);
                    }
                }
            }
        };
        timeout(Duration::from_secs(30), exchanges)
            .await
            .expect("each response ends and returns its share");
    }

    // Every response carries the browser hardening headers, and the CSP
    // admits exactly the inline scripts of the embedded build by hash.
    #[tokio::test]
    async fn responses_carry_hardening_headers_with_the_builds_script_hashes() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let mut inline_scripts = Vec::new();
        for page in ["index.html", "200.html"] {
            let html = String::from_utf8(Spa::get(page).unwrap().data.into_owned()).unwrap();
            for tail in html.split("<script>").skip(1) {
                inline_scripts.push(tail.split("</script>").next().unwrap().to_string());
            }
        }
        assert!(inline_scripts.len() >= 2, "the build has inline scripts");

        let app = with_routes(Router::new());
        for uri in ["/", "/admin", "/health"] {
            let response = app
                .clone()
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string()
            };
            assert_eq!(header("x-content-type-options"), "nosniff", "{uri}");
            assert_eq!(
                header("referrer-policy"),
                "strict-origin-when-cross-origin",
                "{uri}"
            );
            let csp = header("content-security-policy");
            assert!(csp.contains("frame-ancestors 'none'"), "{uri}: {csp}");
            assert!(csp.contains("script-src 'self'"), "{uri}: {csp}");
            for script in &inline_scripts {
                let hash = format!("'sha256-{}'", independent_sha256_base64(script));
                assert!(csp.contains(&hash), "{uri}: {hash} missing from {csp}");
            }
        }
    }

    // F-230: an unknown /api path answers with the uniform JSON error the SPA
    // parses, while client routes keep the 200 shell and a known route's
    // wrong method keeps axum's 405.
    #[tokio::test]
    async fn unknown_api_paths_get_the_json_error_and_spa_routes_the_shell() {
        use axum::body::{to_bytes, Body};
        use axum::http::Request;
        use tower::ServiceExt;

        let app = with_routes(Router::new().route("/api/thing", get(|| async { "thing" })));
        let call = |method: &str, uri: &str| {
            app.clone().oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
        };
        for uri in ["/api", "/api/unknown", "/api/admin/nope/deeper"] {
            let response = call("GET", uri).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/json",
                "{uri}"
            );
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["error"], "not_found", "{uri}: {body}");
            assert!(body["message"].is_string(), "{uri}: {body}");
        }

        let shell = call("GET", "/some/spa/route").await.unwrap();
        assert_eq!(shell.status(), StatusCode::OK);
        assert!(shell.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html"));

        let wrong_method = call("POST", "/api/thing").await.unwrap();
        assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    /// Standard padded base64 of SHA-256, spelled out bit by bit so it does not
    /// share code with the implementation it checks.
    fn independent_sha256_base64(text: &str) -> String {
        let hex = shared::protocol::sha256_hex(text.as_bytes());
        let bits: String = (0..hex.len())
            .step_by(2)
            .map(|i| format!("{:08b}", u8::from_str_radix(&hex[i..i + 2], 16).unwrap()))
            .collect();
        let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out: String = bits
            .as_bytes()
            .chunks(6)
            .map(|six| {
                let six = format!("{:0<6}", std::str::from_utf8(six).unwrap());
                alphabet.as_bytes()[usize::from_str_radix(&six, 2).unwrap()] as char
            })
            .collect();
        while !out.len().is_multiple_of(4) {
            out.push('=');
        }
        out
    }

    fn temp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("lg-setup-token-{label}-{}-{n}", std::process::id()));
        path
    }

    fn contains_field(logs: &str, name: &str, value: &str) -> bool {
        logs.contains(&format!("{name}={value}")) || logs.contains(&format!("{name}=\"{value}\""))
    }

    fn captured_logs() -> (StdArc<Mutex<Vec<u8>>>, tracing::dispatcher::DefaultGuard) {
        let buffer = StdArc::new(Mutex::new(Vec::new()));
        let writer_buffer = StdArc::clone(&buffer);
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || CaptureWriter(StdArc::clone(&writer_buffer)))
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        (buffer, guard)
    }

    struct CaptureWriter(StdArc<Mutex<Vec<u8>>>);

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
