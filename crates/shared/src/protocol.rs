//! The versioned agent↔central wire protocol and the identity-pin primitives it
//! rides on. Shared verbatim by `central` (which issues) and `agent` (which
//! enrolls) so both agree on the handshake shape, the protocol version, and how a
//! central-identity fingerprint is computed — one definition, no drift.
//!
//! This slice defines enrollment only. The persistent authenticated tunnel that
//! carries per-frame command traffic (and re-pins central on reconnect) is layered
//! on top in a later slice; the [`PROTOCOL_VERSION`] here is the field that lets
//! that later change be a negotiated upgrade rather than a fleet-breaking migration.

use std::future::Future;
use std::io;

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::exec::ExecStatus;
use crate::template::Method;

/// The wire protocol version, present in every handshake from the first release.
/// A peer that speaks a different version is rejected rather than guessed at.
pub const PROTOCOL_VERSION: u16 = 1;

/// Environment variables the copy-paste install command sets for the agent. Named
/// once here so the central-side generator and the agent-side parser cannot drift.
pub const ENV_CENTRAL_URL: &str = "LG_CENTRAL_URL";
pub const ENV_TUNNEL_URL: &str = "LG_TUNNEL_URL";
pub const ENV_CENTRAL_FINGERPRINT: &str = "LG_CENTRAL_FP";
pub const ENV_ENROLL_TOKEN: &str = "LG_ENROLL_TOKEN";
pub const ENV_AGENT_URL: &str = "LG_AGENT_URL";
pub const ENV_AGENT_SHA256: &str = "LG_AGENT_SHA256";
pub const ENV_AGENT_INSTALL_SCRIPT_URL: &str = "LG_AGENT_INSTALL_SCRIPT_URL";
pub const ENV_AGENT_INSTALL_SCRIPT_SHA256: &str = "LG_AGENT_INSTALL_SCRIPT_SHA256";
const ROOT_INSTALL_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// The agent's enrollment request: the protocol version it speaks and the raw
/// single-use token it was handed in the install command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollRequest {
    pub protocol_version: u16,
    pub token: String,
}

/// Central's response to a successful enrollment: the agent's assigned id and the
/// one-time cleartext of its long-lived credential (central keeps only the hash).
/// The credential is returned exactly once, here — never logged, never re-derivable.
/// No `Debug` derive: a `{:?}` on it would leak the one-time credential, so the
/// "never logged" invariant is enforced structurally, not by discipline.
#[derive(Clone, Serialize, Deserialize)]
pub struct EnrollResponse {
    pub protocol_version: u16,
    pub agent_id: String,
    pub credential: String,
}

/// The SHA-256 fingerprint (lowercase hex) of the whole identity material. For a
/// certificate this is the legacy pin: agents enrolled before [`identity_pin`]
/// hold it, and it changes with every renewal.
pub fn fingerprint(identity_material: &[u8]) -> String {
    sha256_hex(identity_material)
}

/// The pin central mints and the agent verifies: SHA-256 (lowercase hex) of the
/// leaf certificate's DER SubjectPublicKeyInfo, so a renewal that keeps the key
/// keeps the pin. Material that does not parse as a certificate pins by its
/// [`fingerprint`].
pub fn identity_pin(cert_der: &[u8]) -> String {
    let der = cert_der.into();
    match webpki::EndEntityCert::try_from(&der) {
        Ok(cert) => sha256_hex(cert.subject_public_key_info().as_ref()),
        Err(_) => fingerprint(cert_der),
    }
}

/// Lowercase-hex SHA-256 of `bytes`. Used both for the identity fingerprint and to
/// hash a high-entropy enrollment token at rest (a 256-bit random token needs no
/// slow KDF — unlike a password — so a plain digest keyed lookup is correct here).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Constant-time equality for two byte slices. Used to compare the pinned
/// fingerprint against central's presented fingerprint (and hashed tokens) without
/// leaking a match position through timing.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The three values that make an enrollment self-contained: where central lives,
/// the fingerprint to pin it by, and the single-use token. Central builds this and
/// renders a no-edit install command; the agent reconstructs it from its environment.
#[derive(Debug, Clone)]
pub struct EnrollmentParams {
    pub central_url: String,
    pub tunnel_url: String,
    pub fingerprint: String,
    pub token: String,
    pub agent_url: String,
    pub agent_sha256: String,
    pub install_script_url: String,
    pub install_script_sha256: String,
}

impl EnrollmentParams {
    /// Render the copy-paste install command. Every value is baked in — there is
    /// nothing for the operator to edit (FR-021). The installer receives the
    /// expected binary SHA-256 before it downloads, installs, or starts the agent.
    pub fn install_command(&self) -> String {
        self.install_command_with_env(ROOT_INSTALL_PATH, &[])
    }

    #[cfg(debug_assertions)]
    pub fn install_command_for_test(&self, additional_env: &[(&str, &str)]) -> String {
        for (name, _) in additional_env {
            assert!(
                is_allowed_test_install_env(name),
                "test install env is not allowlisted: {name}"
            );
        }
        self.install_command_with_env(ROOT_INSTALL_PATH, additional_env)
    }

    fn install_command_with_env(&self, root_path: &str, additional_env: &[(&str, &str)]) -> String {
        let installer_workflow = "IFS= read -r LG_ENROLL_TOKEN && export LG_ENROLL_TOKEN && workdir=$(mktemp -d -t lookingglass-agent.XXXXXXXXXX) && trap 'rm -rf \"$workdir\"' EXIT && [ \"$(stat -c %u \"$workdir\")\" = 0 ] && tmp=\"$workdir/install-agent.sh\" && curl -fsSL \"$LG_AGENT_INSTALL_SCRIPT_URL\" -o \"$tmp\" && printf '%s  %s\\n' \"$LG_AGENT_INSTALL_SCRIPT_SHA256\" \"$tmp\" | sha256sum -c - && bash \"$tmp\"";
        let mut installer_env = format!(
            "PATH={} {ENV_AGENT_URL}={} {ENV_AGENT_SHA256}={} {ENV_CENTRAL_URL}={} {ENV_TUNNEL_URL}={} {ENV_CENTRAL_FINGERPRINT}={} {ENV_AGENT_INSTALL_SCRIPT_URL}={} {ENV_AGENT_INSTALL_SCRIPT_SHA256}={}",
            shell_quote(root_path),
            shell_quote(&self.agent_url),
            shell_quote(&self.agent_sha256),
            shell_quote(&self.central_url),
            shell_quote(&self.tunnel_url),
            shell_quote(&self.fingerprint),
            shell_quote(&self.install_script_url),
            shell_quote(&self.install_script_sha256),
        );
        for (name, value) in additional_env {
            assert!(
                is_shell_env_name(name),
                "invalid environment variable name: {name}"
            );
            installer_env.push(' ');
            installer_env.push_str(name);
            installer_env.push('=');
            installer_env.push_str(&shell_quote(value));
        }
        // The token goes in on stdin from the pasting shell's builtin `printf`, so it
        // never appears in the argv of `sudo`/`env` that `ps` shows for the install.
        format!(
            "printf '%s\\n' {} | if [ \"$(id -u)\" -eq 0 ]; then env -i {installer_env} bash -c {}; else sudo env -i {installer_env} bash -c {}; fi",
            shell_quote(&self.token),
            shell_quote(installer_workflow),
            shell_quote(installer_workflow),
        )
    }
}

fn is_shell_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first == b'_' || first.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

