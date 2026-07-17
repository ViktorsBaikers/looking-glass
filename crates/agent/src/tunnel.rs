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
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use shared::exec::{ExecEngine, ExecEvent, ExecHandle, ExecStatus, StartError};
use shared::liveness::HEARTBEAT_INTERVAL;
use shared::protocol::{
    client_handshake, verify_pinned_identity, AuthChannel, FrameTransport, TunnelError,
    TunnelMessage, TUNNEL_KEY_BYTES,
};
use shared::template::{DaemonProbe, Method, ScopedDaemonProbe};
use shared::validate::{bgp_arg, validate_target, HostResolver, PrefixFamily};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// Backoff between reconnect attempts — bounded so a flapping central does not
/// become a tight loop, short enough that a recovered agent rejoins promptly.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(5);

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
    /// Split a `host[:port]` (or a full `scheme://host:port/...`) tunnel URL into
    /// host + port, defaulting the port to 8443 (the tunnel listener).
    pub fn from_parts(
        tunnel_url: &str,
        fingerprint: String,
        agent_id: String,
        credential: String,
    ) -> Self {
        let (host, port) = parse_host_port(tunnel_url);
        Self {
            host,
            port,
            fingerprint,
            agent_id,
            credential,
        }
    }
}

fn parse_host_port(url: &str) -> (String, u16) {
    let without_scheme = url.split("://").last().unwrap_or(url);
    let authority = without_scheme
        .split(['/', '?'])
        .next()
        .unwrap_or(without_scheme);
    match authority.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().unwrap_or(8443)),
        None => (authority.to_string(), 8443),
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
    async fn spawn(&self, method: &str, target: &str) -> Result<ExecHandle, RelayReject> {
        // A diagnostic: re-validate the target through the SSRF boundary, pin the IP.
        if let Some(method) = method_from_wire(method) {
            let validated = validate_target(target, &self.resolver)
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

/// Map a wire method name to a runnable diagnostic [`Method`]. BGP names and any
/// unknown name yield `None`; BGP is recognised separately via
/// [`PrefixFamily::from_wire`] because it is prefix-based, not a target method.
fn method_from_wire(method: &str) -> Option<Method> {
    Some(match method {
        "ping" => Method::Ping,
        "ping6" => Method::Ping6,
        "mtr" => Method::Mtr,
        "mtr6" => Method::Mtr6,
        "traceroute" => Method::Traceroute,
        "traceroute6" => Method::Traceroute6,
        _ => return None,
    })
}

/// Serve relayed commands over an authenticated channel until it tears down. Each
/// `Command` runs locally and its output streams back as `Output`/`Done`/`Error`.
/// A channel error (bad tag, replay, close) propagates so the caller reconnects —
/// never continue past an auth failure.
pub async fn serve_relay<T, E>(
    channel: &mut AuthChannel<T>,
    executor: &E,
) -> Result<(), TunnelError>
where
    T: FrameTransport,
    E: CommandExecutor,
{
    // Beat every HEARTBEAT_INTERVAL so central derives this node online (Slice 8b).
    // The first tick fires immediately; skip it (the handshake already proved us
    // live), so the first beat lands one interval in. Delay missed-tick behaviour so
    // a long relayed run doesn't burst a backlog of catch-up beats when it finishes.
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                channel.send_message(&TunnelMessage::Heartbeat).await?;
            }
            message = channel.recv_message() => {
                match message? {
                    TunnelMessage::Command {
                        run_id,
                        method,
                        target,
                    } => match executor.spawn(&method, &target).await {
                        Ok(handle) => stream_run(channel, &run_id, handle).await?,
                        Err(reject) => {
                            channel
                                .send_message(&TunnelMessage::Error {
                                    run_id,
                                    message: reject.message(),
                                })
                                .await?
                        }
                    },
                    // An up-frame echoed back (or a stray heartbeat) is not ours to act
                    // on; ignore it and keep serving.
                    TunnelMessage::Heartbeat
                    | TunnelMessage::Output { .. }
                    | TunnelMessage::Done { .. }
                    | TunnelMessage::Error { .. } => {}
                }
            }
        }
    }
}

/// Drain one run's exec events into authenticated frames on the channel.
async fn stream_run<T: FrameTransport>(
    channel: &mut AuthChannel<T>,
    run_id: &str,
    mut handle: ExecHandle,
) -> Result<(), TunnelError> {
    while let Some(event) = handle.events.recv().await {
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
                channel
                    .send_message(&TunnelMessage::Error {
                        run_id: run_id.to_string(),
                        message,
                    })
                    .await?;
            }
            ExecEvent::Done { status, .. } => {
                let ok = matches!(status, ExecStatus::Completed { success: true });
                channel
                    .send_message(&TunnelMessage::Done {
                        run_id: run_id.to_string(),
                        ok,
                    })
                    .await?;
                break;
            }
        }
    }
    Ok(())
}

