//! The agent↔central tunnel, agent side (Slice 8) — the RCE surface, node side.
//!
//! The agent holds an **outbound** WebSocket-over-TLS connection to central's
//! direct tunnel listener; there is no inbound command port on the node (FR-025).
//! On every (re)connect it verifies central's TLS identity against the SHA-256
//! fingerprint pinned from the install command (Slice 7) and aborts on a
//! mismatch — no trust-on-first-use, fail closed (FR-070/AC35). It then proves its
//! credential once, derives the shared session key, and rides an
//! [`AuthChannel`] so every relayed frame is authenticated (FR-024).
//!
//! A relayed command runs through the one audited [`shared::exec`] engine —
//! re-validated against [`shared::validate`] (the SSRF boundary) and built as an
//! argv template ([`shared::template`]), never a second executor — and honours the
//! same global concurrency cap (AC40). Output streams back up the authenticated
//! channel.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use shared::exec::{ExecEngine, ExecEvent, ExecHandle, ExecLimits, ExecStatus, StartError};
use shared::liveness::{HEARTBEAT_INTERVAL, OFFLINE_AFTER};
use shared::protocol::{
    client_handshake, identity_pin, verify_pinned_identity, AuthChannel, CertificateReport,
    CertificateStatus, FrameTransport, RunLimits, TunnelError, TunnelMessage, TUNNEL_KEY_BYTES,
};
use shared::template::{DaemonProbe, Method, ScopedDaemonProbe};
use shared::validate::{bgp_arg, validate_target, HostResolver, PrefixFamily};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// Backoff between reconnect attempts — bounded so a flapping central does not
/// become a tight loop, short enough that a recovered agent rejoins promptly.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(5);

/// How long the agent waits on central before it gives up on a connection and
/// redials after [`RECONNECT_BACKOFF`].
#[derive(Clone, Copy)]
struct Deadlines {
    /// The whole dial: TCP, TLS, the WebSocket upgrade and the credential
    /// handshake. Central allows each of its three stages 10 s.
    connect: Duration,
    /// An established tunnel that hears nothing from central for this long is
    /// dead. The agent pings a quiet tunnel, so a live central always answers.
    /// Twice central's offline window, so a central that is merely slow to read
    /// is not cut, and a black-holed path is noticed in about a minute.
    silence: Duration,
}

const DEADLINES: Deadlines = Deadlines {
    connect: Duration::from_secs(30),
    silence: Duration::from_secs(2 * OFFLINE_AFTER.as_secs()),
};
/// Where the service keeps its credential, as `main.rs` and the systemd unit set it.
const ENV_CREDENTIAL_PATH: &str = "LG_AGENT_CREDENTIAL";
const DEFAULT_CREDENTIAL_PATH: &str = "data/agent-credential.json";

/// Everything the outbound tunnel needs: where the tunnel is, the fingerprint to pin
/// it by, and the credential to authenticate with.
#[derive(Debug, Clone)]
pub struct TunnelClientConfig {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub agent_id: String,
    pub credential: String,
}

impl TunnelClientConfig {
    /// Split an `https://host:port` tunnel URL into host + port with the same parse
    /// the startup check uses ([`crate::enroll::tunnel_host_port`]).
    ///
    /// # Panics
    ///
    /// If `tunnel_url` did not pass [`crate::enroll::validate_stored_credential`],
    /// which runs before the tunnel starts.
    pub fn from_parts(
        tunnel_url: &str,
        fingerprint: String,
        agent_id: String,
        credential: String,
    ) -> Self {
        let (host, port) = crate::enroll::tunnel_host_port(tunnel_url)
            .expect("the tunnel URL is validated before the tunnel starts");
        Self {
            host,
            port,
            fingerprint,
            agent_id,
            credential,
        }
    }
}

/// Why a relayed command could not be started. Each maps to a clear terminal
/// error the visitor sees (AC41) — never a silent drop.
#[derive(Debug)]
pub enum RelayReject {
    /// The node's global concurrency cap is saturated (AC40).
    Busy,
    /// The target/prefix failed re-validation, the tool is absent, or (for BGP) no
    /// supported routing daemon is present on this node.
    Rejected(String),
    /// Central asked for a method name this node does not recognise.
    UnknownMethod(String),
}

impl RelayReject {
    fn message(&self) -> String {
        match self {
            RelayReject::Busy => "the node is at capacity; try again shortly".to_string(),
            RelayReject::Rejected(reason) => reason.clone(),
            RelayReject::UnknownMethod(method) => {
                format!("this node cannot run the requested method: {method}")
            }
        }
    }
}

/// Turns a relayed command into a running process on this node. The production
/// [`NodeExecutor`] re-validates the target and runs it through [`shared::exec`];
/// the seam keeps the relay loop provable without spawning real network tools.
pub trait CommandExecutor {
    fn spawn(
        &self,
        method: &str,
        target: &str,
        limits: Option<RunLimits>,
    ) -> impl Future<Output = Result<ExecHandle, RelayReject>> + Send;
}

/// The production executor: for a diagnostic, re-validate the target (SSRF
/// boundary) and build the argv (no shell); for BGP, re-validate the prefix with
/// the family-locked grammar and detect this node's routing daemon. Both run
/// through the one shared exec engine (honouring the global cap). One audited
/// execution path, shared with the local node.
pub struct NodeExecutor<R: HostResolver> {
    engine: ExecEngine,
    resolver: R,
    probe: Arc<dyn DaemonProbe>,
}

impl<R: HostResolver> NodeExecutor<R> {
    pub fn new(engine: ExecEngine, resolver: R) -> Self {
        Self {
            engine,
            resolver,
            // The agent resolves BGP ONLY through its scoped wrapper directory, never
            // the full PATH: on a real router the unscoped system birdc/vtysh is on
            // PATH, so a full-PATH probe would reach the unscoped daemon. Absent a
            // scoped wrapper, this fails closed with the existing clear refusal.
            probe: Arc::new(ScopedDaemonProbe::from_env()),
        }
    }

    /// Override the routing-daemon probe — used by tests to simulate a BGP daemon
    /// present or absent without a live BIRD/FRR install.
    pub fn with_daemon_probe(mut self, probe: Arc<dyn DaemonProbe>) -> Self {
        self.probe = probe;
        self
    }
}

impl<R: HostResolver + Sync> CommandExecutor for NodeExecutor<R> {
    async fn spawn(
        &self,
        method: &str,
        target: &str,
        limits: Option<RunLimits>,
    ) -> Result<ExecHandle, RelayReject> {
        // Bound this run by central's saved limits, or by this node's defaults
        // for a frame without them (an older central). Relayed runs start one
        // at a time, so setting the engine's run bounds here cannot race.
        let limits = limits.map_or_else(ExecLimits::default, |limits| ExecLimits {
            timeout: Duration::from_secs(limits.timeout_secs),
            max_output_bytes: limits.max_output_bytes,
            ..ExecLimits::default()
        });
        self.engine
            .set_run_limits(limits.timeout, limits.max_output_bytes);
        // A diagnostic: re-validate the target through the SSRF boundary, pin the IP.
        if let Some(method) = Method::from_wire(method) {
            let validated = validate_target(target, method.family(), &self.resolver)
                .await
                .map_err(|reason| RelayReject::Rejected(reason.to_string()))?;
            return self
                .engine
                .try_start(method.command(&validated), Some(validated.ip()))
                .map_err(map_start_error);
        }
        // BGP: re-validate the prefix (family-locked IP/CIDR grammar, no SSRF filter)
        // and shell to this node's read-only daemon CLI. No pinned IP — BGP inspects
        // the local RIB and never connects.
        if let Some(family) = PrefixFamily::from_wire(method) {
            let prefix = bgp_arg(target, family)
                .map_err(|reason| RelayReject::Rejected(reason.to_string()))?;
            let daemon = self.probe.detect().ok_or_else(|| {
                RelayReject::Rejected(
                    "BGP is not available on this node — no supported routing daemon is present"
                        .to_string(),
                )
            })?;
            return self
                .engine
                .try_start(daemon.command(&prefix), None)
                .map_err(map_start_error);
        }
        Err(RelayReject::UnknownMethod(method.to_string()))
    }
}

fn map_start_error(error: StartError) -> RelayReject {
    match error {
        StartError::Busy => RelayReject::Busy,
        StartError::Rejected(reason) => RelayReject::Rejected(reason),
    }
}

/// The tunnel's side of the speed-test data plane: where the origin
/// central assigns goes, and the certificate status to report back.
pub struct DataPlaneLink {
    pub origin: watch::Sender<Option<String>>,
    pub status: watch::Receiver<Option<CertificateStatus>>,
    /// Voids the status when the origin changes: it was for the old one.
    void: watch::Sender<Option<CertificateStatus>>,
}

impl DataPlaneLink {
    /// A link plus the data plane's ends of it: the assigned origin to serve,
    /// and the certificate status to publish.
    pub fn new() -> (
        Self,
        watch::Receiver<Option<String>>,
        watch::Sender<Option<CertificateStatus>>,
    ) {
        let (origin, origin_rx) = watch::channel(None);
        let (status_tx, status) = watch::channel(None);
        let void = status_tx.clone();
        (
            Self {
                origin,
                status,
                void,
            },
            origin_rx,
            status_tx,
        )
    }
}