#[cfg(debug_assertions)]
fn is_allowed_test_install_env(name: &str) -> bool {
    matches!(
        name,
        "LG_INSTALL_DRY_RUN" | "LG_INSTALL_DRY_RUN_STATE_DIR" | "LG_ENROLL_RESPONSE_FILE"
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

// ---------------------------------------------------------------------------
// Authenticated tunnel (Slice 8) — the per-frame-authenticated channel the agent
// and central speak once TLS is up. TLS gives the channel + central-identity pin;
// this layer's job is *agent* authentication and per-frame integrity/replay, so
// the payload rides plaintext-under-TLS and carries an HMAC tag, not a second
// encryption layer (decisions.md "Slice 8 tunnel per-frame auth model").
// ---------------------------------------------------------------------------

/// A 256-bit tunnel session key or nonce.
pub const TUNNEL_KEY_BYTES: usize = 32;

/// The agent's opening handshake frame: the protocol version, its assigned id, a
/// one-time proof of its long-lived credential, and a fresh client nonce. The
/// credential rides here under TLS and is verified once; it is never logged, so
/// this type deliberately does **not** derive `Debug`.
#[derive(Serialize, Deserialize)]
pub struct TunnelHello {
    pub protocol_version: u16,
    pub agent_id: String,
    pub credential: String,
    pub client_nonce: [u8; TUNNEL_KEY_BYTES],
    /// The agent understands [`TunnelMessage::Cancel`]. Absent from an older
    /// agent's hello, so it defaults to `false` and central never sends one.
    #[serde(default)]
    pub accepts_cancel: bool,
    /// The agent understands [`TunnelMessage::DataPlane`] and may answer with
    /// [`TunnelMessage::Certificate`]. Absent from an older agent's hello, so
    /// central never sends it a data-plane origin.
    #[serde(default)]
    pub accepts_data_plane: bool,
}

/// Central's reply to a verified handshake: its own fresh nonce. Both nonces bind
/// into the transcript so each session derives a distinct key — a frame captured
/// on one session cannot validate on the next (its tag was computed under the old
/// key).
#[derive(Debug, Serialize, Deserialize)]
pub struct TunnelAccept {
    pub server_nonce: [u8; TUNNEL_KEY_BYTES],
}

/// The per-run bounds a relayed [`TunnelMessage::Command`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLimits {
    pub timeout_secs: u64,
    pub max_output_bytes: usize,
}

/// An application message carried inside an authenticated frame. `Command` flows
/// down (central → agent); `Output`/`Done`/`Error` flow up. `Heartbeat` is a
/// reserved variant for the Slice-8b liveness window — no liveness logic lives
/// here, only the wire shape so 8b is an additive change, not a protocol break.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelMessage {
    /// Relay a diagnostic down. The agent re-validates `target` (the SSRF
    /// boundary) and builds the argv itself — central never ships an argv.
    Command {
        run_id: String,
        method: String,
        target: String,
        /// The run's bounds from central's saved Execution limits. An older
        /// central omits them (the agent runs with its own defaults) and an
        /// older agent ignores them, so either side may be the newer one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limits: Option<RunLimits>,
    },
    /// One line of relayed output, streamed up as it is produced.
    Output { run_id: String, line: String },
    /// The relayed run reached a terminal state; `ok` mirrors its exit success.
    /// `status` names how it ended ([`TunnelMessage::done`]). An agent that
    /// predates it omits the field and an older central ignores it, so either
    /// side may be the newer one.
    Done {
        run_id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    /// A terminal error surfaced to the visitor (AC41) — a refused start, a
    /// failed spawn, or a truncation. It is the run's only terminal frame: no
    /// `Done` follows it. `status` names how the run ended, as on `Done`
    /// ([`TunnelMessage::error`]); a refused start and an older agent omit it.
    Error {
        run_id: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    /// Reserved for Slice-8b liveness. Carries no logic in this slice.
    Heartbeat,
    /// Stop one relayed run (its visitor left) without closing the tunnel; the
    /// agent answers with the run's terminal frame. Down only, and only to an
    /// agent whose hello set [`TunnelHello::accepts_cancel`]: an older agent
    /// fails to decode an unknown variant and would drop the whole tunnel.
    Cancel { run_id: String },
    /// The data-plane origin central assigned to this node's location. Down
    /// only, and only to an agent whose hello set
    /// [`TunnelHello::accepts_data_plane`]; the agent serves its speed-test data
    /// plane there over HTTPS with a certificate it obtains itself.
    DataPlane { origin: String },
    /// The data-plane certificate's state. Up only, and only after the agent
    /// received a `DataPlane` on this connection, so only a central that
    /// understands it is ever sent one.
    Certificate(CertificateReport),
}

/// A [`TunnelMessage::Certificate`] report: the status, and the data-plane
/// origin it was obtained for. An older agent omits `origin`, and an older
/// central ignores it (neither side denies unknown fields), so either side may
/// be the newer one; the status keeps its wire shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificateReport {
    #[serde(flatten)]
    pub status: CertificateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// The agent's data-plane certificate as the admin sees it: when the served
/// certificate was issued and expires (unix seconds), and the last issuance or
/// renewal error, cleared by the next success.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificateStatus {
    #[serde(default)]
    pub issued_at: Option<u64>,
    #[serde(default)]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl TunnelMessage {
    /// The `Done` frame for a run that ended with `status`.
    pub fn done(run_id: &str, status: ExecStatus) -> Self {
        TunnelMessage::Done {
            run_id: run_id.to_string(),
            ok: status == ExecStatus::Completed { success: true },
            status: Some(status_label(status).to_string()),
        }
    }

    /// The `Error` frame for a run that failed with `message` and ended with
    /// `status`.
    pub fn error(run_id: &str, message: String, status: ExecStatus) -> Self {
        TunnelMessage::Error {
            run_id: run_id.to_string(),
            message,
            status: Some(status_label(status).to_string()),
        }
    }

    /// How a run ended, read back from a `Done` or `Error` frame. `None` for
    /// any other frame, for an agent that sent no status, or for a label this
    /// build does not know.
    pub fn terminal_status(&self) -> Option<ExecStatus> {
        let (ok, label) = match self {
            TunnelMessage::Done {
                ok,
                status: Some(label),
                ..
            } => (*ok, label),
            TunnelMessage::Error {
                status: Some(label),
                ..
            } => (false, label),
            _ => return None,
        };
        Some(match label.as_str() {
            "completed" => ExecStatus::Completed { success: ok },
            "timeout" => ExecStatus::TimedOut,
            "truncated" => ExecStatus::OutputCapped,
            "canceled" => ExecStatus::Canceled,
            "failed" => ExecStatus::Failed,
            _ => return None,
        })
    }

    /// The run this frame belongs to, or `None` for a run-less frame (heartbeat).
    /// Used to correlate an inbound frame with the run currently being relayed —
    /// a mismatch is a protocol violation, never a frame to forward.
    pub fn run_id(&self) -> Option<&str> {
        match self {
            TunnelMessage::Command { run_id, .. }
            | TunnelMessage::Output { run_id, .. }
            | TunnelMessage::Done { run_id, .. }
            | TunnelMessage::Error { run_id, .. }
            | TunnelMessage::Cancel { run_id } => Some(run_id),
            TunnelMessage::Heartbeat
            | TunnelMessage::DataPlane { .. }
            | TunnelMessage::Certificate(_) => None,
        }
    }
}

/// The wire label for how a run ended, as a local run's `done` names it.
fn status_label(status: ExecStatus) -> &'static str {
    match status {
        ExecStatus::Completed { .. } => "completed",
        ExecStatus::TimedOut => "timeout",
        ExecStatus::OutputCapped => "truncated",
        ExecStatus::Canceled => "canceled",
        ExecStatus::Failed => "failed",
    }
}

impl Method {
    /// The wire name central relays and the agent runs: the one mapping both
    /// sides use, as [`PrefixFamily::wire`](crate::validate::PrefixFamily::wire)
    /// is for BGP.
    pub fn wire(self) -> &'static str {
        match self {
            Method::Ping => "ping",
            Method::Ping6 => "ping6",
            Method::Mtr => "mtr",
            Method::Mtr6 => "mtr6",
            Method::Traceroute => "traceroute",
            Method::Traceroute6 => "traceroute6",
        }
    }

    /// Recognise a diagnostic wire name, or `None` for BGP or an unknown name.
    pub fn from_wire(name: &str) -> Option<Self> {
        Some(match name {
            "ping" => Method::Ping,
            "ping6" => Method::Ping6,
            "mtr" => Method::Mtr,
            "mtr6" => Method::Mtr6,
            "traceroute" => Method::Traceroute,
            "traceroute6" => Method::Traceroute6,
            _ => return None,
        })
    }
}

