//! Agent-side enrollment: parse the pinned install command, exchange the token for
//! a credential over a transport that presents central's identity, and — the crux
//! (AC35 / G4) — verify that presented identity against the fingerprint pinned from
//! the install command *before* trusting anything. A mismatch aborts with no
//! credential stored: no trust-on-first-use, fail closed.
//!
//! The production install exchange opens TLS to central's HTTPS API origin and
//! posts to `/api/enroll`; here [`CentralConnector`] keeps the pin logic provable
//! in isolation. The presented identity is the TLS peer certificate, verified
//! before any credential is trusted.

use std::fmt;
use std::io;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use serde::{Deserialize, Serialize};
use shared::protocol::{
    constant_time_eq, fingerprint, verify_pinned_identity, EnrollRequest, EnrollResponse,
    ENV_CENTRAL_FINGERPRINT, ENV_CENTRAL_URL, ENV_ENROLL_TOKEN, ENV_TUNNEL_URL, PROTOCOL_VERSION,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// The three values the install command bakes in, recovered from the agent's
/// environment: where central is, the fingerprint to pin it by, and the token.
#[derive(Debug, Clone)]
pub struct PinnedCommand {
    pub central_url: String,
    pub tunnel_url: String,
    pub fingerprint: String,
    pub token: String,
}

impl PinnedCommand {
    /// Reconstruct the pinned command from the environment the install command set.
    /// Every value is required — a missing one is a misconfigured install and fails
    /// closed rather than enrolling against an unpinned central.
    pub fn from_env() -> Result<Self, EnrollError> {
        Ok(Self {
            central_url: required_env(ENV_CENTRAL_URL)?,
            tunnel_url: required_env(ENV_TUNNEL_URL)?,
            fingerprint: required_env(ENV_CENTRAL_FINGERPRINT)?,
            token: required_env(ENV_ENROLL_TOKEN)?,
        })
    }
}

fn required_env(key: &'static str) -> Result<String, EnrollError> {
    match std::env::var(key) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => Err(EnrollError::MissingParam(key)),
    }
}

/// What central presents during the enrollment exchange: its TLS peer certificate
/// identity material and its enroll response.
pub struct PresentedEnrollment {
    pub identity_material: Vec<u8>,
    pub response: EnrollResponse,
}

/// The transport seam. Production uses HTTPS `/api/enroll`; tests inject one that
/// presents a chosen identity + response.
pub trait CentralConnector {
    fn exchange(
        &self,
        request: EnrollRequest,
    ) -> impl std::future::Future<Output = Result<PresentedEnrollment, EnrollError>>;
}

#[derive(Debug, Clone)]
struct EnrollEndpoint {
    host: String,
    port: u16,
    path: String,
    http_authority: String,
}

impl EnrollEndpoint {
    fn parse(central_url: &str) -> Result<Self, EnrollError> {
        let without_scheme = central_url
            .strip_prefix("https://")
            .ok_or(EnrollError::InvalidCredential("central URL must use https"))?;
        let authority = without_scheme
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default();
        if authority.is_empty() {
            return Err(EnrollError::InvalidCredential("central URL host is empty"));
        }
        let rest = &without_scheme[authority.len()..];
        if !rest.is_empty() {
            return Err(EnrollError::InvalidCredential(
                "central URL must be an https origin",
            ));
        }
        let path = "/api/enroll".to_string();

        let (host, port) = parse_authority(authority)?;
        if host.is_empty() {
            return Err(EnrollError::InvalidCredential("central URL host is empty"));
        }
        ServerName::try_from(host.clone())
            .map_err(|_| EnrollError::InvalidCredential("central URL host is invalid"))?;
        Ok(Self {
            host,
            port,
            path,
            http_authority: authority.to_string(),
        })
    }
}