/// The reconnect loop: connect (pinning central every time), serve, and on any
/// disconnect wait a bounded backoff and reconnect. Never returns.
pub async fn run<E: CommandExecutor>(config: TunnelClientConfig, executor: E) {
    loop {
        let correlation_id = connection_correlation_id();
        match connect_once(&config, &executor, &correlation_id).await {
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
/// it runs on this connect and on every reconnect.
async fn connect_once<E: CommandExecutor>(
    config: &TunnelClientConfig,
    executor: &E,
    correlation_id: &str,
) -> Result<(), TunnelError> {
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

    let request = format!("wss://{}:{}/", config.host, config.port);
    let (ws, _response) = client_async(request, tls)
        .await
        .map_err(|error| TunnelError::Transport(std::io::Error::other(error.to_string())))?;

    let transport = WsTransport::new(ws);
    let mut channel = client_handshake(
        transport,
        &config.agent_id,
        &config.credential,
        random_nonce(),
    )
    .await?;
    log_agent_connect(correlation_id, config);
    serve_relay(&mut channel, executor).await
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
struct PinnedCentral {
    pinned_fingerprint: String,
    algorithms: WebPkiSupportedAlgorithms,
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
        verify_pinned_identity(end_entity.as_ref(), &self.pinned_fingerprint)
            .map(|()| ServerCertVerified::assertion())
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
struct WsTransport<S> {
    ws: WebSocketStream<S>,
}

impl<S> WsTransport<S> {
    fn new(ws: WebSocketStream<S>) -> Self {
        Self { ws }
    }
}

impl<S> FrameTransport for WsTransport<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn send(&mut self, frame: Vec<u8>) -> std::io::Result<()> {
        self.ws
            .send(Message::binary(frame))
            .await
            .map_err(|error| std::io::Error::other(format!("{error}")))
    }

    async fn recv(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        while let Some(message) = self.ws.next().await {
            match message.map_err(|error| std::io::Error::other(format!("{error}")))? {
                Message::Binary(bytes) => return Ok(Some(bytes.to_vec())),
                Message::Close(_) => return Ok(None),
                _ => continue,
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::exec::ExecLimits;
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
        async fn spawn(&self, _method: &str, _target: &str) -> Result<ExecHandle, RelayReject> {
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
        async fn spawn(&self, _method: &str, _target: &str) -> Result<ExecHandle, RelayReject> {
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

        let serving = tokio::spawn(async move { serve_relay(&mut agent_channel, &executor).await });

        central_channel
            .send_message(&TunnelMessage::Command {
                run_id: "r1".into(),
                method: "ping".into(),
                target: "8.8.8.8".into(),
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
            let _ = serve_relay(&mut agent_channel, &executor).await;
        });

        central_channel
            .send_message(&TunnelMessage::Command {
                run_id: "r1".into(),
                method: "ping".into(),
                target: "8.8.8.8".into(),
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
        let refused = executor.spawn("ping", "8.8.8.8").await.err();
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
        let rejected = executor.spawn("ping", "10.0.0.1").await.err();
        assert!(
            matches!(rejected, Some(RelayReject::Rejected(_))),
            "a private target must be rejected at the SSRF boundary, got {rejected:?}"
        );
    }

    // An unknown method name is refused — central cannot drive an unsupported method.
    #[tokio::test]
    async fn node_executor_refuses_an_unknown_method() {
        let executor = NodeExecutor::new(ExecEngine::new(ExecLimits::default()), StubResolver);
        assert!(matches!(
            executor.spawn("telnet", "8.8.8.8").await,
            Err(RelayReject::UnknownMethod(_))
        ));
    }

    // ----- Slice 11: BGP agent-side execution ---------------------------------

    struct StubProbe(Option<shared::template::BgpDaemon>);
    impl DaemonProbe for StubProbe {
        fn detect(&self) -> Option<shared::template::BgpDaemon> {
            self.0
        }
    }

    fn bgp_executor(daemon: Option<shared::template::BgpDaemon>) -> NodeExecutor<StubResolver> {
        NodeExecutor::new(ExecEngine::new(ExecLimits::default()), StubResolver)
            .with_daemon_probe(Arc::new(StubProbe(daemon)))
    }

    // AC32/AC37 agent leg: a valid BGP prefix with a detected daemon is dispatched
    // to the shared exec engine (a handle is returned — the read-only template is
    // asserted in shared::template). A private CIDR is accepted (no SSRF filter).
    #[tokio::test]
    async fn node_executor_runs_bgp_with_a_present_daemon() {
        let executor = bgp_executor(Some(shared::template::BgpDaemon::Bird));
        assert!(
            executor.spawn("bgp", "10.0.0.0/8").await.is_ok(),
            "a valid v4 prefix with a daemon present must dispatch to exec"
        );
        let frr = bgp_executor(Some(shared::template::BgpDaemon::Frr));
        assert!(frr.spawn("bgp6", "2001:db8::/32").await.is_ok());
    }

    // AC41 agent leg: BGP on a node with no supported daemon is refused with a clear
    // message, before any process is spawned — no hang.
    #[tokio::test]
    async fn node_executor_bgp_is_unavailable_without_a_daemon() {
        let executor = bgp_executor(None);
        match executor.spawn("bgp", "8.8.8.8").await {
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
            executor.spawn("bgp", "8.8.8.8; configure").await,
            Err(RelayReject::Rejected(_))
        ));
        // A v4 literal on the v6 bgp6 method is wrong-family, refused pre-daemon.
        assert!(matches!(
            executor.spawn("bgp6", "8.8.8.8").await,
            Err(RelayReject::Rejected(_))
        ));
        assert_eq!(
            engine.available_permits(),
            before,
            "a rejected BGP prefix must not spawn a process or hold a permit"
        );
    }

    #[test]
    fn host_port_parsing_handles_scheme_and_default_port() {
        assert_eq!(
            parse_host_port("https://central.example:9443/x"),
            ("central.example".to_string(), 9443)
        );
        assert_eq!(
            parse_host_port("central.example"),
            ("central.example".to_string(), 8443)
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
            let _ = serve_relay(&mut agent_channel, &executor).await;
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