/// Every way the authenticated channel can refuse a frame or fail a handshake.
/// A bad tag or an out-of-order counter is fatal to the channel: the caller tears
/// the tunnel down (fail closed), never continues past an auth failure.
#[derive(Debug)]
pub enum TunnelError {
    /// The underlying transport (WebSocket / socket) errored.
    Transport(io::Error),
    /// The peer closed the channel before a frame was read.
    Closed,
    /// A frame was too short or its payload did not deserialize.
    Malformed,
    /// The HMAC tag did not verify — a forged or corrupted frame.
    BadTag,
    /// The frame counter was not exactly the next expected value — a replayed or
    /// dropped/gapped frame.
    ReplayOrGap { expected: u64, got: u64 },
    /// The peer speaks a different wire protocol version.
    ProtocolMismatch { got: u16 },
    /// Central's presented identity did not match the pinned fingerprint.
    IdentityMismatch,
    /// Central refused the handshake — unknown, revoked, or wrong-credential
    /// agent. Uniform so it is no oracle for which case applied.
    AuthRejected,
}

impl std::fmt::Display for TunnelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TunnelError::Transport(e) => write!(f, "tunnel transport error: {e}"),
            TunnelError::Closed => f.write_str("tunnel closed by peer"),
            TunnelError::Malformed => f.write_str("malformed tunnel frame"),
            TunnelError::BadTag => f.write_str("tunnel frame failed authentication"),
            TunnelError::ReplayOrGap { expected, got } => {
                write!(
                    f,
                    "tunnel frame out of order: expected {expected}, got {got}"
                )
            }
            TunnelError::ProtocolMismatch { got } => {
                write!(
                    f,
                    "peer speaks tunnel protocol version {got}, expected {PROTOCOL_VERSION}"
                )
            }
            TunnelError::IdentityMismatch => {
                f.write_str("central identity does not match the pinned fingerprint")
            }
            TunnelError::AuthRejected => f.write_str("tunnel handshake refused"),
        }
    }
}

impl std::error::Error for TunnelError {}

/// Verify central's presented certificate against the pin from the install
/// command (Slice 7): its [`identity_pin`], or the legacy whole-certificate
/// [`fingerprint`] an older credential holds. Constant-time so a partial match is
/// not timed; a mismatch is fatal — the agent aborts, no trust-on-first-use. This
/// runs inside the TLS certificate verifier on **every** (re)connect, so a central
/// with another key is refused at connect and at reconnect.
pub fn verify_pinned_identity(
    presented: &[u8],
    pinned_fingerprint: &str,
) -> Result<(), TunnelError> {
    let pinned = pinned_fingerprint.as_bytes();
    let key_match = constant_time_eq(identity_pin(presented).as_bytes(), pinned);
    let legacy_match = constant_time_eq(fingerprint(presented).as_bytes(), pinned);
    if key_match | legacy_match {
        Ok(())
    } else {
        Err(TunnelError::IdentityMismatch)
    }
}

/// The bytes both sides hash into the session key. Binding the version, agent id,
/// and both nonces makes the key unique and unpredictable per session. Fields are
/// length-prefixed so no two distinct handshakes share a transcript by accident.
fn handshake_transcript(
    agent_id: &str,
    client_nonce: &[u8; TUNNEL_KEY_BYTES],
    server_nonce: &[u8; TUNNEL_KEY_BYTES],
) -> Vec<u8> {
    let id = agent_id.as_bytes();
    let mut out = Vec::with_capacity(2 + 4 + id.len() + TUNNEL_KEY_BYTES * 2);
    out.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    out.extend_from_slice(&(id.len() as u32).to_be_bytes());
    out.extend_from_slice(id);
    out.extend_from_slice(client_nonce);
    out.extend_from_slice(server_nonce);
    out
}

