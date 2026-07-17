use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    http::{header, StatusCode, Uri},
    middleware::from_fn_with_state,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use rust_embed::{EmbeddedFile, RustEmbed};
use tower::ServiceBuilder;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod admin_api;
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
pub use store::{Agent, EnrollmentToken, Store, StoreError};
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
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let store = Store::open(&path).expect("open redb store");
    let setup_token = if store.is_installed().unwrap_or(false) {
        None
    } else {
        let (token, _source) =
            setup_token_for_startup(&path).expect("prepare first-run setup token");
        Some(Arc::from(token.as_str()))
    };
    let settings = store.settings().unwrap_or_default();
    let files_dir = std::env::var("LG_FILES_DIR").unwrap_or_else(|_| DEFAULT_FILES_DIR.to_string());
    let _ = std::fs::create_dir_all(&files_dir);

    // Enrollment pins the HTTPS API origin that serves `/api/enroll`. The tunnel
    // listener is a separate TLS/WebSocket socket and must not become LG_CENTRAL_URL.
    let tunnel_identity = tunnel::TunnelIdentity::from_env();
    let enroll = EnrollConfig::from_env();

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
    let admin = api_routes(state.clone()).layer(session_layer);
    let run = run_api::routes(state.clone());
    let files = files::routes(state.clone());
    let public = admin_api::public_routes(state.clone());
    let api = admin
        .merge(run)
        .merge(files)
        .merge(public)
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
        .merge(enroll::routes())
        .with_state(state)
}

pub fn with_routes(features: Router) -> Router {
    features
        .route("/health", get(health))
        .fallback(serve_spa)
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
                .layer(TraceLayer::new_for_http())
                .layer(PropagateRequestIdLayer::x_request_id()),
        )
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
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    let requested = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };

    if let Some(file) = Spa::get(requested) {
        return embedded_response(requested, file);
    }
    match Spa::get("index.html") {
        Some(file) => embedded_response("index.html", file),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn embedded_response(path: &str, file: EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    (
        [(header::CONTENT_TYPE, mime.as_ref())],
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
