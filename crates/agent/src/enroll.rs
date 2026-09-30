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
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::ClientConfig;
use serde::{Deserialize, Serialize};
use shared::protocol::{
    verify_pinned_identity, EnrollRequest, EnrollResponse, ENV_CENTRAL_FINGERPRINT,
    ENV_CENTRAL_URL, ENV_ENROLL_TOKEN, ENV_TUNNEL_URL, PROTOCOL_VERSION,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::tunnel::PinnedCentral;

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

/// The tunnel URL's host and port: the one parse both the startup check
/// ([`validate_stored_credential`]) and the dialer
/// ([`crate::tunnel::TunnelClientConfig::from_parts`]) use, so they cannot
/// disagree. Like central it takes only an explicit `https://host:port`;
/// a bracketed IPv6 literal or a missing port is refused, never guessed.
pub(crate) fn tunnel_host_port(tunnel_url: &str) -> Result<(String, u16), EnrollError> {
    let endpoint = EnrollEndpoint::parse(tunnel_url)?;
    if endpoint.http_authority.starts_with('[') || !endpoint.http_authority.contains(':') {
        return Err(EnrollError::InvalidCredential(
            "tunnel URL must be https://host:port with an explicit port and no bracketed IPv6 literal",
        ));
    }
    Ok((endpoint.host, endpoint.port))
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

/// Deadline for the whole install-time exchange, connect to response: a central
/// or front that accepts TCP and never answers must fail the install, not hang it.
const ENROLL_TIMEOUT: Duration = Duration::from_secs(30);

/// Production install-time enrollment connector. It opens TLS to central, pins the
/// presented certificate against `LG_CENTRAL_FP`, then posts the enrollment request.
/// The enrollment token is written only to the TLS stream body, never to shell argv.
pub struct HttpsEnrollConnector {
    endpoint: EnrollEndpoint,
    pinned_fingerprint: String,
    timeout: Duration,
}

impl HttpsEnrollConnector {
    pub fn new(central_url: &str, pinned_fingerprint: &str) -> Result<Self, EnrollError> {
        Ok(Self {
            endpoint: EnrollEndpoint::parse(central_url)?,
            pinned_fingerprint: pinned_fingerprint.to_string(),
            timeout: ENROLL_TIMEOUT,
        })
    }
}

impl CentralConnector for HttpsEnrollConnector {
    async fn exchange(&self, request: EnrollRequest) -> Result<PresentedEnrollment, EnrollError> {
        tokio::time::timeout(self.timeout, self.exchange_unbounded(request))
            .await
            .unwrap_or_else(|_| {
                Err(EnrollError::Transport(format!(
                    "central did not answer within {:?}",
                    self.timeout
                )))
            })
    }
}

impl HttpsEnrollConnector {
    /// Connect, pin, post and read the answer; [`Self::exchange`] bounds it.
    async fn exchange_unbounded(
        &self,
        request: EnrollRequest,
    ) -> Result<PresentedEnrollment, EnrollError> {
        let captured_identity = Arc::new(Mutex::new(None));
        let verifier = Arc::new(PinnedCentral {
            pinned_fingerprint: self.pinned_fingerprint.clone(),
            captured_identity: Some(Arc::clone(&captured_identity)),
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

        let body = read_http_response(&mut tls).await?;
        let response = serde_json::from_slice::<EnrollResponse>(&body).map_err(|error| {
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

/// Read central's response and return its body. With a Content-Length, exactly
/// that many body bytes are read and nothing after, so a front that closes without
/// a TLS close_notify still completes: central has already consumed the token. A
/// chunked body likewise ends at its last chunk.
async fn read_http_response<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, EnrollError> {
    let transport = |error: io::Error| EnrollError::Transport(error.to_string());
    let mut head = Vec::new();
    let header_end = loop {
        if let Some(end) = head.windows(4).position(|w| w == b"\r\n\r\n") {
            // An interim 1xx head (100 Continue, 103 Early Hints; RFC 9110 15.2)
            // precedes the final response: skip it. 101 would switch protocols,
            // so it stays final (a refusal).
            let interim = head.starts_with(b"HTTP/1.")
                && head.get(9) == Some(&b'1')
                && head.get(9..12) != Some(b"101");
            if interim {
                head.drain(..end + 4);
                continue;
            }
            break end + 4;
        }
        let mut chunk = [0u8; 1024];
        let read = reader.read(&mut chunk).await.map_err(transport)?;
        if read == 0 {
            return Err(EnrollError::Transport(
                "enrollment response was not HTTP".to_string(),
            ));
        }
        head.extend_from_slice(&chunk[..read]);
    };
    let mut body = head.split_off(header_end);
    let body = async {
        if header(&head, "transfer-encoding")
            .is_some_and(|value| value.trim().to_ascii_lowercase().ends_with("chunked"))
        {
            // ponytail: re-decodes from the start per read, quadratic in body size;
            // fine for a sub-KB enrollment answer, go incremental if it ever grows.
            loop {
                if let Some(decoded) = decode_chunked(&body)? {
                    return Ok(decoded);
                }
                let mut chunk = [0u8; 1024];
                let read = reader.read(&mut chunk).await.map_err(transport)?;
                if read == 0 {
                    return Err(malformed_chunked());
                }
                body.extend_from_slice(&chunk[..read]);
            }
        }
        match header(&head, "content-length").and_then(|value| value.trim().parse::<usize>().ok()) {
            Some(length) => {
                let missing = length.saturating_sub(body.len()) as u64;
                reader
                    .take(missing)
                    .read_to_end(&mut body)
                    .await
                    .map_err(transport)?;
                body.truncate(length);
            }
            None => {
                reader.read_to_end(&mut body).await.map_err(transport)?;
            }
        }
        Ok(body)
    }
    .await;
    if head.starts_with(b"HTTP/1.1 200 ") || head.starts_with(b"HTTP/1.0 200 ") {
        return body;
    }
    // Any other status is a refusal. Central's messages for a bad token or a
    // cleartext hop are written for a browser, so name the operator's fix for
    // those codes; central's own message is shown only for codes meant for
    // operators (its misconfiguration, a bad request, rate limiting).
    let error = body
        .ok()
        .and_then(|body| serde_json::from_slice::<serde_json::Value>(&body).ok());
    let message = error.as_ref().and_then(|error| {
        Some(match error.get("error")?.as_str()? {
            "unauthorized" => "the install token is invalid, expired or already used; \
                generate a new install command"
                .to_string(),
            "insecure_transport" => "central did not see HTTPS; the proxy in front of \
                central must send X-Forwarded-Proto: https and be listed in LG_TRUSTED_PROXIES"
                .to_string(),
            "identity_mismatch" | "tunnel_unavailable" | "invalid_input" | "rate_limited" => error
                .get("message")?
                .as_str()?
                .chars()
                .filter(|c| !c.is_control())
                .take(512)
                .collect(),
            _ => return None,
        })
    });
    Err(EnrollError::Transport(match message {
        Some(message) => format!("central refused enrollment: {message}"),
        None => "central refused enrollment".to_string(),
    }))
}

/// The value of the `wanted` header. Lines are scanned as bytes, so a non-UTF-8
/// (obs-text) byte in another header cannot hide this one.
fn header<'a>(head: &'a [u8], wanted: &str) -> Option<&'a str> {
    head.split(|&byte| byte == b'\n').find_map(|line| {
        let colon = line.iter().position(|&byte| byte == b':')?;
        if !line[..colon].eq_ignore_ascii_case(wanted.as_bytes()) {
            return None;
        }
        std::str::from_utf8(&line[colon + 1..]).ok()
    })
}

fn malformed_chunked() -> EnrollError {
    EnrollError::Transport("malformed chunked enrollment response".to_string())
}

/// Decode a chunked body (RFC 9112 7.1): `Ok(None)` until the last chunk and
/// the trailer section have arrived, an error for malformed framing.
fn decode_chunked(mut data: &[u8]) -> Result<Option<Vec<u8>>, EnrollError> {
    let mut body = Vec::new();
    loop {
        let Some(line_end) = data.windows(2).position(|w| w == b"\r\n") else {
            return Ok(None);
        };
        // The size is hex digits only; a chunk extension after ';' is ignored.
        let size = std::str::from_utf8(&data[..line_end])
            .ok()
            .and_then(|line| line.split(';').next())
            .map(str::trim)
            .filter(|hex| !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .and_then(|hex| usize::from_str_radix(hex, 16).ok())
            .ok_or_else(malformed_chunked)?;
        data = &data[line_end + 2..];
        if size == 0 {
            // Trailer field lines, then an empty line.
            let done = data.starts_with(b"\r\n") || data.windows(4).any(|w| w == b"\r\n\r\n");
            return Ok(done.then_some(body));
        }
        let end = size.checked_add(2).ok_or_else(malformed_chunked)?;
        if data.len() < end {
            return Ok(None);
        }
        if &data[size..end] != b"\r\n" {
            return Err(malformed_chunked());
        }
        body.extend_from_slice(&data[..size]);
        data = &data[end..];
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

    // Pin check (no TOFU): central's presented identity must match the pin carried
    // in the install command. Constant-time so a partial match is not timed.
    verify_pinned_identity(&presented.identity_material, &command.fingerprint)
        .map_err(|_| EnrollError::IdentityMismatch)?;

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
    tunnel_host_port(&credential.tunnel_url)?;
    Ok(())
}

/// Persist the credential to the node, owner-read/write only — it authenticates
/// every later tunnel frame, so it must not be world-readable.
pub fn store_credential(path: &Path, credential: &AgentCredential) -> io::Result<()> {
    let encoded = serde_json::to_vec_pretty(credential)?;
    write_owner_only(path, &encoded, false)
}

/// Replace a legacy whole-certificate pin in the stored credential with the
/// public-key pin of the same, just verified, central. A no-op unless the
/// file still holds `legacy_pin`. The rewrite is atomic (temp file, fsync, rename),
/// owner-only and keeps the file's owner, so a crash leaves the old credential.
pub(crate) fn repin_credential(path: &Path, legacy_pin: &str, pin: &str) -> io::Result<()> {
    let mut credential: AgentCredential = serde_json::from_slice(&std::fs::read(path)?)?;
    if credential.fingerprint != legacy_pin {
        return Ok(());
    }
    credential.fingerprint = pin.to_string();
    write_owner_only(path, &serde_json::to_vec_pretty(&credential)?, true)
}

#[cfg(unix)]
fn write_owner_only(path: &Path, bytes: &[u8], keep_owner: bool) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let owner = if keep_owner {
        let metadata = std::fs::metadata(path)?;
        Some((metadata.uid(), metadata.gid()))
    } else {
        None
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("agent-credential.json");
    let tmp_path = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    // A write that crashed before its rename may have left this name behind (a
    // container agent is always pid 1); unlinking never follows a symlink.
    let _ = std::fs::remove_file(&tmp_path);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp_path)?;
    if let Err(error) = io::Write::write_all(&mut file, bytes) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error);
    }
    if let Some((uid, gid)) = owner {
        if let Err(error) = std::os::unix::fs::fchown(&file, Some(uid), Some(gid)) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(error);
        }
    }
    // Set the mode on the open file, never by path after the rename: the directory
    // is service-user writable, and a root chmod by path follows a swapped-in symlink.
    if let Err(error) = file.set_permissions(std::fs::Permissions::from_mode(0o600)) {
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
    Ok(())
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, bytes: &[u8], _keep_owner: bool) -> io::Result<()> {
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

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
    use rustls::ServerConfig;
    use shared::protocol::{fingerprint, EnrollRequest, PROTOCOL_VERSION};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    use super::{
        validate_stored_credential, AgentCredential, CentralConnector, EnrollError,
        HttpsEnrollConnector,
    };
    use crate::tunnel::TunnelClientConfig;

    // A throwaway self-signed P-256 certificate for 127.0.0.1; only its pin matters.
    pub(crate) const TEST_CERT: &str = "-----BEGIN CERTIFICATE-----
MIIBkDCCATagAwIBAgIUBeO8HnUacniszu1YUwSTZgqtcWMwCgYIKoZIzj0EAwIw
FDESMBAGA1UEAwwJMTI3LjAuMC4xMCAXDTI2MDkyNjA0MTgzM1oYDzIxMjYwOTAy
MDQxODMzWjAUMRIwEAYDVQQDDAkxMjcuMC4wLjEwWTATBgcqhkjOPQIBBggqhkjO
PQMBBwNCAASiGN4kWzPxjpPEo2Dk8189mdAPwtpxLbCvMUiNDd+zufbjHokPvyFQ
Z2/b1QOwyW+8icuE5mJsTVZe1HFl84CKo2QwYjAdBgNVHQ4EFgQUj63XqYsaA/Aq
PTQY/gfwKtnWyx0wHwYDVR0jBBgwFoAUj63XqYsaA/AqPTQY/gfwKtnWyx0wDwYD
VR0TAQH/BAUwAwEB/zAPBgNVHREECDAGhwR/AAABMAoGCCqGSM49BAMCA0gAMEUC
IAlFGp0E8owzvQx5u7wjC6QrSoTtPvR6bGom9pAAHldwAiEAm4PEPiB6HmQ8hQ+l
v1J63xZ4ADDsaxdAJjuIj96n9yM=
-----END CERTIFICATE-----";
    pub(crate) const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgBnwSybWSJPrey63M
fJpXFezEefVwvUGjBhlnURaAvkOhRANCAASiGN4kWzPxjpPEo2Dk8189mdAPwtpx
LbCvMUiNDd+zufbjHokPvyFQZ2/b1QOwyW+8icuE5mJsTVZe1HFl84CK
-----END PRIVATE KEY-----";

    /// Serve one enrollment over TLS: read the request, answer with `response`,
    /// then close the TCP connection without a TLS close_notify.
    async fn enroll_through_a_front_without_close_notify(
        response: String,
    ) -> Result<super::PresentedEnrollment, EnrollError> {
        let certs = CertificateDer::pem_slice_iter(TEST_CERT.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let pinned = fingerprint(certs[0].as_ref());
        let key = PrivateKeyDer::from_pem_slice(TEST_KEY.as_bytes()).unwrap();
        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut tls = TlsAcceptor::from(Arc::new(config))
                .accept(tcp)
                .await
                .unwrap();
            // Read the whole request (its JSON body ends in '}') so the close is a
            // clean FIN, not a reset over unread bytes.
            let mut request = Vec::new();
            while !request.ends_with(b"}") {
                let mut chunk = [0u8; 1024];
                let read = tls.read(&mut chunk).await.unwrap();
                assert_ne!(read, 0, "client closed before its request body");
                request.extend_from_slice(&chunk[..read]);
            }
            tls.write_all(response.as_bytes()).await.unwrap();
            tls.flush().await.unwrap();
            let (mut tcp, _) = tls.into_inner();
            tcp.shutdown().await.unwrap();
        });

        HttpsEnrollConnector::new(&format!("https://{addr}"), &pinned)
            .unwrap()
            .exchange(EnrollRequest {
                protocol_version: PROTOCOL_VERSION,
                token: "enrollment-token".to_string(),
            })
            .await
    }

    // Central has already consumed the token when it answers, so a complete
    // Content-Length response must be accepted even when the HTTPS front closes
    // without close_notify; otherwise every install command through it is burnt.
    #[tokio::test]
    async fn a_complete_response_without_close_notify_still_enrolls() {
        let body = format!(
            r#"{{"protocol_version":{PROTOCOL_VERSION},"agent_id":"agent-xyz","credential":"cred-abc123"}}"#
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        // Bytes past Content-Length are not part of the body.
        for response in [response.clone(), format!("{response}trailing")] {
            let presented = enroll_through_a_front_without_close_notify(response)
                .await
                .expect("a complete Content-Length body is enough; close_notify is not required");

            assert_eq!(presented.response.agent_id, "agent-xyz");
            assert_eq!(presented.response.credential, "cred-abc123");
        }
    }

    // F-357: an interim 1xx head (RFC 9110 15.2) is not central's answer; the
    // final 200 after it is, so its credential is kept.
    #[tokio::test]
    async fn an_interim_1xx_response_before_the_200_still_enrolls() {
        let body = format!(
            r#"{{"protocol_version":{PROTOCOL_VERSION},"agent_id":"agent-xyz","credential":"cred-abc123"}}"#
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        for interim in [
            "HTTP/1.1 100 Continue\r\n\r\n",
            "HTTP/1.1 103 Early Hints\r\nLink: </style.css>; rel=preload\r\n\r\n",
        ] {
            let presented =
                enroll_through_a_front_without_close_notify(format!("{interim}{response}"))
                    .await
                    .unwrap_or_else(|error| panic!("{interim:?} then 200 must enroll: {error:?}"));
            assert_eq!(presented.response.credential, "cred-abc123");
        }
    }

    // Without a Content-Length the body runs to the end of the stream, including
    // bytes that arrive after the header was read.
    #[tokio::test]
    async fn a_response_without_content_length_reads_its_body_to_the_end() {
        let head: &[u8] = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
        let rest: &[u8] = br#"{"late":"body"}"#;
        let body = super::read_http_response(&mut head.chain(rest))
            .await
            .unwrap();
        assert_eq!(body, rest);
    }

    // F-238: an HTTP/1.1 client must read a chunked response (RFC 9112 6.1):
    // a front may re-frame central's answer that way. The body ends at the last
    // chunk, so a front that then closes without close_notify still enrolls.
    #[tokio::test]
    async fn a_chunked_response_is_decoded() {
        let body = format!(
            r#"{{"protocol_version":{PROTOCOL_VERSION},"agent_id":"agent-xyz","credential":"cred-abc123"}}"#
        );
        let (first, second) = body.split_at(10);
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n{:x};ext=1\r\n{first}\r\n{:X}\r\n{second}\r\n0\r\nx-trailer: 1\r\n\r\n",
            first.len(),
            second.len()
        );
        let presented = enroll_through_a_front_without_close_notify(response)
            .await
            .expect("a chunked body is an enrollment response");

        assert_eq!(presented.response.agent_id, "agent-xyz");
        assert_eq!(presented.response.credential, "cred-abc123");
    }

    // Malformed or truncated chunk framing is an error, never a panic and never
    // the raw framing handed on as the body.
    #[tokio::test]
    async fn a_malformed_chunked_response_is_refused() {
        for framing in [
            "zz\r\nab\r\n0\r\n\r\n",
            "+2\r\nab\r\n0\r\n\r\n",
            "\r\nab\r\n0\r\n\r\n",
            "2\r\nabc\r\n0\r\n\r\n",
            "ffffffffffffffffffff\r\nab\r\n0\r\n\r\n",
            "ffffffffffffffff\r\nab\r\n0\r\n\r\n",
            "2\r\nab\r\n",
            "2\r\nab\r\n0\r\n",
            "",
        ] {
            let response =
                format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{framing}");
            let result = super::read_http_response(&mut response.as_bytes()).await;
            assert!(
                matches!(result, Err(EnrollError::Transport(_))),
                "{framing:?}: {result:?}"
            );
        }
    }

    // A central (or front) that accepts TCP and never answers fails the
    // exchange at its deadline with a clear error instead of hanging the install.
    #[tokio::test]
    async fn a_central_that_never_answers_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let silent = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((tcp, _)) = listener.accept().await {
                held.push(tcp);
            }
        });
        let mut connector =
            HttpsEnrollConnector::new(&format!("https://{addr}"), &"0".repeat(64)).unwrap();
        connector.timeout = std::time::Duration::from_millis(200);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            connector.exchange(EnrollRequest {
                protocol_version: PROTOCOL_VERSION,
                token: "enrollment-token".to_string(),
            }),
        )
        .await
        .expect("the exchange ends at its own deadline, not still pending after 5 s");
        silent.abort();

        let error = result
            .err()
            .expect("a silent central is an error")
            .to_string();
        assert!(error.contains("did not answer"), "{error}");
    }

    // Only a 200 is an enrollment; any other status is central refusing it,
    // whatever body comes with it.
    #[tokio::test]
    async fn a_non_200_response_is_a_refusal() {
        for status in [
            "201 Created",
            "302 Found",
            "403 Forbidden",
            "500 Internal Server Error",
        ] {
            let response = format!("HTTP/1.1 {status}\r\nContent-Length: 2\r\n\r\n{{}}");
            let result = super::read_http_response(&mut response.as_bytes()).await;
            assert!(
                matches!(result, Err(EnrollError::Transport(ref message)) if message == "central refused enrollment"),
                "{status}: {result:?}"
            );
        }
        // 101 switches protocols, so it is final, not an interim head to skip:
        // a 200 after it is not an enrollment.
        let response = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: h2c\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        let result = super::read_http_response(&mut response.as_bytes()).await;
        assert!(
            matches!(result, Err(EnrollError::Transport(ref message)) if message == "central refused enrollment"),
            "101 then 200: {result:?}"
        );
    }

    // F-328: a refusal carrying central's JSON error shows its message, so the
    // operator can tell a central misconfiguration from a bad token.
    #[tokio::test]
    async fn a_refusal_shows_centrals_message() {
        for (status, body, framing) in [
            (
                "503 Service Unavailable",
                r#"{"error":"identity_mismatch","message":"fix LG_TUNNEL_CERT"}"#,
                "length",
            ),
            (
                "503 Service Unavailable",
                r#"{"error":"identity_mismatch","message":"fix LG_TUNNEL_CERT"}"#,
                "chunked",
            ),
        ] {
            let response = if framing == "chunked" {
                format!("HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n", body.len())
            } else {
                format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
            };
            let result = super::read_http_response(&mut response.as_bytes()).await;
            let message = serde_json::from_str::<serde_json::Value>(body).unwrap()["message"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(
                matches!(result, Err(EnrollError::Transport(ref m)) if m == &format!("central refused enrollment: {message}")),
                "{status}: {result:?}"
            );
        }
    }

    // F-341: central's messages for a bad token or a cleartext hop are written
    // for a browser, so the agent names the operator's fix for those codes, and
    // shows central's own text only for codes meant for operators, stripped of
    // control characters and capped in length.
    #[tokio::test]
    async fn a_refusal_names_the_operators_fix() {
        let long = format!("fix\u{1b}[2J LG_TUNNEL_CERT {}", "x".repeat(2000));
        for (status, body, expected) in [
            (
                "401 Unauthorized",
                r#"{"error":"unauthorized","message":"Authentication required."}"#.to_string(),
                "the install token is invalid, expired or already used; generate a new install command".to_string(),
            ),
            (
                "403 Forbidden",
                r#"{"error":"insecure_transport","message":"A secure (TLS) connection is required for this action."}"#.to_string(),
                "central did not see HTTPS; the proxy in front of central must send X-Forwarded-Proto: https and be listed in LG_TRUSTED_PROXIES".to_string(),
            ),
            (
                "503 Service Unavailable",
                serde_json::json!({"error": "identity_mismatch", "message": long}).to_string(),
                format!("fix[2J LG_TUNNEL_CERT {}", "x".repeat(512 - 22)),
            ),
            (
                "503 Service Unavailable",
                r#"{"error":"tunnel_unavailable","message":"The operator must set LG_TUNNEL_CERT and LG_TUNNEL_KEY."}"#.to_string(),
                "The operator must set LG_TUNNEL_CERT and LG_TUNNEL_KEY.".to_string(),
            ),
            (
                "422 Unprocessable Entity",
                r#"{"error":"invalid_input","message":"Unsupported protocol version."}"#.to_string(),
                "Unsupported protocol version.".to_string(),
            ),
            (
                "429 Too Many Requests",
                r#"{"error":"rate_limited","message":"Too many attempts. Try again later."}"#.to_string(),
                "Too many attempts. Try again later.".to_string(),
            ),
            (
                "500 Internal Server Error",
                r#"{"error":"internal_error","message":"Something went wrong."}"#.to_string(),
                String::new(),
            ),
        ] {
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let result = super::read_http_response(&mut response.as_bytes()).await;
            let wanted = if expected.is_empty() {
                "central refused enrollment".to_string()
            } else {
                format!("central refused enrollment: {expected}")
            };
            assert!(
                matches!(result, Err(EnrollError::Transport(ref m)) if *m == wanted),
                "{status}: {result:?}"
            );
        }
    }

    // F-345: a header with a non-UTF-8 (obs-text) byte does not hide the
    // framing headers, so a chunked or length-framed 200 still decodes.
    #[tokio::test]
    async fn framing_is_found_past_a_non_utf8_header() {
        let body = br#"{"protocol_version":1}"#;
        let mut chunked =
            b"HTTP/1.1 200 OK\r\nServer: proxy\xe9\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        chunked.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
        chunked.extend_from_slice(body);
        chunked.extend_from_slice(b"\r\n0\r\n\r\n");
        let mut length = b"HTTP/1.1 200 OK\r\nServer: proxy\xe9\r\n".to_vec();
        length.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        length.extend_from_slice(body);
        length.extend_from_slice(b"trailing bytes past the body");
        for response in [chunked, length] {
            let decoded = super::read_http_response(&mut response.as_slice()).await;
            assert_eq!(decoded.ok().as_deref(), Some(&body[..]));
        }
    }

    // The re-pin only replaces the legacy pin it verified. A credential
    // rewritten meanwhile (a re-enrollment) is left as it is.
    #[test]
    fn repin_leaves_a_credential_without_that_legacy_pin_alone() {
        let path = std::env::temp_dir().join(format!(
            "lg-agent-repin-guard-{}-{}.json",
            std::process::id(),
            fingerprint(TEST_CERT.as_bytes())
        ));
        let reenrolled = AgentCredential {
            agent_id: "agent-new".to_string(),
            credential: "cred-new".to_string(),
            central_url: "https://central.test".to_string(),
            tunnel_url: "https://tunnel.central.test:8443".to_string(),
            fingerprint: fingerprint(b"new-central"),
        };
        super::store_credential(&path, &reenrolled).unwrap();
        let before = std::fs::read(&path).unwrap();

        super::repin_credential(&path, &fingerprint(b"old-central"), &fingerprint(b"key")).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = std::fs::remove_file(&path);
    }

    // The startup check and the dialer read the tunnel URL through
    // one parse, so a URL the dialer would mis-dial (bracketed IPv6 literal, or no
    // explicit port) is refused before the tunnel starts.
    #[test]
    fn a_stored_tunnel_url_needs_an_explicit_host_and_port() {
        let valid = AgentCredential {
            agent_id: "agent-xyz".to_string(),
            credential: "cred-abc123".to_string(),
            central_url: "https://central.test".to_string(),
            tunnel_url: "https://tunnel.central.test:9443".to_string(),
            fingerprint: fingerprint(b"central-identity"),
        };
        validate_stored_credential(&valid).expect("an explicit host:port is accepted");
        let config = TunnelClientConfig::from_parts(
            &valid.tunnel_url,
            valid.fingerprint.clone(),
            valid.agent_id.clone(),
            valid.credential.clone(),
        );
        assert_eq!(
            (config.host.as_str(), config.port),
            ("tunnel.central.test", 9443)
        );

        for tunnel_url in [
            "https://[::1]:8443",
            "https://[2001:db8::1]",
            "https://tunnel.central.test",
        ] {
            let mut credential = valid.clone();
            credential.tunnel_url = tunnel_url.to_string();
            assert!(
                matches!(
                    validate_stored_credential(&credential),
                    Err(EnrollError::InvalidCredential(_))
                ),
                "tunnel URL {tunnel_url:?} must be refused before the tunnel starts"
            );
        }
    }
}