fn parse_authority(authority: &str) -> Result<(String, u16), EnrollError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, suffix)) = rest.split_once(']') else {
            return Err(EnrollError::InvalidCredential(
                "central URL host is invalid",
            ));
        };
        if !matches!(host.parse::<IpAddr>(), Ok(IpAddr::V6(_))) {
            return Err(EnrollError::InvalidCredential(
                "central URL host is invalid",
            ));
        }
        let port = match suffix.strip_prefix(':') {
            Some(port) => parse_port(port)?,
            None if suffix.is_empty() => 443,
            None => {
                return Err(EnrollError::InvalidCredential(
                    "central URL host is invalid",
                ));
            }
        };
        return Ok((host.to_string(), port));
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && !host.contains(':') => {
            (host.to_string(), parse_port(port)?)
        }
        Some((_, _)) => {
            return Err(EnrollError::InvalidCredential(
                "central URL host is invalid",
            ));
        }
        None if authority.contains(':') => {
            return Err(EnrollError::InvalidCredential(
                "central URL host is invalid",
            ));
        }
        None => (authority.to_string(), 443),
    };
    Ok((host, port))
}

fn parse_port(port: &str) -> Result<u16, EnrollError> {
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err(EnrollError::InvalidCredential(
            "central URL port is invalid",
        ));
    }
    let port = port
        .parse::<u16>()
        .map_err(|_| EnrollError::InvalidCredential("central URL port is invalid"))?;
    if port == 0 {
        return Err(EnrollError::InvalidCredential(
            "central URL port is invalid",
        ));
    }
    Ok(port)
}

/// Production install-time enrollment connector. It opens TLS to central, pins the
/// presented certificate against `LG_CENTRAL_FP`, then posts the enrollment request.
/// The enrollment token is written only to the TLS stream body, never to shell argv.
pub struct HttpsEnrollConnector {
    endpoint: EnrollEndpoint,
    pinned_fingerprint: String,
}

impl HttpsEnrollConnector {
    pub fn new(central_url: &str, pinned_fingerprint: &str) -> Result<Self, EnrollError> {
        Ok(Self {
            endpoint: EnrollEndpoint::parse(central_url)?,
            pinned_fingerprint: pinned_fingerprint.to_string(),
        })
    }
}

impl CentralConnector for HttpsEnrollConnector {
    async fn exchange(&self, request: EnrollRequest) -> Result<PresentedEnrollment, EnrollError> {
        let captured_identity = Arc::new(Mutex::new(None));
        let verifier = Arc::new(PinnedEnrollCentral {
            pinned_fingerprint: self.pinned_fingerprint.clone(),
            captured_identity: Arc::clone(&captured_identity),
            algorithms: rustls::crypto::ring::default_provider().signature_verification_algorithms,
        });
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| EnrollError::Transport(format!("{error:?}")))?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();

        let tcp = TcpStream::connect((self.endpoint.host.as_str(), self.endpoint.port))
            .await
            .map_err(|error| EnrollError::Transport(error.to_string()))?;
        let server_name = ServerName::try_from(self.endpoint.host.clone())
            .map_err(|_| EnrollError::InvalidCredential("central URL host is invalid"))?;
        let mut tls = TlsConnector::from(Arc::new(config))
            .connect(server_name, tcp)
            .await
            .map_err(|error| EnrollError::Transport(error.to_string()))?;

        let body = serde_json::to_vec(&request).map_err(|error| {
            EnrollError::Transport(format!("could not encode enrollment request: {error}"))
        })?;
        let http = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.endpoint.path,
            self.endpoint.http_authority,
            body.len()
        );
        tls.write_all(http.as_bytes())
            .await
            .map_err(|error| EnrollError::Transport(error.to_string()))?;
        tls.write_all(&body)
            .await
            .map_err(|error| EnrollError::Transport(error.to_string()))?;

        let mut bytes = Vec::new();
        tls.read_to_end(&mut bytes)
            .await
            .map_err(|error| EnrollError::Transport(error.to_string()))?;
        let (_, body) = split_http_response(&bytes)?;
        let response = serde_json::from_slice::<EnrollResponse>(body).map_err(|error| {
            EnrollError::Transport(format!("invalid enrollment response: {error}"))
        })?;
        let identity_material = captured_identity
            .lock()
            .map_err(|_| EnrollError::Transport("central identity capture failed".to_string()))?
            .clone()
            .ok_or_else(|| {
                EnrollError::Transport("central did not present identity".to_string())
            })?;
        Ok(PresentedEnrollment {
            identity_material,
            response,
        })
    }
}