/// Serve relayed commands over an authenticated channel until it tears down. Each
/// `Command` runs locally and its output streams back as `Output`/`Done`/`Error`.
/// A channel error (bad tag, replay, close) propagates so the caller reconnects —
/// never continue past an auth failure.
///
/// A `DataPlane` frame hands the assigned origin to the data plane; from then
/// on (only then: an older central cannot decode it) the certificate status
/// goes up now and on every change.
pub async fn serve_relay<T, E>(
    channel: &mut AuthChannel<T>,
    executor: &E,
    data_plane: &DataPlaneLink,
) -> Result<(), TunnelError>
where
    T: FrameTransport,
    E: CommandExecutor,
{
    // Beat every HEARTBEAT_INTERVAL so central derives this node online (Slice 8b),
    // during a relayed run too. The first tick fires immediately; skip it (the
    // handshake already proved us live), so the first beat lands one interval in.
    // Delay missed-tick behaviour so a busy moment doesn't burst catch-up beats.
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut status = data_plane.status.clone();
    let mut assigned = false;
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                channel.send_message(&TunnelMessage::Heartbeat).await?;
            }
            // Each connection's receiver starts unseen, so a status set before
            // the origin arrived goes up as soon as it is assigned. A status is
            // for the assigned origin: a new one voids it (below), and the data
            // plane drops a pass that outlived its origin (`acme::manage`).
            _ = status.changed(), if assigned => {
                let current = status.borrow_and_update().clone();
                if let Some(status) = current {
                    let origin = data_plane.origin.borrow().clone();
                    let report = CertificateReport { status, origin };
                    channel.send_message(&TunnelMessage::Certificate(report)).await?;
                }
            }
            message = channel.recv_message() => {
                match message? {
                    TunnelMessage::Command {
                        run_id,
                        method,
                        target,
                        limits,
                    } => match executor.spawn(&method, &target, limits).await {
                        Ok(handle) => stream_run(channel, &run_id, handle, &mut heartbeat).await?,
                        Err(reject) => {
                            channel
                                .send_message(&TunnelMessage::Error {
                                    run_id,
                                    message: reject.message(),
                                    status: None,
                                })
                                .await?
                        }
                    },
                    // An up-frame echoed back (or a stray heartbeat) is not ours to act
                    // on; ignore it and keep serving.
                    TunnelMessage::Heartbeat
                    | TunnelMessage::Output { .. }
                    | TunnelMessage::Done { .. }
                    | TunnelMessage::Error { .. }
                    | TunnelMessage::Cancel { .. }
                    | TunnelMessage::Certificate(_) => {}
                    TunnelMessage::DataPlane { origin } => {
                        // Under the status lock, so the data plane cannot publish
                        // between the change and the void.
                        data_plane.void.send_if_modified(|status| {
                            let mut replaced = false;
                            data_plane.origin.send_if_modified(|current| {
                                let changed = current.as_deref() != Some(origin.as_str());
                                replaced = changed && current.is_some();
                                *current = Some(origin);
                                changed
                            });
                            replaced && status.take().is_some()
                        });
                        assigned = true;
                    }
                }
            }
        }
    }
}

/// Drain one run's exec events into authenticated frames on the channel, ending
/// with exactly one terminal frame: `Error` or `Done`. Central may cancel this
/// run meanwhile (its visitor left): the process is killed and the run ends with
/// a canceled `Done`, keeping the tunnel for the next run. The heartbeat keeps
/// beating, so a run that prints nothing for a while does not look offline.
async fn stream_run<T: FrameTransport>(
    channel: &mut AuthChannel<T>,
    run_id: &str,
    mut handle: ExecHandle,
    heartbeat: &mut tokio::time::Interval,
) -> Result<(), TunnelError> {
    loop {
        let event = tokio::select! {
            event = handle.events.recv() => event,
            _ = heartbeat.tick() => {
                channel.send_message(&TunnelMessage::Heartbeat).await?;
                continue;
            }
            message = channel.recv_message() => {
                if matches!(&message?, TunnelMessage::Cancel { run_id: cancelled } if cancelled == run_id) {
                    // Dropping the handle kills the run's process group.
                    drop(handle);
                    return channel
                        .send_message(&TunnelMessage::done(run_id, ExecStatus::Canceled))
                        .await;
                }
                continue;
            }
        };
        let Some(event) = event else { break };
        match event {
            ExecEvent::Line(line) => {
                channel
                    .send_message(&TunnelMessage::Output {
                        run_id: run_id.to_string(),
                        line,
                    })
                    .await?;
            }
            ExecEvent::Failed(message) => {
                // The engine's own `Done` follows at once. The Error carries its
                // status and is the run's only terminal; waiting also returns the
                // run's exec permit before the next command.
                let mut status = ExecStatus::Failed;
                while let Some(event) = handle.events.recv().await {
                    if let ExecEvent::Done { status: ended, .. } = event {
                        status = ended;
                    }
                }
                channel
                    .send_message(&TunnelMessage::error(run_id, message, status))
                    .await?;
                break;
            }
            ExecEvent::Done { status, .. } => {
                channel
                    .send_message(&TunnelMessage::done(run_id, status))
                    .await?;
                break;
            }
        }
    }
    Ok(())
}

/// The reconnect loop: connect (pinning central every time), serve, and on any
/// disconnect wait a bounded backoff and reconnect. Never returns.
pub async fn run<E: CommandExecutor>(
    config: TunnelClientConfig,
    executor: E,
    data_plane: DataPlaneLink,
) {
    run_with(config, executor, data_plane, DEADLINES).await
}

async fn run_with<E: CommandExecutor>(
    mut config: TunnelClientConfig,
    executor: E,
    data_plane: DataPlaneLink,
    deadlines: Deadlines,
) {
    loop {
        let correlation_id = connection_correlation_id();
        match connect_once(
            &mut config,
            &executor,
            &data_plane,
            &correlation_id,
            deadlines,
        )
        .await
        {
            Ok(()) => log_agent_disconnect(&correlation_id, &config),
            Err(error) => tracing::warn!(
                event = "agent.connect",
                correlation_id = %correlation_id,
                agent_id = %config.agent_id,
                outcome = "failed",
                %error,
                "tunnel connection failed; reconnecting"
            ),
        }
        tokio::time::sleep(RECONNECT_BACKOFF).await;
    }
}

/// One connection attempt: TCP → TLS (central pinned by fingerprint) → WebSocket →
/// credential handshake → serve. The pin is enforced inside the TLS handshake, so
/// it runs on this connect and on every reconnect. A legacy whole-certificate pin
/// that got through moves to the public-key pin once authenticated.
async fn connect_once<E: CommandExecutor>(
    config: &mut TunnelClientConfig,
    executor: &E,
    data_plane: &DataPlaneLink,
    correlation_id: &str,
    deadlines: Deadlines,
) -> Result<(), TunnelError> {
    // The whole dial runs under one deadline: a peer that accepts TCP and then
    // stalls must not keep the agent from its reconnect backoff.
    let dial = async {
        let tcp = TcpStream::connect((config.host.as_str(), config.port))
            .await
            .map_err(TunnelError::Transport)?;

        let connector = TlsConnector::from(Arc::new(pinned_client_config(&config.fingerprint)?));
        let server_name = ServerName::try_from(config.host.clone())
            .map_err(|_| TunnelError::Transport(std::io::Error::other("invalid central host")))?;
        let tls = connector
            .connect(server_name, tcp)
            .await
            .map_err(TunnelError::Transport)?;
        let presented_pin = tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|certs| certs.first())
            .map(|cert| identity_pin(cert.as_ref()));

        let request = format!("wss://{}:{}/", config.host, config.port);
        let (ws, _response) = client_async(request, tls)
            .await
            .map_err(|error| TunnelError::Transport(std::io::Error::other(error.to_string())))?;

        let transport = WsTransport::new(ws, deadlines.silence);
        let channel = client_handshake(
            transport,
            &config.agent_id,
            &config.credential,
            random_nonce(),
        )
        .await?;
        Ok((channel, presented_pin))
    };
    let (mut channel, presented_pin) = tokio::time::timeout(deadlines.connect, dial)
        .await
        .map_err(|_| {
            TunnelError::Transport(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "central did not complete the handshake in time",
            ))
        })??;
    log_agent_connect(correlation_id, config);
    if let Some(pin) = presented_pin.filter(|pin| *pin != config.fingerprint) {
        adopt_public_key_pin(config, pin);
    }
    serve_relay(&mut channel, executor, data_plane).await
}

/// The verifier accepted central by the legacy whole-certificate pin: pin its key
/// from now on, in memory and in the stored credential, so the next renewal that
/// keeps the key keeps this agent connected. A failed rewrite is retried on the
/// next start; the legacy pin still verifies this certificate.
fn adopt_public_key_pin(config: &mut TunnelClientConfig, pin: String) {
    // ponytail: main.rs owns this lookup too; pass the path in when main.rs is next touched.
    let path = std::env::var(ENV_CREDENTIAL_PATH)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(DEFAULT_CREDENTIAL_PATH));
    match crate::enroll::repin_credential(&path, &config.fingerprint, &pin) {
        Ok(()) => tracing::info!(
            event = "agent.repin",
            agent_id = %config.agent_id,
            "stored central pin moved to its public key"
        ),
        Err(error) => tracing::warn!(
            event = "agent.repin",
            agent_id = %config.agent_id,
            %error,
            "could not store central's public-key pin; will retry on restart"
        ),
    }
    config.fingerprint = pin;
}

fn log_agent_connect(correlation_id: &str, config: &TunnelClientConfig) {
    tracing::info!(
        event = "agent.connect",
        correlation_id = %correlation_id,
        agent_id = %config.agent_id,
        outcome = "connected",
        "tunnel established (central identity verified, credential accepted)"
    );
}