/// Derive the per-session key: `HKDF-SHA256(ikm = credential, info = transcript)`.
/// The key's secrecy rests entirely on the credential (the HKDF input keying
/// material); the transcript is public and only makes the key session-unique.
/// Held in RAM only — never persisted, never logged.
pub fn derive_session_key(credential: &[u8], transcript: &[u8]) -> [u8; TUNNEL_KEY_BYTES] {
    let hkdf = Hkdf::<Sha256>::new(None, credential);
    let mut key = [0u8; TUNNEL_KEY_BYTES];
    hkdf.expand(transcript, &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    key
}

/// Direction labels folded into every frame tag so the two directions of one
/// session are cryptographically distinct: a frame minted for one direction fails
/// verification if reflected into the other, even at the same counter. Both peers
/// share one session key, so without this a `central → agent` frame would be a
/// valid `agent → central` frame at the same counter index.
const DIR_AGENT_TO_CENTRAL: u8 = 0x01;
const DIR_CENTRAL_TO_AGENT: u8 = 0x02;

/// The per-frame tag: `HMAC-SHA256(session_key, direction ‖ counter_be ‖ payload)`.
/// Authing the direction and counter under the key makes both the replay guard and
/// the direction guard unforgeable — tampering with either breaks the tag.
fn frame_tag(
    session_key: &[u8; TUNNEL_KEY_BYTES],
    direction: u8,
    counter: u64,
    payload: &[u8],
) -> [u8; 32] {
    let mut mac =
        <Hmac<Sha256>>::new_from_slice(session_key).expect("HMAC-SHA256 accepts any key length");
    mac.update(&[direction]);
    mac.update(&counter.to_be_bytes());
    mac.update(payload);
    mac.finalize().into_bytes().into()
}

/// A bidirectional transport carrying opaque byte frames. The production impls
/// wrap a WebSocket (`central`/`agent`); [`ChannelTransport`] is the in-process
/// impl the tunnel tests drive. Framing/delimiting is the transport's job; the
/// [`AuthChannel`] on top adds authentication.
pub trait FrameTransport {
    fn send(&mut self, frame: Vec<u8>) -> impl Future<Output = io::Result<()>> + Send;
    fn recv(&mut self) -> impl Future<Output = io::Result<Option<Vec<u8>>>> + Send;
}

/// The per-frame-authenticated channel over a [`FrameTransport`]. Every send
/// stamps a strictly increasing counter and an HMAC tag; every receive verifies
/// the tag (constant-time) **and** asserts the counter is exactly the next
/// expected value. A bad tag or an out-of-order counter returns an error and the
/// caller tears the channel down — auth is enforced per frame, not just at the
/// handshake.
pub struct AuthChannel<T: FrameTransport> {
    transport: T,
    session_key: [u8; TUNNEL_KEY_BYTES],
    send_direction: u8,
    recv_direction: u8,
    send_counter: u64,
    expected_recv: u64,
    peer_accepts_cancel: bool,
    peer_accepts_data_plane: bool,
}

/// The counter(8) + tag(32) header every authenticated frame carries before its
/// payload.
const FRAME_HEADER_LEN: usize = 8 + 32;

impl<T: FrameTransport> AuthChannel<T> {
    /// `send_direction` is the label this side stamps on frames it sends;
    /// `recv_direction` is the label it expects on frames it receives. The two are
    /// opposite for the two peers, so neither side accepts a frame it (or a
    /// reflector) minted in its own send direction.
    fn new(
        transport: T,
        session_key: [u8; TUNNEL_KEY_BYTES],
        send_direction: u8,
        recv_direction: u8,
    ) -> Self {
        Self {
            transport,
            session_key,
            send_direction,
            recv_direction,
            send_counter: 0,
            expected_recv: 0,
            peer_accepts_cancel: false,
            peer_accepts_data_plane: false,
        }
    }

    /// Whether the peer's hello said it understands [`TunnelMessage::Cancel`].
    pub fn peer_accepts_cancel(&self) -> bool {
        self.peer_accepts_cancel
    }

    /// Whether the peer's hello said it understands [`TunnelMessage::DataPlane`].
    pub fn peer_accepts_data_plane(&self) -> bool {
        self.peer_accepts_data_plane
    }

    /// Frame, tag, and send a raw payload under the next counter.
    pub async fn send(&mut self, payload: &[u8]) -> Result<(), TunnelError> {
        let counter = self.send_counter;
        let tag = frame_tag(&self.session_key, self.send_direction, counter, payload);
        let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
        frame.extend_from_slice(&counter.to_be_bytes());
        frame.extend_from_slice(&tag);
        frame.extend_from_slice(payload);
        self.transport
            .send(frame)
            .await
            .map_err(TunnelError::Transport)?;
        self.send_counter += 1;
        Ok(())
    }

    /// Receive the next frame, verifying its tag and counter. The tag is checked
    /// before the counter is trusted, since the tag authenticates the counter.
    pub async fn recv(&mut self) -> Result<Vec<u8>, TunnelError> {
        let frame = self
            .transport
            .recv()
            .await
            .map_err(TunnelError::Transport)?
            .ok_or(TunnelError::Closed)?;
        if frame.len() < FRAME_HEADER_LEN {
            return Err(TunnelError::Malformed);
        }
        let counter = u64::from_be_bytes(frame[0..8].try_into().expect("8-byte counter slice"));
        let tag = &frame[8..FRAME_HEADER_LEN];
        let payload = &frame[FRAME_HEADER_LEN..];
        let expected_tag = frame_tag(&self.session_key, self.recv_direction, counter, payload);
        if !constant_time_eq(tag, &expected_tag) {
            return Err(TunnelError::BadTag);
        }
        if counter != self.expected_recv {
            return Err(TunnelError::ReplayOrGap {
                expected: self.expected_recv,
                got: counter,
            });
        }
        self.expected_recv += 1;
        Ok(payload.to_vec())
    }

    /// Serialize, frame, and send a typed message.
    pub async fn send_message(&mut self, message: &TunnelMessage) -> Result<(), TunnelError> {
        let bytes = serde_json::to_vec(message).map_err(|_| TunnelError::Malformed)?;
        self.send(&bytes).await
    }

    /// Receive and deserialize the next typed message (tag + counter verified).
    pub async fn recv_message(&mut self) -> Result<TunnelMessage, TunnelError> {
        let bytes = self.recv().await?;
        serde_json::from_slice(&bytes).map_err(|_| TunnelError::Malformed)
    }
}

/// Agent side of the handshake: prove the credential once, receive central's
/// nonce, and derive the shared session key. Returns the authenticated channel.
/// `client_nonce` is supplied by the caller (a CSPRNG in production) so this is
/// deterministic under test.
pub async fn client_handshake<T: FrameTransport>(
    mut transport: T,
    agent_id: &str,
    credential: &str,
    client_nonce: [u8; TUNNEL_KEY_BYTES],
) -> Result<AuthChannel<T>, TunnelError> {
    let hello = TunnelHello {
        protocol_version: PROTOCOL_VERSION,
        agent_id: agent_id.to_string(),
        credential: credential.to_string(),
        client_nonce,
        accepts_cancel: true,
        accepts_data_plane: true,
    };
    let bytes = serde_json::to_vec(&hello).map_err(|_| TunnelError::Malformed)?;
    transport
        .send(bytes)
        .await
        .map_err(TunnelError::Transport)?;

    let reply = transport
        .recv()
        .await
        .map_err(TunnelError::Transport)?
        .ok_or(TunnelError::AuthRejected)?;
    let accept: TunnelAccept =
        serde_json::from_slice(&reply).map_err(|_| TunnelError::AuthRejected)?;

    let transcript = handshake_transcript(agent_id, &client_nonce, &accept.server_nonce);
    let key = derive_session_key(credential.as_bytes(), &transcript);
    // Agent sends in the agent→central direction and expects central→agent.
    Ok(AuthChannel::new(
        transport,
        key,
        DIR_AGENT_TO_CENTRAL,
        DIR_CENTRAL_TO_AGENT,
    ))
}

/// Central side of the handshake: read the agent's hello, verify its credential
/// (via `verify`, which fails closed on an unknown/revoked/wrong-credential
/// agent), send central's nonce, and derive the shared session key. Returns the
/// authenticated agent id and channel. A refused credential yields
/// [`TunnelError::AuthRejected`] and no channel — no session for an unverified
/// agent.
pub async fn server_handshake<T, V, Fut>(
    mut transport: T,
    server_nonce: [u8; TUNNEL_KEY_BYTES],
    verify: V,
) -> Result<(String, AuthChannel<T>), TunnelError>
where
    T: FrameTransport,
    V: FnOnce(String, String) -> Fut,
    Fut: Future<Output = bool>,
{
    let bytes = transport
        .recv()
        .await
        .map_err(TunnelError::Transport)?
        .ok_or(TunnelError::Closed)?;
    let hello: TunnelHello = serde_json::from_slice(&bytes).map_err(|_| TunnelError::Malformed)?;
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(TunnelError::ProtocolMismatch {
            got: hello.protocol_version,
        });
    }

    if !verify(hello.agent_id.clone(), hello.credential.clone()).await {
        return Err(TunnelError::AuthRejected);
    }

    let accept = TunnelAccept { server_nonce };
    let reply = serde_json::to_vec(&accept).map_err(|_| TunnelError::Malformed)?;
    transport
        .send(reply)
        .await
        .map_err(TunnelError::Transport)?;

    let transcript = handshake_transcript(&hello.agent_id, &hello.client_nonce, &server_nonce);
    let key = derive_session_key(hello.credential.as_bytes(), &transcript);
    // Central sends in the central→agent direction and expects agent→central.
    let mut channel = AuthChannel::new(transport, key, DIR_CENTRAL_TO_AGENT, DIR_AGENT_TO_CENTRAL);
    channel.peer_accepts_cancel = hello.accepts_cancel;
    channel.peer_accepts_data_plane = hello.accepts_data_plane;
    Ok((hello.agent_id, channel))
}

/// An in-process [`FrameTransport`] pair: a bounded mpsc in each direction. The
/// production transport is a WebSocket; this is the loopback the tunnel tests use
/// to drive both handshake sides and the authenticated frame exchange without a
/// socket.
pub struct ChannelTransport {
    outbound: mpsc::Sender<Vec<u8>>,
    inbound: mpsc::Receiver<Vec<u8>>,
}

impl ChannelTransport {
    /// A cross-wired pair — what one end sends, the other receives.
    pub fn pair() -> (Self, Self) {
        let (a_tx, a_rx) = mpsc::channel(64);
        let (b_tx, b_rx) = mpsc::channel(64);
        (
            Self {
                outbound: a_tx,
                inbound: b_rx,
            },
            Self {
                outbound: b_tx,
                inbound: a_rx,
            },
        )
    }
}