fn split_http_response(bytes: &[u8]) -> Result<(&[u8], &[u8]), EnrollError> {
    let Some(header_end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Err(EnrollError::Transport(
            "enrollment response was not HTTP".to_string(),
        ));
    };
    let headers = &bytes[..header_end];
    let body = &bytes[header_end + 4..];
    if !headers.starts_with(b"HTTP/1.1 200 ") && !headers.starts_with(b"HTTP/1.0 200 ") {
        return Err(EnrollError::Transport(
            "central refused enrollment".to_string(),
        ));
    }
    Ok((headers, body))
}

#[derive(Debug)]
struct PinnedEnrollCentral {
    pinned_fingerprint: String,
    captured_identity: Arc<Mutex<Option<Vec<u8>>>>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedEnrollCentral {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        verify_pinned_identity(end_entity.as_ref(), &self.pinned_fingerprint)
            .map(|()| {
                if let Ok(mut captured) = self.captured_identity.lock() {
                    *captured = Some(end_entity.as_ref().to_vec());
                }
                ServerCertVerified::assertion()
            })
            .map_err(|_| {
                rustls::Error::General(
                    "central identity does not match the pinned fingerprint".to_string(),
                )
            })
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// The credential an agent keeps after a verified enrollment. Persisted to the node
/// so the tunnel (Slice 8) can authenticate with it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCredential {
    pub agent_id: String,
    pub credential: String,
    pub central_url: String,
    pub tunnel_url: String,
    pub fingerprint: String,
}

/// Enroll against central and return the credential — only if central's presented
/// identity matches the pinned fingerprint. The verification happens before the
/// credential is trusted, so a central the agent cannot verify yields an error and
/// nothing is returned to store (AC35, fail closed).
pub async fn enroll<C: CentralConnector>(
    command: &PinnedCommand,
    connector: &C,
) -> Result<AgentCredential, EnrollError> {
    let presented = connector
        .exchange(EnrollRequest {
            protocol_version: PROTOCOL_VERSION,
            token: command.token.clone(),
        })
        .await?;

    // Pin check (no TOFU): central's presented identity must hash to the fingerprint
    // carried in the install command. Constant-time so a partial match is not timed.
    let presented_fingerprint = fingerprint(&presented.identity_material);
    if !constant_time_eq(
        presented_fingerprint.as_bytes(),
        command.fingerprint.as_bytes(),
    ) {
        return Err(EnrollError::IdentityMismatch);
    }

    if presented.response.protocol_version != PROTOCOL_VERSION {
        return Err(EnrollError::ProtocolMismatch {
            got: presented.response.protocol_version,
        });
    }

    Ok(AgentCredential {
        agent_id: presented.response.agent_id,
        credential: presented.response.credential,
        central_url: command.central_url.clone(),
        tunnel_url: command.tunnel_url.clone(),
        fingerprint: command.fingerprint.clone(),
    })
}

pub fn credential_from_enroll_response(
    central_url: &str,
    tunnel_url: &str,
    pinned_fingerprint: &str,
    response: EnrollResponse,
) -> Result<AgentCredential, EnrollError> {
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(EnrollError::ProtocolMismatch {
            got: response.protocol_version,
        });
    }
    if response.agent_id.is_empty() {
        return Err(EnrollError::InvalidResponse("missing agent id"));
    }
    if response.credential.is_empty() {
        return Err(EnrollError::InvalidResponse("missing credential"));
    }
    Ok(AgentCredential {
        agent_id: response.agent_id,
        credential: response.credential,
        central_url: central_url.to_string(),
        tunnel_url: tunnel_url.to_string(),
        fingerprint: pinned_fingerprint.to_string(),
    })
}