fn log_agent_disconnect(correlation_id: &str, config: &TunnelClientConfig) {
    tracing::info!(
        event = "agent.disconnect",
        correlation_id = %correlation_id,
        agent_id = %config.agent_id,
        outcome = "central_closed",
        "tunnel closed by central; reconnecting"
    );
}

/// A rustls client config that verifies central's certificate **only** against
/// the pinned SHA-256 fingerprint, ignoring the web PKI. The fingerprint's origin
/// is the operator's install command (Slice 7), so this is a deliberate pin, not
/// an accept-any relaxation — the TLS signature is still verified against the
/// pinned certificate's key.
fn pinned_client_config(fingerprint: &str) -> Result<ClientConfig, TunnelError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let algorithms = provider.signature_verification_algorithms;
    let verifier = Arc::new(PinnedCentral {
        pinned_fingerprint: fingerprint.to_string(),
        captured_identity: None,
        algorithms,
    });
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| TunnelError::Transport(std::io::Error::other(format!("{error:?}"))))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(config)
}

/// The pinned-identity certificate verifier (FR-070/AC35). `verify_server_cert`
/// checks the presented end-entity certificate against the pinned fingerprint and
/// aborts on a mismatch; the signature methods still verify the handshake
/// signature against that certificate's key, so pinning does not weaken the TLS
/// proof of possession.
#[derive(Debug)]
pub(crate) struct PinnedCentral {
    pub(crate) pinned_fingerprint: String,
    /// Set by enrollment, which keeps the identity it verified.
    pub(crate) captured_identity: Option<Arc<Mutex<Option<Vec<u8>>>>>,
    pub(crate) algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedCentral {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        verify_pinned_identity(end_entity.as_ref(), &self.pinned_fingerprint).map_err(|_| {
            rustls::Error::General(
                "central identity does not match the pinned fingerprint".to_string(),
            )
        })?;
        if let Some(Ok(mut captured)) = self.captured_identity.as_ref().map(|c| c.lock()) {
            *captured = Some(end_entity.as_ref().to_vec());
        }
        Ok(ServerCertVerified::assertion())
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

fn random_nonce() -> [u8; TUNNEL_KEY_BYTES] {
    let mut nonce = [0u8; TUNNEL_KEY_BYTES];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut nonce)
        .expect("system CSPRNG must be available");
    nonce
}

fn connection_correlation_id() -> String {
    use std::fmt::Write as _;

    let nonce = random_nonce();
    let mut id = String::with_capacity(TUNNEL_KEY_BYTES * 2);
    for byte in nonce {
        write!(&mut id, "{byte:02x}").expect("write to string");
    }
    id
}

/// A [`FrameTransport`] over a WebSocket: one binary message per frame.
///
/// Hearing nothing from central for `silence` ends the stream with an error, so
/// a dead peer is noticed without waiting out TCP retransmission. While it waits,
/// the transport pings every quarter of that window; central's WebSocket answers
/// each ping, so a live but quiet central keeps the tunnel.
struct WsTransport<S> {
    ws: WebSocketStream<S>,
    silence: Duration,
    heard: tokio::time::Instant,
    pinged: tokio::time::Instant,
}

impl<S> WsTransport<S> {
    fn new(ws: WebSocketStream<S>, silence: Duration) -> Self {
        let now = tokio::time::Instant::now();
        Self {
            ws,
            silence,
            heard: now,
            pinged: now,
        }
    }
}

impl<S> FrameTransport for WsTransport<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn send(&mut self, frame: Vec<u8>) -> std::io::Result<()> {
        // A central that stopped reading fills the socket; give up rather than
        // wait out TCP retransmission with the read deadline never polled.
        tokio::time::timeout(self.silence, self.ws.send(Message::binary(frame)))
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "central stopped reading")
            })?
            .map_err(|error| std::io::Error::other(format!("{error}")))
    }

    async fn recv(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        loop {
            // Absolute instants: a caller's select may drop this future and call
            // again, and that must not restart the clock.
            let dead_at = self.heard + self.silence;
            let ping_at = self.pinged + self.silence / 4;
            let Ok(message) = tokio::time::timeout_at(dead_at.min(ping_at), self.ws.next()).await
            else {
                if tokio::time::Instant::now() >= dead_at {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "central went silent",
                    ));
                }
                self.pinged = tokio::time::Instant::now();
                self.ws
                    .send(Message::Ping(Default::default()))
                    .await
                    .map_err(|error| std::io::Error::other(format!("{error}")))?;
                continue;
            };
            let Some(message) = message else {
                return Ok(None);
            };
            self.heard = tokio::time::Instant::now();
            match message.map_err(|error| std::io::Error::other(format!("{error}")))? {
                Message::Binary(bytes) => return Ok(Some(bytes.to_vec())),
                Message::Close(_) => return Ok(None),
                _ => continue,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::protocol::{server_handshake, ChannelTransport};
    use shared::template::CommandTemplate;
    use shared::validate::{HostResolver, ResolveError};
    use std::net::IpAddr;
    use std::sync::{Arc as StdArc, Mutex, OnceLock};
    use std::time::Duration;

    const CRED: &str = "aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44";

    /// Resolves nothing — every test target is an IP literal, so the resolver is
    /// never consulted (validate rejects/accepts the literal directly).
    struct StubResolver;
    impl HostResolver for StubResolver {
        async fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, ResolveError> {
            Ok(vec![])
        }
    }

    /// A test executor that runs a fixed, deterministic command through the *real*
    /// shared exec engine — proving the relay streams real exec output back,
    /// without depending on a network tool being installed.
    struct EchoExecutor {
        engine: ExecEngine,
    }
    impl CommandExecutor for EchoExecutor {
        async fn spawn(
            &self,
            _method: &str,
            _target: &str,
            _limits: Option<RunLimits>,
        ) -> Result<ExecHandle, RelayReject> {
            self.engine
                .try_start(
                    CommandTemplate {
                        program: "sh",
                        args: vec!["-c".into(), "printf 'relayed-1\\nrelayed-2\\n'".into()],
                    },
                    None,
                )
                .map_err(|_| RelayReject::Busy)
        }
    }

    /// Runs a long-lived, continuously-emitting command through the *real* exec
    /// engine, so the relayed run holds an exec permit until it is reaped.
    struct LoopExecutor {
        engine: ExecEngine,
    }
    impl CommandExecutor for LoopExecutor {
        async fn spawn(
            &self,
            _method: &str,
            _target: &str,
            _limits: Option<RunLimits>,
        ) -> Result<ExecHandle, RelayReject> {
            self.engine
                .try_start(
                    CommandTemplate {
                        program: "sh",
                        args: vec![
                            "-c".into(),
                            "while true; do printf 'x\\n'; sleep 0.05; done".into(),
                        ],
                    },
                    None,
                )
                .map_err(|_| RelayReject::Busy)
        }
    }

    async fn established_agent_channel(
    ) -> (AuthChannel<ChannelTransport>, AuthChannel<ChannelTransport>) {
        let (agent_side, central_side) = ChannelTransport::pair();
        let client =
            tokio::spawn(
                async move { client_handshake(agent_side, "agent-1", CRED, [1u8; 32]).await },
            );
        let (_id, central_channel) = server_handshake(
            central_side,
            [2u8; 32],
            |_, cred| async move { cred == CRED },
        )
        .await
        .expect("server handshake");
        let agent_channel = client.await.unwrap().expect("client handshake");
        (agent_channel, central_channel)
    }

    // AC7 (relay round-trip): a command relayed down runs agent-side (through the
    // real exec engine) and its output streams back up the authenticated channel.
    #[tokio::test]
    async fn relayed_command_runs_and_streams_output_back() {
        let (mut agent_channel, mut central_channel) = established_agent_channel().await;
        let executor = EchoExecutor {
            engine: ExecEngine::new(ExecLimits::default()),
        };

        let serving = tokio::spawn(async move {
            serve_relay(&mut agent_channel, &executor, &DataPlaneLink::new().0).await
        });

        central_channel
            .send_message(&TunnelMessage::Command {
                run_id: "r1".into(),
                method: "ping".into(),
                target: "8.8.8.8".into(),
                limits: None,
            })
            .await
            .unwrap();

        let mut lines = Vec::new();
        loop {
            match central_channel.recv_message().await.unwrap() {
                TunnelMessage::Output { line, .. } => lines.push(line),
                TunnelMessage::Done { ok, .. } => {
                    assert!(ok, "the relayed run completed successfully");
                    break;
                }
                other => panic!("unexpected frame: {other:?}"),
            }
        }
        assert_eq!(
            lines,
            vec!["relayed-1".to_string(), "relayed-2".to_string()]
        );
        drop(central_channel);
        let _ = serving.await;
    }

    // Finding 1 (b): when the central side goes away mid-run, the agent's abandoned
    // process is reaped and its exec permit released — no orphan holding an AC40
    // permit central believes is free. A returned permit proves the process group
    // was killed (shared::exec releases the permit only after the kill).
    #[tokio::test]
    async fn early_exit_reaps_the_agent_process_and_releases_its_permit() {
        let engine = ExecEngine::new(ExecLimits {
            max_concurrent: 1,
            timeout: Duration::from_secs(10),
            ..ExecLimits::default()
        });
        let executor = LoopExecutor {
            engine: engine.clone(),
        };
        let (agent_channel, mut central_channel) = established_agent_channel().await;
        let serving = tokio::spawn(async move {
            let mut agent_channel = agent_channel;
            let _ = serve_relay(&mut agent_channel, &executor, &DataPlaneLink::new().0).await;
        });

        central_channel
            .send_message(&TunnelMessage::Command {
                run_id: "r1".into(),
                method: "ping".into(),
                target: "8.8.8.8".into(),
                limits: None,
            })
            .await
            .unwrap();
        // First output line → the relayed process is running and holds the permit.
        let first = central_channel.recv_message().await.unwrap();
        assert!(matches!(first, TunnelMessage::Output { .. }));
        assert_eq!(
            engine.available_permits(),
            0,
            "the relayed process holds the exec permit"
        );

        // The central side goes away (early exit / connection teardown).
        drop(central_channel);

        // The agent's next send fails → the run is abandoned → the process group is
        // killed and the permit released.
        for _ in 0..100 {
            if engine.available_permits() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            engine.available_permits(),
            1,
            "the abandoned run's process is reaped and its permit released — no orphan"
        );
        let _ = serving.await;
    }

    // Central stops one relayed run without closing the tunnel. The agent
    // kills that run, answers with its terminal frame, and serves the next one.
    #[tokio::test]
    async fn cancel_stops_the_named_run_and_keeps_the_tunnel() {
        let engine = ExecEngine::new(ExecLimits {
            max_concurrent: 1,
            timeout: Duration::from_secs(10),
            ..ExecLimits::default()
        });
        let executor = LoopExecutor {
            engine: engine.clone(),
        };
        let (mut agent_channel, mut central_channel) = established_agent_channel().await;
        let serving = tokio::spawn(async move {
            serve_relay(&mut agent_channel, &executor, &DataPlaneLink::new().0).await
        });

        central_channel
            .send_message(&TunnelMessage::Command {
                run_id: "r1".into(),
                method: "ping".into(),
                target: "8.8.8.8".into(),
                limits: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            central_channel.recv_message().await.unwrap(),
            TunnelMessage::Output { .. }
        ));
        central_channel
            .send_message(&TunnelMessage::Cancel {
                run_id: "r1".into(),
            })
            .await
            .unwrap();

        let terminal = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match central_channel.recv_message().await.unwrap() {
                    TunnelMessage::Output { .. } => continue,
                    other => break other,
                }
            }
        })
        .await
        .expect("the agent must answer a cancel with the run's terminal frame");
        assert_eq!(terminal, TunnelMessage::done("r1", ExecStatus::Canceled));
        for _ in 0..100 {
            if engine.available_permits() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(engine.available_permits(), 1, "the cancelled run is reaped");

        central_channel
            .send_message(&TunnelMessage::Command {
                run_id: "r2".into(),
                method: "ping".into(),
                target: "8.8.8.8".into(),
                limits: None,
            })
            .await
            .unwrap();
        assert_eq!(
            central_channel.recv_message().await.unwrap().run_id(),
            Some("r2"),
            "the same tunnel serves the next run"
        );
        assert!(!serving.is_finished());
        serving.abort();
    }

    /// Runs the method name as a program through the real exec engine, so a
    /// test picks a missing tool, a failing script, or one that outlives the
    /// engine's timeout.
    struct ScriptExecutor {
        engine: ExecEngine,
    }
    impl CommandExecutor for ScriptExecutor {
        async fn spawn(
            &self,
            method: &str,
            _target: &str,
            _limits: Option<RunLimits>,
        ) -> Result<ExecHandle, RelayReject> {
            let (program, script) = match method {
                "missing" => ("lg-t015-no-such-tool", ""),
                "exit3" => ("sh", "exit 3"),
                "silent" => ("sh", "sleep 60"),
                "flood" => ("sh", "while :; do echo flood; done"),
                _ => ("sh", "printf 'ok\\n'"),
            };
            self.engine
                .try_start(
                    CommandTemplate {
                        program,
                        args: vec!["-c".into(), script.into()],
                    },
                    None,
                )
                .map_err(|_| RelayReject::Busy)
        }
    }

    fn script_command(run_id: &str, method: &str) -> TunnelMessage {
        TunnelMessage::Command {
            run_id: run_id.into(),
            method: method.into(),
            target: "8.8.8.8".into(),
            limits: None,
        }
    }

    /// Relay one command and return every frame up to its first terminal.
    async fn relay_until_terminal(
        central: &mut AuthChannel<ChannelTransport>,
        command: TunnelMessage,
    ) -> TunnelMessage {
        central.send_message(&command).await.unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match central.recv_message().await.unwrap() {
                    TunnelMessage::Output { .. } | TunnelMessage::Heartbeat => continue,
                    other => break other,
                }
            }
        })
        .await
        .expect("the run reaches a terminal frame")
    }

    // A run that fails (here, a missing tool) sends one terminal frame,
    // its Error, and no Done after it.
    #[tokio::test]
    async fn a_failed_run_sends_exactly_one_terminal_frame() {
        let executor = ScriptExecutor {
            engine: ExecEngine::new(ExecLimits::default()),
        };
        let (mut agent_channel, mut central) = established_agent_channel().await;
        let serving = tokio::spawn(async move {
            serve_relay(&mut agent_channel, &executor, &DataPlaneLink::new().0).await
        });

        let terminal = relay_until_terminal(&mut central, script_command("r1", "missing")).await;
        assert!(
            matches!(terminal, TunnelMessage::Error { .. }),
            "{terminal:?}"
        );
        let after = tokio::time::timeout(Duration::from_millis(500), central.recv_message()).await;
        assert!(
            after.is_err(),
            "no second terminal may follow the run's Error, got {after:?}"
        );

        let next = relay_until_terminal(&mut central, script_command("r2", "echo")).await;
        assert_eq!(next.run_id(), Some("r2"), "the tunnel serves the next run");
        serving.abort();
    }

    // Done carries how the run ended, so central can tell a timeout and a
    // non-zero exit apart the way a local node does. The budget leaves `exit 3`
    // headroom on a loaded host; `sleep 60` is far past it.
    #[tokio::test]
    async fn done_reports_a_timeout_and_a_non_zero_exit() {
        let executor = ScriptExecutor {
            engine: ExecEngine::new(ExecLimits {
                timeout: Duration::from_secs(3),
                ..ExecLimits::default()
            }),
        };
        let (mut agent_channel, mut central) = established_agent_channel().await;
        let serving = tokio::spawn(async move {
            serve_relay(&mut agent_channel, &executor, &DataPlaneLink::new().0).await
        });

        for (run_id, method, status) in [("r1", "silent", "timeout"), ("r2", "exit3", "completed")]
        {
            let done = relay_until_terminal(&mut central, script_command(run_id, method)).await;
            let done = serde_json::to_value(&done).unwrap();
            assert_eq!(done["Done"]["run_id"], run_id, "{done}");
            assert_eq!(done["Done"]["ok"], false, "{done}");
            assert_eq!(done["Done"]["status"], status, "{done}");
        }
        serving.abort();
    }

    // A run that fails still ends with its Error alone, and the Error says how
    // it ended, so central can tell a truncated run from a failed one.
    #[tokio::test]
    async fn a_failed_run_error_reports_truncated_or_failed() {
        let executor = ScriptExecutor {
            engine: ExecEngine::new(ExecLimits {
                max_output_bytes: 64,
                ..ExecLimits::default()
            }),
        };
        let (mut agent_channel, mut central) = established_agent_channel().await;
        let serving = tokio::spawn(async move {
            serve_relay(&mut agent_channel, &executor, &DataPlaneLink::new().0).await
        });

        for (run_id, method, status) in [("r1", "flood", "truncated"), ("r2", "missing", "failed")]
        {
            let error = relay_until_terminal(&mut central, script_command(run_id, method)).await;
            let error = serde_json::to_value(&error).unwrap();
            assert_eq!(error["Error"]["run_id"], run_id, "{error}");
            assert_eq!(error["Error"]["status"], status, "{error}");
        }
        serving.abort();
    }

    // Reuse-integrity (AC40): the production executor runs through the shared exec
    // engine and honours its global cap — at the cap a relayed command is refused
    // with `Busy` and no second executor is involved.
    #[tokio::test]
    async fn node_executor_honours_the_global_exec_cap() {
        let engine = ExecEngine::new(ExecLimits {
            max_concurrent: 1,
            ..ExecLimits::default()
        });
        // Saturate the one permit on the shared engine.
        let _held = engine
            .clone()
            .try_start(
                CommandTemplate {
                    program: "sh",
                    args: vec!["-c".into(), "sleep 5".into()],
                },
                None,
            )
            .unwrap();

        let executor = NodeExecutor::new(engine, StubResolver);
        let refused = executor.spawn("ping", "8.8.8.8", None).await.err();
        assert!(
            matches!(refused, Some(RelayReject::Busy)),
            "over the cap the relay must be refused via the shared engine, got {refused:?}"
        );
    }

    // The SSRF boundary holds on the relay path: a private target is rejected via
    // shared::validate before any process is spawned.
    #[tokio::test]
    async fn node_executor_rejects_a_private_target() {
        let executor = NodeExecutor::new(ExecEngine::new(ExecLimits::default()), StubResolver);
        let rejected = executor.spawn("ping", "10.0.0.1", None).await.err();
        assert!(
            matches!(rejected, Some(RelayReject::Rejected(_))),
            "a private target must be rejected at the SSRF boundary, got {rejected:?}"
        );
    }

    // A v6 literal on a v4 method is refused with a family error,
    // never spawned as `ping -4 <v6>`.
    #[tokio::test]
    async fn node_executor_rejects_a_wrong_family_target() {
        let executor = NodeExecutor::new(ExecEngine::new(ExecLimits::default()), StubResolver);
        match executor.spawn("ping", "2606:4700:4700::1111", None).await {
            Err(RelayReject::Rejected(message)) => assert!(message.contains("IPv4"), "{message}"),
            Err(other) => panic!("expected a family rejection, got {other:?}"),
            Ok(_) => panic!("a wrong-family target must be refused, not spawned"),
        }
    }

    // An unknown method name is refused — central cannot drive an unsupported method.
    #[tokio::test]
    async fn node_executor_refuses_an_unknown_method() {
        let executor = NodeExecutor::new(ExecEngine::new(ExecLimits::default()), StubResolver);
        assert!(matches!(
            executor.spawn("telnet", "8.8.8.8", None).await,
            Err(RelayReject::UnknownMethod(_))
        ));
    }

    // ----- Slice 11: BGP agent-side execution ---------------------------------

    struct StubProbe(Option<shared::template::BgpDaemon>);
    impl DaemonProbe for StubProbe {
        fn detect(&self) -> Option<shared::template::DaemonCli> {
            self.0.map(shared::template::DaemonCli::on_path)
        }
    }

    fn bgp_executor(daemon: Option<shared::template::BgpDaemon>) -> NodeExecutor<StubResolver> {
        NodeExecutor::new(ExecEngine::new(ExecLimits::default()), StubResolver)
            .with_daemon_probe(Arc::new(StubProbe(daemon)))
    }

    /// A routing "daemon" whose CLI is `yes`: a relayed BGP run floods output
    /// until its limits stop it.
    struct FloodProbe;
    impl DaemonProbe for FloodProbe {
        fn detect(&self) -> Option<shared::template::DaemonCli> {
            Some(shared::template::DaemonCli {
                daemon: shared::template::BgpDaemon::Bird,
                program: "yes",
            })
        }
    }

    /// Relay one command; count its output lines up to its terminal's status.
    /// Every run here ends by its own timeout, at most the agent's default, so
    /// the budget only catches a hang and never races a slow relay.
    async fn relay_counting(
        central: &mut AuthChannel<ChannelTransport>,
        command: TunnelMessage,
    ) -> (usize, Option<ExecStatus>) {
        central.send_message(&command).await.unwrap();
        let mut lines = 0;
        tokio::time::timeout(2 * ExecLimits::default().timeout, async {
            loop {
                match central.recv_message().await.unwrap() {
                    TunnelMessage::Output { .. } => lines += 1,
                    TunnelMessage::Heartbeat => {}
                    terminal => break (lines, terminal.terminal_status()),
                }
            }
        })
        .await
        .expect("the run reaches a terminal frame")
    }

    // F-154: a relayed run is bounded by the limits its Command carries: a 1 s
    // timeout ends it at ~1 s, a 64-byte cap truncates it there, and a frame
    // without limits (an older central) runs with the agent's defaults.
    #[tokio::test]
    async fn node_executor_applies_the_relayed_limits() {
        let executor = NodeExecutor::new(ExecEngine::new(ExecLimits::default()), StubResolver)
            .with_daemon_probe(Arc::new(FloodProbe));
        let (mut agent_channel, mut central) = established_agent_channel().await;
        let serving = tokio::spawn(async move {
            serve_relay(&mut agent_channel, &executor, &DataPlaneLink::new().0).await
        });
        let bgp = |run_id: &str, limits| TunnelMessage::Command {
            run_id: run_id.into(),
            method: "bgp".into(),
            target: "10.0.0.0/8".into(),
            limits,
        };

        let started = std::time::Instant::now();
        let timed = Some(RunLimits {
            timeout_secs: 1,
            max_output_bytes: 1 << 30,
        });
        let (_, status) = relay_counting(&mut central, bgp("r1", timed)).await;
        assert_eq!(status, Some(ExecStatus::TimedOut));
        // Timed out at ~1 s, well before the agent's default: the relayed 1 s
        // timeout applied, not a stretched or clamped one.
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");

        let capped = Some(RunLimits {
            timeout_secs: 30,
            max_output_bytes: 64,
        });
        let (lines, status) = relay_counting(&mut central, bgp("r2", capped)).await;
        assert_eq!(status, Some(ExecStatus::OutputCapped));
        assert!(
            lines <= 3,
            "a 64-byte cap stops `yes` within 3 lines, got {lines}"
        );

        let (lines, status) = relay_counting(&mut central, bgp("r3", None)).await;
        assert_eq!(status, Some(ExecStatus::OutputCapped));
        assert!(lines > 1000, "the default 256 KiB cap applies, got {lines}");
        serving.abort();
    }

    // AC32/AC37 agent leg: a valid BGP prefix with a detected daemon is dispatched
    // to the shared exec engine (a handle is returned — the read-only template is
    // asserted in shared::template). A private CIDR is accepted (no SSRF filter).
    #[tokio::test]
    async fn node_executor_runs_bgp_with_a_present_daemon() {
        let executor = bgp_executor(Some(shared::template::BgpDaemon::Bird));
        assert!(
            executor.spawn("bgp", "10.0.0.0/8", None).await.is_ok(),
            "a valid v4 prefix with a daemon present must dispatch to exec"
        );
        let frr = bgp_executor(Some(shared::template::BgpDaemon::Frr));
        assert!(frr.spawn("bgp6", "2001:db8::/32", None).await.is_ok());
    }

    // AC41 agent leg: BGP on a node with no supported daemon is refused with a clear
    // message, before any process is spawned — no hang.
    #[tokio::test]
    async fn node_executor_bgp_is_unavailable_without_a_daemon() {
        let executor = bgp_executor(None);
        match executor.spawn("bgp", "8.8.8.8", None).await {
            Err(RelayReject::Rejected(message)) => {
                assert!(message.contains("routing daemon"), "{message}");
            }
            Err(other) => panic!("expected a clear BGP-unavailable rejection, got {other:?}"),
            Ok(_) => panic!("BGP with no daemon must be refused, not dispatched"),
        }
    }

    // AC36/T4 agent leg: an injected or wrong-family prefix is rejected BEFORE any
    // daemon command, even with a daemon present, and consumes no exec permit.
    #[tokio::test]
    async fn node_executor_bgp_rejects_injected_and_wrong_family_prefixes() {
        let engine = ExecEngine::new(ExecLimits::default());
        let before = engine.available_permits();
        let executor = NodeExecutor::new(engine.clone(), StubResolver)
            .with_daemon_probe(Arc::new(StubProbe(Some(shared::template::BgpDaemon::Bird))));

        assert!(matches!(
            executor.spawn("bgp", "8.8.8.8; configure", None).await,
            Err(RelayReject::Rejected(_))
        ));
        // A v4 literal on the v6 bgp6 method is wrong-family, refused pre-daemon.
        assert!(matches!(
            executor.spawn("bgp6", "8.8.8.8", None).await,
            Err(RelayReject::Rejected(_))
        ));
        assert_eq!(
            engine.available_permits(),
            before,
            "a rejected BGP prefix must not spawn a process or hold a permit"
        );
    }

    // A tunnel that stays up while idle is a no-op here; the reconnect backoff is a
    // constant, asserted so a future edit cannot silently make it a tight loop.
    #[test]
    fn reconnect_backoff_is_bounded_and_nonzero() {
        assert!(RECONNECT_BACKOFF >= Duration::from_secs(1));
    }

    #[test]
    fn lifecycle_logs_use_connection_correlation_and_hide_secret_fields() {
        let logs = captured_logs();
        let config = TunnelClientConfig {
            host: "central.test".to_string(),
            port: 8443,
            fingerprint: "fingerprint-secret-slice13".to_string(),
            agent_id: "agent-slice13".to_string(),
            credential: "credential-secret-slice13".to_string(),
        };
        let correlation_id = connection_correlation_id();
        assert_ne!(correlation_id, config.agent_id);

        log_agent_connect(&correlation_id, &config);
        log_agent_disconnect(&correlation_id, &config);

        let captured = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        assert!(
            contains_field(&captured, "event", "agent.connect"),
            "{captured}"
        );
        assert!(
            contains_field(&captured, "event", "agent.disconnect"),
            "{captured}"
        );
        assert!(
            contains_field(&captured, "correlation_id", &correlation_id),
            "{captured}"
        );
        for secret in [&config.credential, &config.fingerprint] {
            assert!(
                !captured.contains(secret),
                "secret leaked into logs: {secret}"
            );
        }
    }

    // Slice 8b: an idle agent beats every HEARTBEAT_INTERVAL. Paused-clock time is
    // advanced past one interval and central receives exactly a Heartbeat frame — no
    // real sleep, so the test is deterministic.
    #[tokio::test(start_paused = true)]
    async fn serve_relay_beats_on_the_heartbeat_interval() {
        let (mut agent_channel, mut central_channel) = established_agent_channel().await;
        let executor = EchoExecutor {
            engine: ExecEngine::new(ExecLimits::default()),
        };
        let serving = tokio::spawn(async move {
            let _ = serve_relay(&mut agent_channel, &executor, &DataPlaneLink::new().0).await;
        });

        // Nothing before the first interval elapses (the immediate tick is skipped).
        tokio::time::advance(HEARTBEAT_INTERVAL + Duration::from_millis(1)).await;
        let beat = central_channel.recv_message().await.unwrap();
        assert_eq!(
            beat,
            TunnelMessage::Heartbeat,
            "the agent beats once per interval"
        );

        // And again on the next interval — it is periodic, not a one-shot.
        tokio::time::advance(HEARTBEAT_INTERVAL).await;
        assert_eq!(
            central_channel.recv_message().await.unwrap(),
            TunnelMessage::Heartbeat
        );
        serving.abort();
    }

    // The origin central assigns reaches the data plane, and the data
    // plane's certificate status goes up only once central has sent one (an older
    // central cannot decode `Certificate`), then again on every change.
    #[tokio::test]
    async fn a_data_plane_origin_reaches_the_data_plane_and_its_status_goes_up() {
        let (mut agent_channel, mut central) = established_agent_channel().await;
        let (link, mut origin, status) = DataPlaneLink::new();
        let serving = tokio::spawn(async move {
            let executor = EchoExecutor {
                engine: ExecEngine::new(ExecLimits::default()),
            };
            serve_relay(&mut agent_channel, &executor, &link).await
        });

        let pending = CertificateStatus {
            last_error: Some("not issued yet".into()),
            ..CertificateStatus::default()
        };
        status.send_replace(Some(pending.clone()));
        assert!(
            tokio::time::timeout(Duration::from_millis(300), central.recv_message())
                .await
                .is_err(),
            "no certificate status before central assigned an origin"
        );

        central
            .send_message(&TunnelMessage::DataPlane {
                origin: "https://node.example.test".into(),
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), origin.wait_for(Option::is_some))
            .await
            .expect("the assigned origin must reach the data plane")
            .unwrap();
        assert_eq!(
            origin.borrow().as_deref(),
            Some("https://node.example.test")
        );
        assert_eq!(
            central.recv_message().await.unwrap(),
            TunnelMessage::Certificate(CertificateReport {
                status: pending,
                origin: Some("https://node.example.test".into()),
            }),
            "the current status goes up, for its origin, once central assigned one"
        );

        let issued = CertificateStatus {
            issued_at: Some(10),
            expires_at: Some(20),
            last_error: None,
        };
        status.send_replace(Some(issued.clone()));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), central.recv_message())
                .await
                .expect("a changed status goes up")
                .unwrap(),
            TunnelMessage::Certificate(CertificateReport {
                status: issued,
                origin: Some("https://node.example.test".into()),
            })
        );
        serving.abort();
    }

    // F-207: a status the data plane holds for an origin central has since
    // replaced is not reported for the new one, even across a reconnect: the new
    // origin voids it, and the data plane's next status goes up for the new origin.
    #[tokio::test]
    async fn a_status_for_a_replaced_origin_is_not_reported_for_the_new_one() {
        let (link, _origin, status) = DataPlaneLink::new();
        let link = std::sync::Arc::new(link);
        let connect = |origin: &'static str| {
            let link = std::sync::Arc::clone(&link);
            async move {
                let (mut agent_channel, mut central) = established_agent_channel().await;
                let serving = tokio::spawn(async move {
                    let executor = EchoExecutor {
                        engine: ExecEngine::new(ExecLimits::default()),
                    };
                    serve_relay(&mut agent_channel, &executor, &link).await
                });
                central
                    .send_message(&TunnelMessage::DataPlane {
                        origin: origin.into(),
                    })
                    .await
                    .unwrap();
                (central, serving)
            }
        };
        let report = |status: &CertificateStatus, origin: &str| {
            TunnelMessage::Certificate(CertificateReport {
                status: status.clone(),
                origin: Some(origin.into()),
            })
        };

        let old = CertificateStatus {
            issued_at: Some(1),
            expires_at: Some(2),
            last_error: None,
        };
        let (mut central, serving) = connect("https://old.example.test").await;
        status.send_replace(Some(old.clone()));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), central.recv_message())
                .await
                .expect("a status goes up")
                .unwrap(),
            report(&old, "https://old.example.test")
        );
        serving.abort();

        // The admin changed the origin while the node was away.
        let (mut central, serving) = connect("https://new.example.test").await;
        assert!(
            tokio::time::timeout(Duration::from_millis(300), central.recv_message())
                .await
                .is_err(),
            "the old origin's certificate must not go up for the new origin"
        );
        let new = CertificateStatus {
            issued_at: Some(3),
            expires_at: Some(4),
            last_error: None,
        };
        status.send_replace(Some(new.clone()));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), central.recv_message())
                .await
                .expect("a status goes up")
                .unwrap(),
            report(&new, "https://new.example.test")
        );
        serving.abort();
    }

    // F-237: a data port that cannot bind is retried until it can, and its
    // error keeps going up meanwhile, for a new origin too, so the admin always
    // sees why the node has no HTTPS; once the port frees, the node serves.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_data_port_bind_is_retried_and_reported_for_a_new_origin() {
        use crate::acme::fake::{temp_dir, FakeCa};

        async fn next_report(central: &mut AuthChannel<ChannelTransport>) -> CertificateReport {
            loop {
                match central.recv_message().await.unwrap() {
                    TunnelMessage::Certificate(report) => return report,
                    _ => continue,
                }
            }
        }
        let bind_error = |report: &CertificateReport, origin: &str| {
            report.origin.as_deref() == Some(origin)
                && report
                    .status
                    .last_error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("cannot listen for HTTPS"))
        };

        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = busy.local_addr().unwrap();
        let ca = FakeCa::new(90 * 86_400);
        ca.state().port = addr.port();
        let dir = temp_dir("bind-retry");
        // Only this test starts `dataplane::run`, the one reader of these.
        std::env::set_var("LG_AGENT_DATA_BIND", addr.to_string());
        std::env::set_var("LG_AGENT_FILES_DIR", dir.join("files"));
        let (link, origin_rx, status_tx) = DataPlaneLink::new();
        let data_plane = tokio::spawn(crate::dataplane::run(origin_rx, status_tx, ca.acme(&dir)));
        let (mut agent_channel, mut central) = established_agent_channel().await;
        let serving = tokio::spawn(async move {
            let executor = EchoExecutor {
                engine: ExecEngine::new(ExecLimits::default()),
            };
            serve_relay(&mut agent_channel, &executor, &link).await
        });

        central
            .send_message(&TunnelMessage::DataPlane {
                origin: "https://a.example.test".into(),
            })
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(30), next_report(&mut central))
            .await
            .expect("the bind error goes up");
        assert!(bind_error(&first, "https://a.example.test"), "{first:?}");

        central
            .send_message(&TunnelMessage::DataPlane {
                origin: "https://b.example.test".into(),
            })
            .await
            .unwrap();
        let second = tokio::time::timeout(Duration::from_secs(30), next_report(&mut central)).await;
        assert!(
            second
                .as_ref()
                .is_ok_and(|report| bind_error(report, "https://b.example.test")),
            "the bind error must go up again for the new origin: {second:?}"
        );

        drop(busy);
        // Headroom for a loaded host: the bind retry, then the CA's validation
        // polls (instant-acme backs off 250 ms, 500 ms, 1 s, ...).
        let recovered = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let report = next_report(&mut central).await;
                if report.status.issued_at.is_some() && report.status.last_error.is_none() {
                    return report;
                }
            }
        })
        .await;
        assert!(
            recovered
                .as_ref()
                .is_ok_and(|report| report.origin.as_deref() == Some("https://b.example.test")),
            "once the port frees the bind is retried and a certificate issued: {recovered:?}"
        );
        assert!(!data_plane.is_finished(), "the data plane keeps serving");
        data_plane.abort();
        serving.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    // --- Central is pinned by its public key (SPKI), not its whole certificate ---

    /// Another central's certificate (a different key). Only its public part is
    /// needed: the agent must refuse on the pin, before any handshake signature.
    const ANOTHER_CENTRAL_CERT: &str = "-----BEGIN CERTIFICATE-----\nMIIDGjCCAgKgAwIBAgIUB8nOrMrkRWr5lR8RsPnj4J/PhBcwDQYJKoZIhvcNAQEL\nBQAwFDESMBAGA1UEAwwJMTI3LjAuMC4xMB4XDTI2MDcwNTIzMzYwMFoXDTI2MDcw\nNjIzMzYwMFowFDESMBAGA1UEAwwJMTI3LjAuMC4xMIIBIjANBgkqhkiG9w0BAQEF\nAAOCAQ8AMIIBCgKCAQEAv+zcTZ2KTH4jWa3v76GFj0xg6+UVkmC0e8GyIitJJnQ2\nDrPsa9xK1OZHNL0RjGFHZBu/DxMmaQfbhl4Izmi+pBA1OwjfWPCr6wr4N4+dtN2F\nPQsY7vr6EZxnd/C49nRS+yXVmHUaCkEC2SxoXrQ7wBLpjA6Y70R3vdcoRFgdYu3u\nvddYRSTL9I16x7daCs4m/L3I7I3BKS41aBGZF4dJ1yZ2LgyxB9mE+kF6ZZQLuqt+\nlnLm4gwfkbZCVp9TBwJRJ766TDlGHKC0bO0dgLEBg8gRuZAv9mPPEqov/XWciUsS\nc7thsQ2FG8hUMXVeKrCRvUrUo/DtB8pRlXGcmBDJCwIDAQABo2QwYjAdBgNVHQ4E\nFgQUz4NF3ivpta/zCGT/my4yEvnURuYwHwYDVR0jBBgwFoAUz4NF3ivpta/zCGT/\nmy4yEvnURuYwDwYDVR0TAQH/BAUwAwEB/zAPBgNVHREECDAGhwR/AAABMA0GCSqG\nSIb3DQEBCwUAA4IBAQAkCaT76n7ECoFqfUWAaNypbyFDufX/DY8F60yLMLeLZn3r\nK8swCxa/VKLCdj+5BANJC/2l+L0a1yaiCrzfZaecTAG8LhlZECdUJKfKZ+R5zSDP\nap+EBNggS01ZBV9BINqtX6LP2s5qoBw/Y2rVPrtQWW8HOaULWZLqaWJ9NlpQE+SJ\n3ERIsGI+v3NqnK4sTc8ib3FXbuYCHSooXQrZCUzIjO5H8pdvDOcDqEYn0C+Ye7vD\nISssAEPekto7X9oDQ1iIDeRZy5yYgiQ3OyXyqan4FzdvKP4HKVBM3ZYSmm05bO9r\nNp7mtV1m/hMoD8X1QW1khM6/cFDTa5bjxqnuPrM0\n-----END CERTIFICATE-----";

    fn central_cert_and_key() -> (
        CertificateDer<'static>,
        rustls::pki_types::PrivateKeyDer<'static>,
    ) {
        use rustls::pki_types::pem::PemObject;
        let cert = CertificateDer::from_pem_slice(crate::enroll::tests::TEST_CERT.as_bytes())
            .expect("test certificate");
        let key = rustls::pki_types::PrivateKeyDer::from_pem_slice(
            crate::enroll::tests::TEST_KEY.as_bytes(),
        )
        .expect("test private key");
        (cert, key)
    }

    /// The pin central mints: SHA-256 over the leaf's DER SubjectPublicKeyInfo,
    /// read here through rustls' own certificate parser.
    fn public_key_pin(cert: &CertificateDer<'_>) -> String {
        let parsed = rustls::server::ParsedCertificate::try_from(cert).expect("parse certificate");
        shared::protocol::sha256_hex(parsed.subject_public_key_info().as_ref())
    }

    /// Split one DER TLV off the front of `der`: the whole TLV, its header length,
    /// and what follows it.
    fn der_tlv(der: &[u8]) -> (&[u8], usize, &[u8]) {
        let (len, header) = match der[1] {
            n if n < 0x80 => (usize::from(n), 2),
            0x81 => (usize::from(der[2]), 3),
            0x82 => (usize::from(u16::from_be_bytes([der[2], der[3]])), 4),
            other => panic!("unsupported DER length form {other:#x}"),
        };
        let (tlv, rest) = der.split_at(header + len);
        (tlv, header, rest)
    }

    fn der_wrap(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        match body.len() {
            n if n < 0x80 => out.push(n as u8),
            n if n < 0x100 => out.extend([0x81, n as u8]),
            n => out.extend([0x82, (n >> 8) as u8, n as u8]),
        }
        out.extend_from_slice(body);
        out
    }

    /// Renew `cert` the way `certbot --reuse-key` does: the same key, a new serial
    /// and a new validity, signed again with that key.
    fn renew_with_the_same_key(
        cert: &CertificateDer<'_>,
        key: &rustls::pki_types::PrivateKeyDer<'_>,
    ) -> CertificateDer<'static> {
        let (_, header, _) = der_tlv(cert.as_ref());
        let (tbs, _, rest) = der_tlv(&cert.as_ref()[header..]);
        let (signature_algorithm, _, _) = der_tlv(rest);
        let mut tbs = tbs.to_vec();
        // TBSCertificate: [0] version, serialNumber, signature, issuer, validity, ...
        let mut at = der_tlv(&tbs).1;
        let mut fields = Vec::new();
        for _ in 0..5 {
            let len = der_tlv(&tbs[at..]).0.len();
            fields.push((at, len));
            at += len;
        }
        let (serial_at, serial_len) = fields[1];
        tbs[serial_at + serial_len - 1] ^= 0x01;
        let (validity_at, _) = fields[4];
        let mut time_at = validity_at + der_tlv(&tbs[validity_at..]).1;
        for _ in 0..2 {
            let (time, time_header, _) = der_tlv(&tbs[time_at..]);
            let time_len = time.len();
            // The year's last digit: UTCTime is YYMMDD..., GeneralizedTime YYYYMMDD...
            let digit = time_at + time_header + if tbs[time_at] == 0x17 { 1 } else { 3 };
            tbs[digit] = b'0' + (tbs[digit] - b'0' + 1) % 10;
            time_at += time_len;
        }
        let signer = rustls::crypto::ring::sign::any_supported_type(key)
            .expect("test signing key")
            .choose_scheme(&[
                SignatureScheme::ECDSA_NISTP256_SHA256,
                SignatureScheme::RSA_PKCS1_SHA256,
            ])
            .expect("test signature scheme");
        let mut signature = vec![0];
        signature.extend(signer.sign(&tbs).expect("sign renewed certificate"));
        let mut body = tbs;
        body.extend_from_slice(signature_algorithm);
        body.extend(der_wrap(0x03, &signature));
        CertificateDer::from(der_wrap(0x30, &body))
    }

    /// What a fake central does once the agent has authenticated.
    enum Then {
        /// Hang up.
        Close,
        /// Keep the connection open and never read or answer again.
        GoSilent,
    }

    /// A central on a loopback port presenting `cert`: TLS, WebSocket, then the
    /// shared credential handshake. Resolves to whether the agent authenticated;
    /// what it does next runs on in the background.
    async fn fake_central(
        cert: CertificateDer<'static>,
        key: rustls::pki_types::PrivateKeyDer<'static>,
        then: Then,
    ) -> (u16, tokio::task::JoinHandle<bool>) {
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("test TLS versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("test server certificate");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener");
        let port = listener.local_addr().expect("test listener address").port();
        let central = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept the agent");
            let Ok(tls) = tokio_rustls::TlsAcceptor::from(Arc::new(config))
                .accept(tcp)
                .await
            else {
                return false;
            };
            let Ok(ws) = tokio_tungstenite::accept_async(tls).await else {
                return false;
            };
            let Ok((_, channel)) = server_handshake(
                WsTransport::new(ws, DEADLINES.silence),
                [2u8; TUNNEL_KEY_BYTES],
                |_, credential| async move { credential == CRED },
            )
            .await
            else {
                return false;
            };
            match then {
                Then::Close => {}
                Then::GoSilent => {
                    tokio::spawn(async move {
                        let _held = channel;
                        std::future::pending::<()>().await
                    });
                }
            }
            true
        });
        (port, central)
    }

    fn loopback_config(port: u16, fingerprint: String) -> TunnelClientConfig {
        TunnelClientConfig {
            host: "127.0.0.1".to_string(),
            port,
            fingerprint,
            agent_id: "agent-1".to_string(),
            credential: CRED.to_string(),
        }
    }

    /// Dial `central` once through the real agent connect path; return whether the
    /// agent got through TLS pinning and authenticated.
    async fn connect_to(
        config: &mut TunnelClientConfig,
        central: tokio::task::JoinHandle<bool>,
    ) -> bool {
        let executor = EchoExecutor {
            engine: ExecEngine::new(ExecLimits::default()),
        };
        let link = DataPlaneLink::new().0;
        let agent = tokio::time::timeout(
            Duration::from_secs(30),
            connect_once(config, &executor, &link, "test-connection", DEADLINES),
        );
        let (agent, authenticated) = tokio::join!(agent, central);
        assert!(agent.is_ok(), "the agent connection attempt must end");
        authenticated.expect("fake central task")
    }

    // An agent enrolled against certificate A keeps connecting when central
    // presents a renewed certificate B with the same key (new serial and validity).
    #[tokio::test]
    async fn a_renewed_certificate_with_the_same_key_keeps_the_agent_connected() {
        let (cert, key) = central_cert_and_key();
        let renewed = renew_with_the_same_key(&cert, &key);
        assert_ne!(renewed, cert, "the renewal is a different certificate");
        assert_eq!(
            public_key_pin(&renewed),
            public_key_pin(&cert),
            "the renewal keeps the key"
        );

        let (port, central) = fake_central(renewed, key, Then::Close).await;
        let mut config = loopback_config(port, public_key_pin(&cert));

        assert!(
            connect_to(&mut config, central).await,
            "a renewed certificate with the same key must keep the agent connected"
        );
    }

    // Pinning the key still refuses a central with any other key, whether
    // the agent holds the public-key pin or a legacy whole-certificate pin.
    #[tokio::test]
    async fn a_central_with_a_different_key_is_still_refused() {
        use rustls::pki_types::pem::PemObject;
        let (cert, key) = central_cert_and_key();
        let enrolled = CertificateDer::from_pem_slice(ANOTHER_CENTRAL_CERT.as_bytes())
            .expect("another central's certificate");
        assert_ne!(public_key_pin(&enrolled), public_key_pin(&cert));

        for pin in [
            public_key_pin(&enrolled),
            shared::protocol::fingerprint(enrolled.as_ref()),
        ] {
            let (port, central) = fake_central(cert.clone(), key.clone_key(), Then::Close).await;
            let mut config = loopback_config(port, pin);
            assert!(
                !connect_to(&mut config, central).await,
                "a central with a different key must be refused"
            );
        }
    }

    // A credential stored before the public-key pin holds the SHA-256 of the
    // whole certificate. It still connects, and its first authenticated connection
    // rewrites the stored pin to the public-key pin (atomically, owner-only, same
    // owner), so the next renewal with the same key keeps it connected.
    #[tokio::test]
    async fn a_legacy_certificate_pin_connects_and_moves_to_the_public_key_pin() {
        let (cert, key) = central_cert_and_key();
        let legacy = crate::enroll::AgentCredential {
            agent_id: "agent-1".to_string(),
            credential: CRED.to_string(),
            central_url: "https://central.test".to_string(),
            tunnel_url: "https://tunnel.central.test:8443".to_string(),
            fingerprint: shared::protocol::fingerprint(cert.as_ref()),
        };
        let dir = std::env::temp_dir().join(format!(
            "lg-agent-repin-{}-{}",
            std::process::id(),
            connection_correlation_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent-credential.json");
        crate::enroll::store_credential(&path, &legacy).unwrap();
        // An earlier rewrite that crashed before its rename left its temp file.
        std::fs::write(
            dir.join(format!(".agent-credential.json.{}.tmp", std::process::id())),
            b"{",
        )
        .unwrap();
        let before = std::fs::metadata(&path).unwrap();
        std::env::set_var("LG_AGENT_CREDENTIAL", &path);

        let (port, central) = fake_central(cert.clone(), key.clone_key(), Then::Close).await;
        let mut config = loopback_config(port, legacy.fingerprint.clone());
        assert!(
            connect_to(&mut config, central).await,
            "a legacy certificate pin must still connect"
        );

        let stored: crate::enroll::AgentCredential =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            stored.fingerprint,
            public_key_pin(&cert),
            "the first authenticated connection must move the stored pin to the public key"
        );
        assert_eq!(
            (
                stored.agent_id.as_str(),
                stored.credential.as_str(),
                stored.central_url.as_str(),
                stored.tunnel_url.as_str()
            ),
            (
                legacy.agent_id.as_str(),
                legacy.credential.as_str(),
                legacy.central_url.as_str(),
                legacy.tunnel_url.as_str()
            ),
            "only the pin changes"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let after = std::fs::metadata(&path).unwrap();
            assert_eq!(
                after.permissions().mode() & 0o777,
                0o600,
                "still owner-only"
            );
            assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
        }
        let _ = before;

        let (port, central) =
            fake_central(renew_with_the_same_key(&cert, &key), key, Then::Close).await;
        config.port = port;
        let renewed = connect_to(&mut config, central).await;
        std::env::remove_var("LG_AGENT_CREDENTIAL");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            renewed,
            "after the move, a renewal with the same key must keep the agent connected"
        );
    }

    /// Deadlines short enough that a test sees the agent give up in seconds.
    const SHORT: Deadlines = Deadlines {
        connect: Duration::from_secs(2),
        silence: Duration::from_secs(1),
    };

    // A peer that accepts TCP and then never answers does not hang the dial:
    // the dial gives up at its deadline and the agent dials again after the
    // backoff.
    #[tokio::test]
    async fn a_silent_peer_is_dialled_again_after_the_connect_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (accepted, mut accepts) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((tcp, _)) = listener.accept().await {
                held.push(tcp);
                let _ = accepted.send(());
            }
        });
        let agent = tokio::spawn(run_with(
            loopback_config(port, "00".repeat(32)),
            EchoExecutor {
                engine: ExecEngine::new(ExecLimits::default()),
            },
            DataPlaneLink::new().0,
            SHORT,
        ));

        accepts.recv().await.expect("the first dial");
        let again = tokio::time::timeout(
            SHORT.connect + RECONNECT_BACKOFF + Duration::from_secs(2),
            accepts.recv(),
        )
        .await;
        agent.abort();
        assert!(
            again.is_ok(),
            "the agent must give up on a silent peer and dial again"
        );
    }

    /// Connect to `central` under `deadlines` and serve for at most `limit`.
    /// Returns how the connection ended (`None` if it was still up) and whether
    /// the agent authenticated.
    async fn serve_for(
        central: tokio::task::JoinHandle<bool>,
        config: &mut TunnelClientConfig,
        deadlines: Deadlines,
        limit: Duration,
    ) -> (Option<Result<(), TunnelError>>, bool) {
        let executor = EchoExecutor {
            engine: ExecEngine::new(ExecLimits::default()),
        };
        let link = DataPlaneLink::new().0;
        let serving = tokio::time::timeout(
            limit,
            connect_once(config, &executor, &link, "test-connection", deadlines),
        );
        let (ended, authenticated) = tokio::join!(serving, central);
        (ended.ok(), authenticated.expect("fake central task"))
    }

    // An established tunnel whose central goes silent (a frozen central, or a
    // path that black-holes) ends within the silence deadline, so the agent
    // dials again instead of waiting out TCP retransmission.
    #[tokio::test]
    async fn a_tunnel_to_a_silent_central_ends_within_the_silence_deadline() {
        let (cert, key) = central_cert_and_key();
        let (port, central) = fake_central(cert.clone(), key, Then::GoSilent).await;
        let mut config = loopback_config(port, public_key_pin(&cert));
        let (ended, authenticated) = serve_for(
            central,
            &mut config,
            SHORT,
            SHORT.silence + Duration::from_secs(2),
        )
        .await;
        assert!(authenticated, "the tunnel was established first");
        assert!(
            matches!(ended, Some(Err(_))),
            "a silent central must end the tunnel, got {ended:?}"
        );
    }

    // A central that is quiet but alive answers the agent's pings, so a quiet
    // tunnel is not mistaken for a dead one. F-350: an in-memory WebSocket on
    // virtual time, so a loaded host cannot delay a pong past the deadline.
    #[tokio::test(start_paused = true)]
    async fn a_quiet_live_central_keeps_the_tunnel() {
        use tokio_tungstenite::tungstenite::protocol::Role;

        let (agent, central) = tokio::io::duplex(64 * 1024);
        let agent = WebSocketStream::from_raw_socket(agent, Role::Client, None).await;
        let central = WebSocketStream::from_raw_socket(central, Role::Server, None).await;
        tokio::spawn(async move {
            let (_, mut channel) = server_handshake(
                WsTransport::new(central, DEADLINES.silence),
                [2u8; TUNNEL_KEY_BYTES],
                |_, credential| async move { credential == CRED },
            )
            .await
            .expect("the tunnel was established first");
            while channel.recv_message().await.is_ok() {}
        });
        let mut channel = client_handshake(
            WsTransport::new(agent, SHORT.silence),
            "agent-1",
            CRED,
            [1u8; 32],
        )
        .await
        .expect("the tunnel was established first");
        let executor = EchoExecutor {
            engine: ExecEngine::new(ExecLimits::default()),
        };
        let ended = tokio::time::timeout(
            SHORT.silence * 4,
            serve_relay(&mut channel, &executor, &DataPlaneLink::new().0),
        )
        .await;
        assert!(
            ended.is_err(),
            "a live central must keep the tunnel, got {ended:?}"
        );
    }

    // The agent keeps beating while a relayed run prints nothing, so central
    // never hears silence long enough to mark the location offline.
    #[tokio::test(start_paused = true)]
    async fn heartbeats_continue_while_a_silent_run_streams() {
        let executor = ScriptExecutor {
            engine: ExecEngine::new(ExecLimits::default()),
        };
        let (mut agent_channel, mut central) = established_agent_channel().await;
        let serving = tokio::spawn(async move {
            serve_relay(&mut agent_channel, &executor, &DataPlaneLink::new().0).await
        });

        central
            .send_message(&script_command("r1", "silent"))
            .await
            .unwrap();
        let mut last = tokio::time::Instant::now();
        loop {
            let frame = central.recv_message().await.unwrap();
            let quiet = last.elapsed();
            assert!(
                quiet <= HEARTBEAT_INTERVAL + Duration::from_secs(1),
                "central heard nothing for {quiet:?} during the run, then {frame:?}"
            );
            last = tokio::time::Instant::now();
            if matches!(
                frame,
                TunnelMessage::Done { .. } | TunnelMessage::Error { .. }
            ) {
                break;
            }
        }
        serving.abort();
    }

    // A central that stops reading cannot hold a send forever either: once the
    // socket fills, the send gives up within the silence deadline.
    #[tokio::test]
    async fn a_send_to_a_central_that_stopped_reading_gives_up() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let central = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let _ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            std::future::pending::<()>().await
        });
        let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (ws, _) = client_async(format!("ws://127.0.0.1:{port}/"), tcp)
            .await
            .unwrap();
        let mut transport = WsTransport::new(ws, SHORT.silence);
        let stalled = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Err(error) = transport.send(vec![0; 64 * 1024]).await {
                    break error;
                }
            }
        })
        .await;
        central.abort();
        assert!(
            matches!(&stalled, Ok(error) if error.kind() == std::io::ErrorKind::TimedOut),
            "a send into a full socket must give up, got {stalled:?}"
        );
    }

    fn contains_field(logs: &str, name: &str, value: &str) -> bool {
        logs.contains(&format!("{name}={value}")) || logs.contains(&format!("{name}=\"{value}\""))
    }

    static LOG_CAPTURE: OnceLock<StdArc<Mutex<Vec<u8>>>> = OnceLock::new();

    fn captured_logs() -> StdArc<Mutex<Vec<u8>>> {
        LOG_CAPTURE
            .get_or_init(|| {
                let buffer = StdArc::new(Mutex::new(Vec::new()));
                let writer_buffer = StdArc::clone(&buffer);
                let subscriber = tracing_subscriber::fmt()
                    .with_ansi(false)
                    .with_writer(move || CaptureWriter(StdArc::clone(&writer_buffer)))
                    .finish();
                let _ = tracing::subscriber::set_global_default(subscriber);
                buffer
            })
            .clone()
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