impl FrameTransport for ChannelTransport {
    async fn send(&mut self, frame: Vec<u8>) -> io::Result<()> {
        self.outbound
            .send(frame)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "tunnel peer went away"))
    }

    async fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        Ok(self.inbound.recv().await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_lowercase_hex_sha256() {
        // Known SHA-256 of the empty input, so the hex encoding is pinned, not
        // just "some 64 chars".
        assert_eq!(
            fingerprint(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let fp = fingerprint(b"central-identity");
        assert_eq!(fp.len(), 64);
        assert!(fp
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn distinct_identities_have_distinct_fingerprints() {
        assert_ne!(fingerprint(b"identity-a"), fingerprint(b"identity-b"));
    }

    #[test]
    fn constant_time_eq_matches_only_equal_slices() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn install_command_embeds_values_and_needs_no_edits() {
        let params = EnrollmentParams {
            central_url: "https://central.example:8443".to_string(),
            tunnel_url: "https://tunnel.central.example:8443".to_string(),
            fingerprint: fingerprint(b"central-identity"),
            token: "deadbeef".to_string(),
            agent_url: "https://downloads.example/lg-agent".to_string(),
            agent_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
            install_script_url: "https://downloads.example/install-agent.sh".to_string(),
            install_script_sha256:
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string(),
        };
        let cmd = params.install_command();
        assert!(cmd.contains(&params.fingerprint), "carries the fingerprint");
        assert!(cmd.contains("deadbeef"), "carries the token");
        assert!(
            cmd.contains(&params.agent_sha256),
            "carries the expected binary checksum"
        );
        assert!(
            cmd.contains(&params.install_script_sha256),
            "carries the expected installer checksum"
        );
        assert!(
            cmd.contains("sha256sum -c -"),
            "verifies the installer before running it"
        );
        assert!(
            !cmd.contains("curl -fsSL https://downloads.example/install-agent.sh |"),
            "must not execute a remotely fetched script without verifying it first: {cmd}"
        );
        assert!(
            !cmd.starts_with("tmp=$(mktemp)"),
            "non-root paste path must not verify a user-owned temp file before sudo: {cmd}"
        );
        assert!(
            cmd.contains("bash \"$tmp\""),
            "runs the verified temp file, not a pipe"
        );
        assert!(
            cmd.contains("env -i PATH='/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin' LG_AGENT_URL="),
            "root execution must scrub ambient environment and set only PATH plus installer variables"
        );
        for test_only in [
            "LG_INSTALL_DRY_RUN",
            "LG_AGENT_STATE_DIR",
            "LG_INSTALL_DRY_RUN_STATE_DIR",
            "LG_ENROLL_RESPONSE_FILE",
        ] {
            assert!(
                !cmd.contains(test_only),
                "production command must not carry test-only variable {test_only}: {cmd}"
            );
        }
        assert!(
            cmd.contains("sudo env -i"),
            "non-root paste path must escalate with a scrubbed environment: {cmd}"
        );
        assert!(
            !cmd.contains("sudo -E"),
            "paste path must not preserve the operator's ambient environment: {cmd}"
        );
        assert!(
            cmd.contains("if [ \"$(id -u)\" -eq 0 ]; then env -i"),
            "root paste path must run without requiring sudo and with env scrubbed: {cmd}"
        );
        assert!(
            cmd.contains("workdir=$(mktemp -d -t lookingglass-agent.XXXXXXXXXX)"),
            "download and execution must happen from a root-created temp directory: {cmd}"
        );
        assert!(
            cmd.contains("[ \"$(stat -c %u \"$workdir\")\" = 0 ]"),
            "root workflow must prove the temp path is root-owned before executing from it: {cmd}"
        );
        assert!(cmd.contains("https://central.example:8443"));
        assert!(cmd.contains("https://tunnel.central.example:8443"));
        assert!(cmd.contains("https://downloads.example/lg-agent"));
        assert!(cmd.contains("https://downloads.example/install-agent.sh"));
        // No placeholder an operator would have to fill in.
        for placeholder in ["<", ">", "REPLACE", "YOUR_", "TODO", "{{"] {
            assert!(
                !cmd.contains(placeholder),
                "command must need no manual edit, found {placeholder:?}: {cmd}"
            );
        }
    }

    /// The one-time token must not sit in the argv of `sudo`/`env`, which other local
    /// users can read via `ps` or `/proc/<pid>/cmdline` for the whole install. Stubs
    /// record each argv; `mktemp`, the workflow's first external command, reports the
    /// token it inherited. Both paste branches, under `sh` and `bash`.
    #[cfg(unix)]
    #[test]
    fn install_command_passes_the_token_on_stdin_not_argv() {
        use std::os::unix::fs::PermissionsExt;
        let token = "f00dfeedcafe0123";
        let cmd = EnrollmentParams {
            central_url: "https://central.example:8443".to_string(),
            tunnel_url: "https://tunnel.central.example:8443".to_string(),
            fingerprint: fingerprint(b"central-identity"),
            token: token.to_string(),
            agent_url: "https://downloads.example/lg-agent".to_string(),
            agent_sha256: "0".repeat(64),
            install_script_url: "https://downloads.example/install-agent.sh".to_string(),
            install_script_sha256: "a".repeat(64),
        }
        .install_command();
        let dir = std::env::temp_dir().join(format!("lg-install-argv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in [
            ("id", r#"echo "$LG_TEST_UID""#),
            (
                "sudo",
                r#"printf '%s\n' "sudo $*" >>"$LG_TEST_LOG"; exec "$@""#,
            ),
            (
                "env",
                r#"printf '%s\n' "env $*" >>"$LG_TEST_LOG"; for last; do :; done; exec sh -c "$last""#,
            ),
            (
                "mktemp",
                r#"printf 'installer-env %s\n' "$LG_ENROLL_TOKEN" >>"$LG_TEST_LOG"; exit 1"#,
            ),
        ] {
            let stub = dir.join(name);
            std::fs::write(&stub, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let log = dir.join("log");
        let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
        for shell in ["sh", "bash"] {
            for uid in ["0", "1000"] {
                let _ = std::fs::remove_file(&log);
                std::process::Command::new(shell)
                    .args(["-c", &cmd])
                    .env("PATH", &path)
                    .env("LG_TEST_UID", uid)
                    .env("LG_TEST_LOG", &log)
                    .env_remove(ENV_ENROLL_TOKEN)
                    .status()
                    .unwrap();
                let seen = std::fs::read_to_string(&log).unwrap();
                let case = format!("{shell} uid={uid}: {seen}");
                let argv: Vec<&str> = seen
                    .lines()
                    .filter(|l| !l.starts_with("installer-env "))
                    .collect();
                assert!(argv.iter().any(|l| l.starts_with("env -i ")), "{case}");
                assert_eq!(
                    uid == "1000",
                    argv.iter().any(|l| l.starts_with("sudo env -i ")),
                    "{case}"
                );
                assert!(
                    argv.iter().all(|l| !l.contains(token)),
                    "token in argv: {case}"
                );
                assert!(seen.contains(&format!("installer-env {token}\n")), "{case}");
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_command_for_test_rejects_production_env_overrides() {
        let params = EnrollmentParams {
            central_url: "https://central.example:8443".to_string(),
            tunnel_url: "https://tunnel.central.example:8443".to_string(),
            fingerprint: fingerprint(b"central-identity"),
            token: "deadbeef".to_string(),
            agent_url: "https://downloads.example/lg-agent".to_string(),
            agent_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
            install_script_url: "https://downloads.example/install-agent.sh".to_string(),
            install_script_sha256:
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string(),
        };

        let allowed = params.install_command_for_test(&[
            ("LG_INSTALL_DRY_RUN", "1"),
            ("LG_INSTALL_DRY_RUN_STATE_DIR", "/tmp/lg-state"),
            ("LG_ENROLL_RESPONSE_FILE", "/tmp/enroll.json"),
        ]);
        assert!(allowed.contains("LG_INSTALL_DRY_RUN='1'"));
        assert!(
            allowed.contains("PATH='/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin'"),
            "test command must preserve the production root PATH: {allowed}"
        );
        assert!(
            allowed.contains("LG_INSTALL_DRY_RUN_STATE_DIR='/tmp/lg-state'"),
            "test command may inject only the dry-run state directory override: {allowed}"
        );
        assert!(
            !allowed.contains("/tmp/fake-bin"),
            "test command must not smuggle a separate PATH override: {allowed}"
        );

        for blocked in [
            "PATH",
            "LG_AGENT_STATE_DIR",
            ENV_AGENT_URL,
            ENV_AGENT_SHA256,
            ENV_CENTRAL_URL,
            ENV_TUNNEL_URL,
            ENV_CENTRAL_FINGERPRINT,
            ENV_ENROLL_TOKEN,
            ENV_AGENT_INSTALL_SCRIPT_URL,
            ENV_AGENT_INSTALL_SCRIPT_SHA256,
        ] {
            let attempt = std::panic::catch_unwind(|| {
                params.install_command_for_test(&[(blocked, "override")]);
            });
            assert!(
                attempt.is_err(),
                "test install command must reject overriding {blocked}"
            );
        }
    }

    #[test]
    fn install_command_verifies_installer_before_bash_and_quotes_asset_urls() {
        let params = EnrollmentParams {
            central_url: "https://central.example:8443".to_string(),
            tunnel_url: "https://tunnel.central.example:8443".to_string(),
            fingerprint: fingerprint(b"central-identity"),
            token: "deadbeef".to_string(),
            agent_url: "https://downloads.example/lg-agent;touch$IFS/tmp/pwn".to_string(),
            agent_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
            install_script_url: "https://downloads.example/install-agent.sh;touch$IFS/tmp/pwn"
                .to_string(),
            install_script_sha256:
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string(),
        };

        let cmd = params.install_command();
        let verify = cmd.find("sha256sum -c - &&").unwrap();
        let run = cmd.find("bash \"$tmp\"").unwrap();
        assert!(
            verify < run,
            "installer must be verified before it runs: {cmd}"
        );
        assert!(
            cmd.contains("'https://downloads.example/install-agent.sh;touch$IFS/tmp/pwn'"),
            "installer URL must be shell-quoted: {cmd}"
        );
        assert!(
            cmd.contains("'https://downloads.example/lg-agent;touch$IFS/tmp/pwn'"),
            "agent URL must be shell-quoted: {cmd}"
        );
    }

    #[test]
    fn install_command_shell_quotes_embedded_single_quotes() {
        let params = EnrollmentParams {
            central_url: "https://central.example:8443".to_string(),
            tunnel_url: "https://tunnel.central.example:8443".to_string(),
            fingerprint: "fp'$(touch /tmp/pwn)".to_string(),
            token: "dead'beef".to_string(),
            agent_url: "https://downloads.example/lg-agent'$(touch /tmp/pwn)".to_string(),
            agent_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
            install_script_url: "https://downloads.example/install-agent.sh'$(touch /tmp/pwn)"
                .to_string(),
            install_script_sha256:
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string(),
        };

        let cmd = params.install_command();
        assert!(
            cmd.contains("'https://downloads.example/lg-agent'\\''$(touch /tmp/pwn)'"),
            "embedded single quote in agent URL must be escaped: {cmd}"
        );
        assert!(
            cmd.contains("'https://downloads.example/install-agent.sh'\\''$(touch /tmp/pwn)'"),
            "embedded single quote in installer URL must be escaped: {cmd}"
        );
        assert!(
            cmd.contains("'fp'\\''$(touch /tmp/pwn)'"),
            "embedded single quote in fingerprint must be escaped: {cmd}"
        );
        assert!(
            cmd.contains("'dead'\\''beef'"),
            "embedded single quote in token must be escaped: {cmd}"
        );
    }

    // ----- Tunnel session (Slice 8) --------------------------------------

    const CRED: &str = "b1946ac92492d2347c6235b4d2611184b1946ac92492d2347c6235b4d2611184";

    /// Build a raw wire frame the way [`AuthChannel::send`] does, so a test can
    /// hand-inject frames to exercise the receiver's per-frame checks.
    fn raw_frame(
        key: &[u8; TUNNEL_KEY_BYTES],
        direction: u8,
        counter: u64,
        payload: &[u8],
    ) -> Vec<u8> {
        let tag = frame_tag(key, direction, counter, payload);
        let mut frame = Vec::new();
        frame.extend_from_slice(&counter.to_be_bytes());
        frame.extend_from_slice(&tag);
        frame.extend_from_slice(payload);
        frame
    }

    /// A receiver-only channel in central's role: it accepts frames minted in the
    /// agent→central direction ([`DIR_AGENT_TO_CENTRAL`]).
    fn central_receiver(
        transport: ChannelTransport,
        key: [u8; TUNNEL_KEY_BYTES],
    ) -> AuthChannel<ChannelTransport> {
        AuthChannel::new(transport, key, DIR_CENTRAL_TO_AGENT, DIR_AGENT_TO_CENTRAL)
    }

    #[test]
    fn verify_pinned_identity_accepts_the_pin_and_rejects_a_mismatch() {
        let material = b"central-tls-cert-der";
        let pinned = fingerprint(material);
        assert!(verify_pinned_identity(material, &pinned).is_ok());
        assert!(matches!(
            verify_pinned_identity(b"an-imposter-cert", &pinned),
            Err(TunnelError::IdentityMismatch)
        ));
    }

    // Material that is not a certificate has no public key to pin, so it
    // keeps its whole-material fingerprint (and still never matches a real peer).
    #[test]
    fn non_certificate_material_pins_by_its_fingerprint() {
        assert_eq!(
            identity_pin(b"not-a-certificate"),
            fingerprint(b"not-a-certificate")
        );
    }

    #[test]
    fn session_key_is_deterministic_and_credential_bound() {
        let transcript = handshake_transcript("agent-1", &[1u8; 32], &[2u8; 32]);
        let a = derive_session_key(CRED.as_bytes(), &transcript);
        let b = derive_session_key(CRED.as_bytes(), &transcript);
        assert_eq!(a, b, "same inputs derive the same key");
        let other = derive_session_key(b"a-different-credential", &transcript);
        assert_ne!(a, other, "a different credential derives a different key");
    }

    // The transcript binds both nonces and the agent id: changing any one of
    // them derives a different session key, so a frame from one session never
    // verifies in another.
    #[test]
    fn session_key_is_bound_to_both_nonces_and_the_agent_id() {
        let key = |id: &str, client: u8, server: u8| {
            derive_session_key(
                CRED.as_bytes(),
                &handshake_transcript(id, &[client; 32], &[server; 32]),
            )
        };
        let base = key("agent-1", 1, 2);
        assert_ne!(base, key("agent-2", 1, 2), "agent id not bound");
        assert_ne!(base, key("agent-1", 3, 2), "client nonce not bound");
        assert_ne!(base, key("agent-1", 1, 3), "server nonce not bound");
    }

    // A frame's counter is under its tag: a genuinely tagged frame whose
    // counter header is rewritten fails the tag check, not just the order check.
    #[tokio::test]
    async fn a_rewritten_counter_breaks_a_genuine_frame_tag() {
        let key = [6u8; TUNNEL_KEY_BYTES];
        let (agent_side, mut raw) = ChannelTransport::pair();
        let mut agent =
            AuthChannel::new(agent_side, key, DIR_AGENT_TO_CENTRAL, DIR_CENTRAL_TO_AGENT);
        agent.send(b"first").await.unwrap();
        agent.send(b"second").await.unwrap();
        let _first = raw.recv().await.unwrap().unwrap();
        let mut second = raw.recv().await.unwrap().unwrap();
        second[..8].copy_from_slice(&0u64.to_be_bytes());

        let (mut wire, receiver) = ChannelTransport::pair();
        let mut central = central_receiver(receiver, key);
        wire.send(second).await.unwrap();
        assert!(matches!(central.recv().await, Err(TunnelError::BadTag)));
    }

    // A hello for another protocol version is refused before its credential is
    // checked or a nonce is sent.
    #[tokio::test]
    async fn server_handshake_refuses_another_protocol_version() {
        let (mut agent_side, central_side) = ChannelTransport::pair();
        let hello = TunnelHello {
            protocol_version: PROTOCOL_VERSION + 1,
            agent_id: "agent-1".to_string(),
            credential: CRED.to_string(),
            client_nonce: [7u8; 32],
            accepts_cancel: true,
            accepts_data_plane: true,
        };
        agent_side
            .send(serde_json::to_vec(&hello).unwrap())
            .await
            .unwrap();
        let result = server_handshake(central_side, [9u8; 32], |_, _| async {
            panic!("a wrong-version hello must not reach credential verification")
        })
        .await;
        assert!(matches!(
            result,
            Err(TunnelError::ProtocolMismatch { got }) if got == PROTOCOL_VERSION + 1
        ));
    }

    #[tokio::test]
    async fn handshake_derives_a_shared_key_and_authenticates_both_directions() {
        let (agent_side, central_side) = ChannelTransport::pair();

        let client =
            tokio::spawn(
                async move { client_handshake(agent_side, "agent-1", CRED, [7u8; 32]).await },
            );
        let server = tokio::spawn(async move {
            server_handshake(central_side, [9u8; 32], |id, cred| async move {
                id == "agent-1" && cred == CRED
            })
            .await
        });

        let mut agent_ch = client.await.unwrap().expect("client handshake");
        let (who, mut central_ch) = server.await.unwrap().expect("server handshake");
        assert_eq!(who, "agent-1");

        // Down: central → agent.
        let cmd = TunnelMessage::Command {
            run_id: "r1".into(),
            method: "ping".into(),
            target: "8.8.8.8".into(),
            limits: None,
        };
        central_ch.send_message(&cmd).await.unwrap();
        assert_eq!(agent_ch.recv_message().await.unwrap(), cmd);

        // Up: agent → central.
        let out = TunnelMessage::Output {
            run_id: "r1".into(),
            line: "64 bytes from 8.8.8.8".into(),
        };
        agent_ch.send_message(&out).await.unwrap();
        assert_eq!(central_ch.recv_message().await.unwrap(), out);
    }

    // Compatibility: a certificate report names the origin it was obtained
    // for as one more optional field. A central that predates it still decodes
    // the report (the field is ignored), and a report from an agent that
    // predates it still decodes here, with no origin.
    #[test]
    fn a_certificate_report_carries_its_origin_and_either_side_may_be_older() {
        /// The message as a central built before the report's `origin` decodes it.
        #[derive(Debug, PartialEq, Deserialize)]
        enum OlderCentral {
            Certificate(CertificateStatus),
        }
        let status = CertificateStatus {
            issued_at: Some(1),
            expires_at: Some(2),
            last_error: None,
        };
        let report = TunnelMessage::Certificate(CertificateReport {
            status: status.clone(),
            origin: Some("https://node.example.test".into()),
        });
        let wire = serde_json::to_value(&report).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"Certificate": {
                "issued_at": 1, "expires_at": 2, "last_error": null,
                "origin": "https://node.example.test"
            }})
        );
        assert_eq!(
            serde_json::from_value::<TunnelMessage>(wire.clone()).unwrap(),
            report
        );
        assert_eq!(
            serde_json::from_value::<OlderCentral>(wire).unwrap(),
            OlderCentral::Certificate(status.clone())
        );
        assert_eq!(
            serde_json::from_value::<TunnelMessage>(serde_json::json!(
                {"Certificate": {"issued_at": 1, "expires_at": 2, "last_error": null}}
            ))
            .unwrap(),
            TunnelMessage::Certificate(CertificateReport {
                status,
                origin: None
            })
        );
    }

    // Compatibility (C-087): a relayed run carries central's saved limits as one
    // optional field. A frame from an older central (no field) still decodes,
    // with no limits, and an older agent still decodes a frame that has them.
    #[test]
    fn a_command_carries_optional_limits_and_either_side_may_be_older() {
        let command = TunnelMessage::Command {
            run_id: "r1".into(),
            method: "ping".into(),
            target: "8.8.8.8".into(),
            limits: Some(RunLimits {
                timeout_secs: 5,
                max_output_bytes: 1024,
            }),
        };
        let wire = serde_json::to_vec(&command).unwrap();
        assert_eq!(
            serde_json::from_slice::<TunnelMessage>(&wire).unwrap(),
            command
        );
        assert!(serde_json::from_slice::<LegacyTunnelMessage>(&wire).is_ok());
        assert_eq!(
            serde_json::from_value::<TunnelMessage>(serde_json::json!(
                {"Command": {"run_id": "r1", "method": "ping", "target": "8.8.8.8"}}
            ))
            .unwrap(),
            TunnelMessage::Command {
                run_id: "r1".into(),
                method: "ping".into(),
                target: "8.8.8.8".into(),
                limits: None,
            }
        );
    }

    /// The message enum as an agent built before `Cancel` decodes it.
    #[derive(Deserialize)]
    #[allow(dead_code)]
    enum LegacyTunnelMessage {
        Command {
            run_id: String,
            method: String,
            target: String,
        },
        Output {
            run_id: String,
            line: String,
        },
        Done {
            run_id: String,
            ok: bool,
        },
        Error {
            run_id: String,
            message: String,
        },
        Heartbeat,
    }

    // Compatibility: an older agent cannot decode `Cancel` (its relay loop would
    // fail and drop the tunnel), so the frame is negotiated: an older agent's
    // hello lacks `accepts_cancel` and central sees `false`; the current agent
    // opts in. Every frame an older agent can be sent still decodes for it.
    #[tokio::test]
    async fn cancel_is_negotiated_because_an_older_agent_cannot_decode_it() {
        let cancel = serde_json::to_vec(&TunnelMessage::Cancel {
            run_id: "r1".into(),
        })
        .unwrap();
        assert!(serde_json::from_slice::<LegacyTunnelMessage>(&cancel).is_err());
        let command = serde_json::to_vec(&TunnelMessage::Command {
            run_id: "r1".into(),
            method: "ping".into(),
            target: "8.8.8.8".into(),
            limits: None,
        })
        .unwrap();
        assert!(serde_json::from_slice::<LegacyTunnelMessage>(&command).is_ok());

        let (mut legacy_agent, central_side) = ChannelTransport::pair();
        let legacy_hello = serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "agent_id": "agent-1",
            "credential": CRED,
            "client_nonce": vec![7u8; 32],
        });
        legacy_agent
            .send(serde_json::to_vec(&legacy_hello).unwrap())
            .await
            .unwrap();
        let (_, legacy_channel) = server_handshake(central_side, [9u8; 32], |_, _| async { true })
            .await
            .expect("an older agent still connects");
        assert!(!legacy_channel.peer_accepts_cancel());

        let (agent_side, central_side) = ChannelTransport::pair();
        let client =
            tokio::spawn(
                async move { client_handshake(agent_side, "agent-1", CRED, [7u8; 32]).await },
            );
        let (_, channel) = server_handshake(central_side, [9u8; 32], |_, _| async { true })
            .await
            .unwrap();
        client.await.unwrap().unwrap();
        assert!(channel.peer_accepts_cancel());
    }

    // Compatibility: an older agent cannot decode `DataPlane` and an older
    // central cannot decode `Certificate`, so both are negotiated like `Cancel`:
    // an older agent's hello leaves `accepts_data_plane` false, the current agent
    // opts in, and the frames keep their wire shape.
    #[tokio::test]
    async fn data_plane_is_negotiated_because_older_peers_cannot_decode_it() {
        for frame in [
            TunnelMessage::DataPlane {
                origin: "https://node.example.test".into(),
            },
            TunnelMessage::Certificate(CertificateReport::default()),
        ] {
            let bytes = serde_json::to_vec(&frame).unwrap();
            assert!(serde_json::from_slice::<LegacyTunnelMessage>(&bytes).is_err());
            assert_eq!(frame.run_id(), None, "a run-less frame");
        }
        assert_eq!(
            serde_json::to_value(TunnelMessage::Certificate(CertificateReport {
                status: CertificateStatus {
                    issued_at: Some(1),
                    expires_at: Some(2),
                    last_error: None,
                },
                origin: None,
            }))
            .unwrap(),
            serde_json::json!({"Certificate": {"issued_at": 1, "expires_at": 2, "last_error": null}})
        );

        let (mut legacy_agent, central_side) = ChannelTransport::pair();
        let legacy_hello = serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "agent_id": "agent-1",
            "credential": CRED,
            "client_nonce": vec![7u8; 32],
            "accepts_cancel": true,
        });
        legacy_agent
            .send(serde_json::to_vec(&legacy_hello).unwrap())
            .await
            .unwrap();
        let (_, legacy_channel) = server_handshake(central_side, [9u8; 32], |_, _| async { true })
            .await
            .expect("an older agent still connects");
        assert!(!legacy_channel.peer_accepts_data_plane());

        let (agent_side, central_side) = ChannelTransport::pair();
        let client =
            tokio::spawn(
                async move { client_handshake(agent_side, "agent-1", CRED, [7u8; 32]).await },
            );
        let (_, channel) = server_handshake(central_side, [9u8; 32], |_, _| async { true })
            .await
            .unwrap();
        client.await.unwrap().unwrap();
        assert!(
            channel.peer_accepts_data_plane(),
            "the current agent must announce that it takes a data-plane origin"
        );
    }

    // Compatibility, newer agent -> older central: a Done that carries a status
    // still decodes where `Done` is only `{run_id, ok}`, with `ok` unchanged.
    #[test]
    fn an_older_peer_decodes_a_done_that_carries_a_status() {
        for (status, ok) in [
            (ExecStatus::Completed { success: true }, true),
            (ExecStatus::Completed { success: false }, false),
            (ExecStatus::TimedOut, false),
        ] {
            let done = serde_json::to_vec(&TunnelMessage::done("r1", status)).unwrap();
            let legacy = serde_json::from_slice::<LegacyTunnelMessage>(&done).unwrap();
            assert!(
                matches!(legacy, LegacyTunnelMessage::Done { ok: got, .. } if got == ok),
                "{status:?}"
            );
        }
    }

    // Compatibility, older agent -> newer central: a Done without a status
    // decodes and reports no status, so central falls back to `ok`.
    #[test]
    fn a_done_from_an_older_agent_has_no_status() {
        let done: TunnelMessage =
            serde_json::from_str(r#"{"Done":{"run_id":"r1","ok":false}}"#).unwrap();
        assert_eq!(
            done,
            TunnelMessage::Done {
                run_id: "r1".into(),
                ok: false,
                status: None
            }
        );
        assert_eq!(done.terminal_status(), None);
    }

    #[test]
    fn every_exec_status_survives_the_done_frame() {
        for status in [
            ExecStatus::Completed { success: true },
            ExecStatus::Completed { success: false },
            ExecStatus::TimedOut,
            ExecStatus::OutputCapped,
            ExecStatus::Canceled,
            ExecStatus::Failed,
        ] {
            let wire = serde_json::to_vec(&TunnelMessage::done("r1", status)).unwrap();
            let done: TunnelMessage = serde_json::from_slice(&wire).unwrap();
            assert_eq!(done.terminal_status(), Some(status));
        }
        let unknown: TunnelMessage =
            serde_json::from_str(r#"{"Done":{"run_id":"r1","ok":false,"status":"later"}}"#)
                .unwrap();
        assert_eq!(
            unknown.terminal_status(),
            None,
            "an unknown label is not guessed"
        );
    }

    // An Error carries how the failed run ended; an older central still decodes
    // it with the message intact, and an older agent's Error has no status.
    #[test]
    fn an_error_frame_carries_its_status_across_versions() {
        for status in [ExecStatus::OutputCapped, ExecStatus::Failed] {
            let wire = serde_json::to_vec(&TunnelMessage::error("r1", "m".into(), status)).unwrap();
            let error: TunnelMessage = serde_json::from_slice(&wire).unwrap();
            assert_eq!(error.terminal_status(), Some(status));
            let legacy = serde_json::from_slice::<LegacyTunnelMessage>(&wire).unwrap();
            assert!(
                matches!(legacy, LegacyTunnelMessage::Error { ref message, .. } if message == "m"),
                "{status:?}"
            );
        }
        let old: TunnelMessage =
            serde_json::from_str(r#"{"Error":{"run_id":"r1","message":"m"}}"#).unwrap();
        assert_eq!(old.terminal_status(), None);
    }

    // Central and the agent share one wire name per method.
    #[test]
    fn method_wire_names_round_trip() {
        for method in [
            Method::Ping,
            Method::Ping6,
            Method::Mtr,
            Method::Mtr6,
            Method::Traceroute,
            Method::Traceroute6,
        ] {
            assert_eq!(Method::from_wire(method.wire()), Some(method));
        }
        assert_eq!(Method::from_wire("bgp"), None);
    }

    #[tokio::test]
    async fn server_handshake_refuses_a_wrong_credential_and_yields_no_channel() {
        let (agent_side, central_side) = ChannelTransport::pair();
        let client =
            tokio::spawn(
                async move { client_handshake(agent_side, "agent-1", CRED, [7u8; 32]).await },
            );
        // Verifier rejects (models a missing/revoked at-rest hash → revoke).
        let result = server_handshake(central_side, [9u8; 32], |_, _| async { false }).await;
        assert!(matches!(result, Err(TunnelError::AuthRejected)));
        // The agent never receives an accept, so its handshake fails closed too.
        assert!(client.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn a_forged_tag_frame_after_a_valid_one_is_refused_per_frame() {
        let key = [3u8; TUNNEL_KEY_BYTES];
        let (mut raw, receiver) = ChannelTransport::pair();
        let mut channel = central_receiver(receiver, key);

        // First frame is genuine and accepted — the channel is live/authenticated.
        raw.send(raw_frame(&key, DIR_AGENT_TO_CENTRAL, 0, b"legit"))
            .await
            .unwrap();
        assert_eq!(channel.recv().await.unwrap(), b"legit");

        // Next frame carries the correct next counter but a corrupted tag: even on
        // an established channel a single bad frame is refused (auth is per-frame).
        let mut forged = raw_frame(&key, DIR_AGENT_TO_CENTRAL, 1, b"attacker payload");
        forged[8] ^= 0xff; // flip a tag byte
        raw.send(forged).await.unwrap();
        assert!(matches!(channel.recv().await, Err(TunnelError::BadTag)));
    }

    #[tokio::test]
    async fn a_replayed_or_gapped_counter_is_refused() {
        let key = [4u8; TUNNEL_KEY_BYTES];

        // Replay: the exact first frame, sent twice.
        let (mut raw, receiver) = ChannelTransport::pair();
        let mut channel = central_receiver(receiver, key);
        let frame0 = raw_frame(&key, DIR_AGENT_TO_CENTRAL, 0, b"first");
        raw.send(frame0.clone()).await.unwrap();
        assert_eq!(channel.recv().await.unwrap(), b"first");
        raw.send(frame0).await.unwrap();
        assert!(matches!(
            channel.recv().await,
            Err(TunnelError::ReplayOrGap {
                expected: 1,
                got: 0
            })
        ));

        // Gap: a valid tag but a skipped counter (2 while expecting 1).
        let (mut raw, receiver) = ChannelTransport::pair();
        let mut channel = central_receiver(receiver, key);
        raw.send(raw_frame(&key, DIR_AGENT_TO_CENTRAL, 0, b"a"))
            .await
            .unwrap();
        assert_eq!(channel.recv().await.unwrap(), b"a");
        raw.send(raw_frame(&key, DIR_AGENT_TO_CENTRAL, 2, b"skips one"))
            .await
            .unwrap();
        assert!(matches!(
            channel.recv().await,
            Err(TunnelError::ReplayOrGap {
                expected: 1,
                got: 2
            })
        ));
    }

    // Finding 2 (per-direction domain separation): a frame minted in central's own
    // send direction (central → agent) must be REJECTED when reflected into
    // central's recv (which expects agent → central), even at the aligned counter.
    // Without direction binding this reflected frame would verify and be accepted.
    #[tokio::test]
    async fn a_reflected_same_direction_frame_is_refused() {
        let key = [5u8; TUNNEL_KEY_BYTES];
        let (mut raw, receiver) = ChannelTransport::pair();
        let mut central = central_receiver(receiver, key);

        // A frame central itself would SEND (central → agent), reflected back at it.
        raw.send(raw_frame(&key, DIR_CENTRAL_TO_AGENT, 0, b"reflected"))
            .await
            .unwrap();
        assert!(
            matches!(central.recv().await, Err(TunnelError::BadTag)),
            "a same-direction (reflected) frame must not verify"
        );

        // Sanity: a properly-directed agent → central frame at counter 0 is accepted.
        let (mut raw, receiver) = ChannelTransport::pair();
        let mut central = central_receiver(receiver, key);
        raw.send(raw_frame(&key, DIR_AGENT_TO_CENTRAL, 0, b"proper"))
            .await
            .unwrap();
        assert_eq!(central.recv().await.unwrap(), b"proper");
    }
}