pub async fn store_install_credential<C: CentralConnector>(
    path: &Path,
    command: &PinnedCommand,
    connector: &C,
) -> Result<(), EnrollError> {
    let credential = enroll(command, connector).await?;
    store_credential(path, &credential).map_err(|error| EnrollError::Store(error.to_string()))
}

pub fn store_dry_run_response_credential(
    response_path: &Path,
    credential_path: &Path,
    central_url: &str,
    tunnel_url: &str,
    pinned_fingerprint: &str,
    dry_run: bool,
) -> Result<(), EnrollError> {
    if !dry_run {
        return Err(EnrollError::ResponseFileOutsideDryRun);
    }
    let bytes =
        std::fs::read(response_path).map_err(|error| EnrollError::Store(error.to_string()))?;
    let response = serde_json::from_slice::<EnrollResponse>(&bytes)
        .map_err(|_| EnrollError::InvalidResponse("malformed response file"))?;
    let credential =
        credential_from_enroll_response(central_url, tunnel_url, pinned_fingerprint, response)?;
    store_credential(credential_path, &credential)
        .map_err(|error| EnrollError::Store(error.to_string()))
}

pub fn validate_stored_credential(credential: &AgentCredential) -> Result<(), EnrollError> {
    if credential.agent_id.is_empty() {
        return Err(EnrollError::InvalidCredential("missing agent id"));
    }
    if credential.credential.is_empty() {
        return Err(EnrollError::InvalidCredential("missing credential"));
    }
    if credential.fingerprint.len() != 64
        || !credential
            .fingerprint
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(EnrollError::InvalidCredential(
            "central fingerprint must be 64 hex characters",
        ));
    }
    EnrollEndpoint::parse(&credential.central_url)?;
    EnrollEndpoint::parse(&credential.tunnel_url)?;
    Ok(())
}

/// Persist the credential to the node, owner-read/write only — it authenticates
/// every later tunnel frame, so it must not be world-readable.
pub fn store_credential(path: &Path, credential: &AgentCredential) -> io::Result<()> {
    let encoded = serde_json::to_vec_pretty(credential)?;
    write_owner_only(path, &encoded)
}

#[cfg(unix)]
fn write_owner_only(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("agent-credential.json");
    let tmp_path = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp_path)?;
    if let Err(error) = io::Write::write_all(&mut file, bytes) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error);
    }
    if let Err(error) = file.sync_all() {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error);
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

#[derive(Debug)]
pub enum EnrollError {
    MissingParam(&'static str),
    IdentityMismatch,
    ProtocolMismatch { got: u16 },
    InvalidResponse(&'static str),
    InvalidCredential(&'static str),
    ResponseFileOutsideDryRun,
    Store(String),
    Transport(String),
}

impl fmt::Display for EnrollError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnrollError::MissingParam(key) => {
                write!(f, "missing enrollment parameter {key}")
            }
            EnrollError::IdentityMismatch => f.write_str(
                "central's identity does not match the pinned fingerprint — enrollment aborted",
            ),
            EnrollError::ProtocolMismatch { got } => {
                write!(
                    f,
                    "central speaks protocol version {got}, expected {PROTOCOL_VERSION}"
                )
            }
            EnrollError::InvalidResponse(msg) => write!(f, "invalid enrollment response: {msg}"),
            EnrollError::InvalidCredential(msg) => write!(f, "invalid agent credential: {msg}"),
            EnrollError::ResponseFileOutsideDryRun => {
                f.write_str("LG_ENROLL_RESPONSE_FILE is allowed only with LG_INSTALL_DRY_RUN=1")
            }
            EnrollError::Store(msg) => write!(f, "credential storage error: {msg}"),
            EnrollError::Transport(msg) => write!(f, "enrollment transport error: {msg}"),
        }
    }
}

impl std::error::Error for EnrollError {}
