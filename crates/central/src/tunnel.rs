//! The agent↔central tunnel, central side (Slice 8) — the RCE surface.
//!
//! A **direct TLS listener**, separate from the HTTP web surface / proxy
//! (FR-071b), accepts an agent's outbound WebSocket connection. Central proves
//! the agent per-connection by verifying its credential against the stored
//! Argon2id hash (a deleted hash = revoked = refused, FR-024), then every frame
//! rides an [`AuthChannel`] so a wrong-credential or replayed frame is refused
//! per-frame, not just at the handshake. A relayed command runs agent-side and
//! its output streams back; if the agent drops mid-run, central emits a terminal
//! error to the run's consumer within a bounded time (AC41) rather than hanging.
//!
//! The per-frame auth mechanism (HMAC-SHA256 + HKDF + monotonic counter) lives in
//! [`shared::protocol`], written and proven once; this module is the central-side
//! transport + relay that rides it. Routing a visitor's SSE run to a connected
//! agent through [`TunnelHub`] is wired in Slice 10; this slice proves the relay.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use shared::protocol::{
    fingerprint, server_handshake, AuthChannel, FrameTransport, TunnelMessage, TUNNEL_KEY_BYTES,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::auth::verify_password;
use crate::observability::new_correlation_id;
use crate::store::Store;

const ENV_TUNNEL_BIND: &str = "LG_TUNNEL_BIND";
const ENV_TUNNEL_CERT: &str = "LG_TUNNEL_CERT";
const ENV_TUNNEL_KEY: &str = "LG_TUNNEL_KEY";
const DEFAULT_TUNNEL_BIND: &str = "0.0.0.0:8443";

const PREAUTH_TIMEOUT: Duration = Duration::from_secs(10);
const PREAUTH_MAX_CONCURRENT: usize = 64;
const PREAUTH_FAILURE_MAX: u32 = 20;
const PREAUTH_FAILURE_WINDOW: Duration = Duration::from_secs(60);
const PREAUTH_PRUNE_THRESHOLD: usize = 4096;

/// Backstop on inter-frame silence during a relayed run: if no frame arrives for
/// this long the connection is torn down so a silent agent can never hang the
/// consumer (AC41). It bounds the gap *between* frames, not total run duration —
/// the real run-duration bound is the agent's own exec deadline, which sends a
/// terminal frame far sooner on a healthy agent. This is not the Slice-8b liveness
/// window; it only catches an agent that stops responding without closing.
const RELAY_INTER_FRAME_TIMEOUT: Duration = Duration::from_secs(120);

/// Bound on the relay event channel — backpressure on a slow consumer.
const RELAY_EVENT_CAPACITY: usize = 64;

#[derive(Default)]
struct PreAuthFailures {
    windows: Mutex<HashMap<IpAddr, PreAuthWindow>>,
}

struct PreAuthWindow {
    count: u32,
    start: Instant,
}

impl PreAuthFailures {
    fn allow(&self, peer: IpAddr) -> bool {
        let now = Instant::now();
        let mut windows = self.windows.lock().expect("tunnel preauth limiter");
        if windows.len() > PREAUTH_PRUNE_THRESHOLD {
            windows.retain(|_, window| now.duration_since(window.start) < PREAUTH_FAILURE_WINDOW);
        }
        match windows.get(&peer) {
            Some(window) => {
                now.duration_since(window.start) >= PREAUTH_FAILURE_WINDOW
                    || window.count < PREAUTH_FAILURE_MAX
            }
            None => true,
        }
    }

    fn record_failure(&self, peer: IpAddr) {
        let now = Instant::now();
        let mut windows = self.windows.lock().expect("tunnel preauth limiter");
        let window = windows.entry(peer).or_insert(PreAuthWindow {
            count: 0,
            start: now,
        });
        if now.duration_since(window.start) >= PREAUTH_FAILURE_WINDOW {
            window.count = 0;
            window.start = now;
        }
        window.count += 1;
    }

    fn clear(&self, peer: IpAddr) {
        self.windows
            .lock()
            .expect("tunnel preauth limiter")
            .remove(&peer);
    }
}

/// What a relayed run surfaces to its consumer (the visitor's SSE stream, wired
/// in Slice 10). Exactly one [`RelayEvent::Terminal`] is emitted per run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayEvent {
    /// One line of the agent's output.
    Line(String),
    /// The run ended. `error` is `None` on success and carries a clear message on
    /// any failure — including the agent dropping mid-run (AC41).
    Terminal { error: Option<String> },
}

/// A relay request handed to a connected agent's serving task.
pub struct RelayJob {
    pub command: TunnelMessage,
    pub events: mpsc::Sender<RelayEvent>,
    busy: Arc<AtomicBool>,
}

/// The agent is not currently connected on the tunnel, so nothing can be relayed
/// to it.
#[derive(Debug)]
pub struct NotConnected;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    NotConnected,
    Busy,
}

/// One connected agent's entry in the hub: the channel its serving task drains
/// jobs from, tagged with the connection's generation. The generation makes
/// deregistration connection-scoped — an older connection ending cannot evict a
/// newer one that reused the same `agent_id`.
struct AgentEntry {
    generation: u64,
    jobs: mpsc::Sender<RelayJob>,
    shutdown: oneshot::Sender<()>,
    busy: Arc<AtomicBool>,
}

/// The registry of currently-connected, authenticated agents: `agent_id` → the
/// channel its serving task drains relay jobs from. Cheap to clone (shared map).
/// Slice 10's remote-run endpoint calls [`TunnelHub::submit`]; this slice fills
/// and drains it from the listener.
#[derive(Clone, Default)]
pub struct TunnelHub {
    agents: Arc<Mutex<HashMap<String, AgentEntry>>>,
    next_generation: Arc<AtomicU64>,
}

impl TunnelHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a connection under `agent_id`, returning the generation token the
    /// matching [`Self::unregister`] must present. A second connection for the same
    /// id supersedes the first in the map.
    fn register(
        &self,
        agent_id: &str,
        jobs: mpsc::Sender<RelayJob>,
        shutdown: oneshot::Sender<()>,
        busy: Arc<AtomicBool>,
    ) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        self.agents.lock().expect("tunnel hub mutex").insert(
            agent_id.to_string(),
            AgentEntry {
                generation,
                jobs,
                shutdown,
                busy,
            },
        );
        generation
    }

    /// Remove `agent_id` only if the mapped entry is still THIS connection
    /// (compare-and-remove by generation), so an older connection tearing down
    /// cannot deregister a newer live one that took its place.
    fn unregister(&self, agent_id: &str, generation: u64) {
        let mut agents = self.agents.lock().expect("tunnel hub mutex");
        if agents.get(agent_id).map(|entry| entry.generation) == Some(generation) {
            agents.remove(agent_id);
        }
    }

    /// Whether an agent is connected on the tunnel right now.
    pub fn is_connected(&self, agent_id: &str) -> bool {
        self.agents
            .lock()
            .expect("tunnel hub mutex")
            .contains_key(agent_id)
    }

    /// Drop live connections for revoked agents. Removing the only job sender
    /// wakes the serving task, which then drops the authenticated transport.
    pub fn kick_agents(&self, agent_ids: &[String]) -> usize {
        let mut agents = self.agents.lock().expect("tunnel hub mutex");
        agent_ids
            .iter()
            .filter(|id| {
                agents
                    .remove(id.as_str())
                    .map(|entry| {
                        let _ = entry.shutdown.send(());
                    })
                    .is_some()
            })
            .count()
    }

    /// Relay `command` down to the connected agent and return a receiver that
    /// streams its output back. Fails closed with [`NotConnected`] if the agent
    /// is not currently on the tunnel.
    pub async fn submit(
        &self,
        agent_id: &str,
        command: TunnelMessage,
    ) -> Result<mpsc::Receiver<RelayEvent>, SubmitError> {
        let entry = {
            self.agents
                .lock()
                .expect("tunnel hub mutex")
                .get(agent_id)
                .map(|entry| (entry.jobs.clone(), entry.busy.clone()))
        };
        let (jobs, busy) = entry.ok_or(SubmitError::NotConnected)?;
        if busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(SubmitError::Busy);
        }
        let (events_tx, events_rx) = mpsc::channel(RELAY_EVENT_CAPACITY);
        jobs.send(RelayJob {
            command,
            events: events_tx,
            busy: busy.clone(),
        })
        .await
        .map_err(|_| {
            busy.store(false, Ordering::Release);
            SubmitError::NotConnected
        })?;
        Ok(events_rx)
    }

    #[cfg(test)]
    pub(crate) fn register_for_test(&self, agent_id: &str) -> mpsc::Receiver<RelayJob> {
        let (jobs_tx, jobs_rx) = mpsc::channel::<RelayJob>(16);
        let (shutdown_tx, _shutdown_rx) = oneshot::channel();
        self.register(
            agent_id,
            jobs_tx,
            shutdown_tx,
            Arc::new(AtomicBool::new(false)),
        );
        jobs_rx
    }
}

/// Verify a presented credential against the agent's stored Argon2id hash. Fails
/// closed on an unknown or revoked agent (a deleted/absent hash is exactly how a
/// revoke lands — the reconnect handshake then fails) or any store error.
fn verify_agent(store: &Store, agent_id: &str, credential: &str) -> bool {
    match store.get_agent(agent_id) {
        Ok(Some(agent)) if !agent.revoked => verify_password(credential, &agent.credential_hash),
        _ => false,
    }
}

/// Whether the connection survives a relayed run or must be torn down. Because a
/// single ordered channel carries one run at a time, any early exit (consumer
/// gone, inter-frame backstop, channel error, or a foreign `run_id`) leaves the
/// channel in an unsafe state — reusing it would forward this run's leftover
/// frames as the next run's, and the agent would keep running an abandoned
/// process. So those cases tear the whole connection down; the agent reconnects
/// with a fresh session and its exec engine reaps the abandoned run.
enum ConnectionControl {
    KeepAlive,
    TearDown,
}

/// Records an authenticated agent's proof-of-life into the store as `last_seen`
/// (Slice 8b, compute-on-read). Any received up-frame — a heartbeat when idle, or a
/// run's output frame — is proof the agent is alive, so [`Self::touch`] is called on
/// every frame. Writes are throttled to at most one per second (unix-second
/// granularity) so a chatty run's output does not amplify into a write storm; the
/// derived-online window is 30s, so per-second granularity is ample. The store write
/// preserves `revoked`, so recording liveness never resurrects a revoked agent.
struct Liveness {
    store: Store,
    agent_id: String,
    last_written: u64,
}

impl Liveness {
    fn new(store: Store, agent_id: String) -> Self {
        Self {
            store,
            agent_id,
            last_written: 0,
        }
    }

    fn touch(&mut self) {
        let now = crate::store::unix_now();
        if now <= self.last_written {
            return;
        }
        self.last_written = now;
        if let Err(error) = self.store.touch_agent_last_seen(&self.agent_id, now) {
            // Liveness is best-effort telemetry; a failed write must not tear down a
            // healthy tunnel. Surface it, keep serving.
            tracing::warn!(agent_id = %self.agent_id, %error, "failed to record agent liveness");
        }
    }

    fn is_revoked_or_missing(&self) -> bool {
        match self.store.get_agent(&self.agent_id) {
            Ok(Some(agent)) => agent.revoked,
            _ => true,
        }
    }
}

/// Relay one command down an authenticated channel and stream the agent's output
/// back as [`RelayEvent`]s. Emits exactly one [`RelayEvent::Terminal`]. Returns
/// [`ConnectionControl::KeepAlive`] only on a clean run-terminal (the agent's
/// `Done`/`Error`); every early exit returns [`ConnectionControl::TearDown`].
async fn relay_run<T: FrameTransport>(
    channel: &mut AuthChannel<T>,
    command: TunnelMessage,
    events: mpsc::Sender<RelayEvent>,
    deadline: Duration,
    liveness: &mut Liveness,
    shutdown: &mut oneshot::Receiver<()>,
) -> ConnectionControl {
    let active_run = command.run_id().unwrap_or_default().to_string();
    if liveness.is_revoked_or_missing() {
        let _ = events
            .send(RelayEvent::Terminal {
                error: Some("the remote agent was revoked".to_string()),
            })
            .await;
        return ConnectionControl::TearDown;
    }
    tokio::select! {
        biased;
        _ = &mut *shutdown => {
            let _ = events
                .send(RelayEvent::Terminal {
                    error: Some("the remote agent was revoked".to_string()),
                })
                .await;
            return ConnectionControl::TearDown;
        }
        sent = channel.send_message(&command) => {
            if sent.is_err() {
                let _ = events
                    .send(RelayEvent::Terminal {
                        error: Some("the remote node dropped the connection".to_string()),
                    })
                    .await;
                return ConnectionControl::TearDown;
            }
        }
    }
    loop {
        let received = tokio::select! {
            biased;
            _ = &mut *shutdown => {
                let _ = events
                    .send(RelayEvent::Terminal {
                        error: Some("the remote agent was revoked".to_string()),
                    })
                    .await;
                return ConnectionControl::TearDown;
            }
            received = tokio::time::timeout(deadline, channel.recv_message()) => received,
        };
        let message = match received {
            Err(_elapsed) => {
                let _ = events
                    .send(RelayEvent::Terminal {
                        error: Some("the remote node did not respond in time".to_string()),
                    })
                    .await;
                return ConnectionControl::TearDown;
            }
            Ok(Ok(message)) => message,
            Ok(Err(_channel_error)) => {
                // Bad tag / replay / closed transport: the agent dropped or the
                // channel is compromised. Surface a terminal error (AC41) and tear
                // the tunnel down — never continue past an auth failure.
                let _ = events
                    .send(RelayEvent::Terminal {
                        error: Some("the remote node dropped the connection".to_string()),
                    })
                    .await;
                return ConnectionControl::TearDown;
            }
        };

        if liveness.is_revoked_or_missing() {
            let _ = events
                .send(RelayEvent::Terminal {
                    error: Some("the remote agent was revoked".to_string()),
                })
                .await;
            return ConnectionControl::TearDown;
        }

        // Any authenticated frame received during a run is proof the agent is alive.
        liveness.touch();

        // Correlate every run-bearing frame with the active run: a frame for
        // another run is a protocol violation, never forwarded as this run's output.
        if let Some(frame_run) = message.run_id() {
            if frame_run != active_run {
                let _ = events
                    .send(RelayEvent::Terminal {
                        error: Some("the remote node sent a frame for a different run".to_string()),
                    })
                    .await;
                return ConnectionControl::TearDown;
            }
        }

        match message {
            TunnelMessage::Output { line, .. } => {
                let sent = tokio::select! {
                    biased;
                    _ = &mut *shutdown => {
                        return ConnectionControl::TearDown;
                    }
                    sent = events.send(RelayEvent::Line(line)) => sent,
                };
                if sent.is_err() {
                    // The consumer went away mid-stream: tear the connection down so
                    // the agent's abandoned process is reaped and no leftover frame
                    // bleeds into the next run on this ordered channel.
                    return ConnectionControl::TearDown;
                }
            }
            TunnelMessage::Done { ok, .. } => {
                let error = (!ok).then(|| "the diagnostic finished with an error".to_string());
                let _ = events.send(RelayEvent::Terminal { error }).await;
                return ConnectionControl::KeepAlive;
            }
            TunnelMessage::Error { message, .. } => {
                let _ = events
                    .send(RelayEvent::Terminal {
                        error: Some(message),
                    })
                    .await;
                return ConnectionControl::KeepAlive;
            }
            // A heartbeat (Slice 8b) or an unexpected up-frame is ignored, not fatal.
            TunnelMessage::Heartbeat | TunnelMessage::Command { .. } => {}
        }
    }
}

/// Serve one authenticated agent connection: register it in the hub, relay each
/// submitted job over the channel one at a time, and unregister on exit. Any run
/// that reports [`ConnectionControl::TearDown`] ends the connection and drops the
/// channel (closing the WebSocket), so the agent reconnects with a fresh session.
async fn serve_agent<T: FrameTransport>(
    mut channel: AuthChannel<T>,
    hub: TunnelHub,
    store: Store,
    agent_id: String,
) {
    let correlation_id = new_correlation_id();
    let (job_tx, mut job_rx) = mpsc::channel::<RelayJob>(16);
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let generation = hub.register(
        &agent_id,
        job_tx,
        shutdown_tx,
        Arc::new(AtomicBool::new(false)),
    );
    let mut liveness = Liveness::new(store, agent_id.clone());
    // A completed handshake is itself proof of life: the location goes online on
    // dial-home, before its first heartbeat arrives (AC7 online half).
    liveness.touch();
    tracing::info!(
        event = "agent.connect",
        correlation_id = %correlation_id,
        agent_id = %agent_id,
        outcome = "connected",
        "agent connected"
    );

    // When idle, the serving task must still read the channel so the agent's
    // heartbeats advance last_seen — otherwise a live but idle agent would derive
    // offline after the window. A single ordered channel carries one run at a time,
    // so relaying and idle-reading never overlap: while a job relays, `relay_run`
    // owns the reads (and touches liveness itself); between jobs, this select reads
    // heartbeats. Both branches only cancel their loser while it is Pending, so no
    // partially-read frame is lost.
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown_rx => {
                break;
            }
            job = job_rx.recv() => {
                let Some(job) = job else { break };
                let control = relay_run(
                    &mut channel,
                    job.command,
                    job.events,
                    RELAY_INTER_FRAME_TIMEOUT,
                    &mut liveness,
                    &mut shutdown_rx,
                )
                .await;
                match control {
                    ConnectionControl::KeepAlive => {
                        job.busy.store(false, Ordering::Release);
                    }
                    ConnectionControl::TearDown => break,
                }
            }
            frame = channel.recv_message() => {
                match frame {
                    // An idle up-frame (a heartbeat) is proof of life, nothing more.
                    Ok(_message) => {
                        if liveness.is_revoked_or_missing() {
                            break;
                        }
                        liveness.touch();
                    }
                    // A closed / forged / replayed frame ends the connection — fail
                    // closed, never continue past an auth failure.
                    Err(_error) => break,
                }
            }
        }
    }
    hub.unregister(&agent_id, generation);
    tracing::info!(
        event = "agent.disconnect",
        correlation_id = %correlation_id,
        agent_id = %agent_id,
        outcome = "disconnected",
        "agent disconnected"
    );
}

/// Central's TLS identity for the tunnel: the certificate chain + private key the
/// listener presents, and the SHA-256 fingerprint of the end-entity certificate —
/// the value the install command carries and the agent pins (Slice 7).
pub struct TunnelIdentity {
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    fingerprint: String,
}

impl TunnelIdentity {
    /// Load the tunnel certificate + key from the PEM paths in the environment,
    /// or `None` if they are unset or fail to load. An absent identity disables
    /// the tunnel listener with a warning (fail-safe, not fail-open) — the same
    /// posture as the Slice-7 ephemeral identity.
    pub fn from_env() -> Option<Self> {
        let cert_path = std::env::var(ENV_TUNNEL_CERT).unwrap_or_default();
        let key_path = std::env::var(ENV_TUNNEL_KEY).unwrap_or_default();
        if cert_path.is_empty() || key_path.is_empty() {
            tracing::warn!(
                "{ENV_TUNNEL_CERT}/{ENV_TUNNEL_KEY} not set — agent tunnel listener DISABLED; \
                 no remote agents can connect until a tunnel certificate is configured"
            );
            return None;
        }
        match Self::load(&cert_path, &key_path) {
            Ok(identity) => Some(identity),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "failed to load tunnel TLS identity — agent tunnel listener DISABLED; \
                     no remote agents can connect until a valid certificate is configured"
                );
                None
            }
        }
    }

    fn load(cert_path: &str, key_path: &str) -> io::Result<Self> {
        let certs = CertificateDer::pem_file_iter(cert_path)
            .map_err(pem_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(pem_error)?;
        let key = PrivateKeyDer::from_pem_file(key_path).map_err(pem_error)?;
        let end_entity = certs
            .first()
            .ok_or_else(|| io::Error::other("tunnel certificate file contains no certificate"))?;
        let fingerprint = fingerprint(end_entity.as_ref());
        Ok(Self {
            certs,
            key,
            fingerprint,
        })
    }

    /// The fingerprint the install command embeds and the agent pins.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

fn pem_error(error: rustls::pki_types::pem::Error) -> io::Error {
    io::Error::other(format!("{error:?}"))
}

/// The bound tunnel listener address (`LG_TUNNEL_BIND`, default `0.0.0.0:8443`).
pub fn bind_addr() -> SocketAddr {
    std::env::var(ENV_TUNNEL_BIND)
        .unwrap_or_else(|_| DEFAULT_TUNNEL_BIND.to_string())
        .parse()
        .unwrap_or_else(|_| {
            DEFAULT_TUNNEL_BIND
                .parse()
                .expect("default bind addr parses")
        })
}

/// Run the direct TLS tunnel listener: accept outbound agent connections, TLS +
/// WebSocket + authenticate each, and serve it. Separate from the HTTP surface
/// (its own socket), so the tunnel is end-to-end TLS and never behind the web
/// proxy (FR-071b).
pub async fn serve(
    bind: SocketAddr,
    identity: TunnelIdentity,
    store: Store,
    hub: TunnelHub,
) -> io::Result<()> {
    // Log the pinned fingerprint so an operator can confirm it matches the value
    // baked into the install command (the agent verifies central against it).
    tracing::info!(
        %bind,
        fingerprint = %identity.fingerprint(),
        "agent tunnel listener up (direct TLS, separate from the web surface)"
    );
    let acceptor = build_acceptor(identity)?;
    let listener = TcpListener::bind(bind).await?;
    let preauth_permits = Arc::new(Semaphore::new(PREAUTH_MAX_CONCURRENT));
    let preauth_failures = Arc::new(PreAuthFailures::default());
    loop {
        let permit = acquire_preauth_permit(preauth_permits.clone()).await?;
        let (tcp, peer, permit) = accept_permitted_socket(&listener, permit).await?;
        let acceptor = acceptor.clone();
        let store = store.clone();
        let hub = hub.clone();
        let preauth_failures = preauth_failures.clone();
        tokio::spawn(async move {
            if !preauth_failures.allow(peer.ip()) {
                tracing::info!(%peer, "agent tunnel pre-auth rate limit exceeded");
                return;
            }
            match handle_connection(acceptor, tcp, store, hub, permit).await {
                Ok(()) => preauth_failures.clear(peer.ip()),
                Err(error) => {
                    preauth_failures.record_failure(peer.ip());
                    // Expected on a failed handshake / dropped agent — info, not error.
                    tracing::info!(%peer, %error, "agent tunnel connection ended");
                }
            }
        });
    }
}

async fn acquire_preauth_permit(permits: Arc<Semaphore>) -> io::Result<OwnedSemaphorePermit> {
    permits
        .acquire_owned()
        .await
        .map_err(|_| io::Error::other("agent tunnel pre-auth limiter closed"))
}

async fn accept_permitted_socket(
    listener: &TcpListener,
    permit: OwnedSemaphorePermit,
) -> io::Result<(tokio::net::TcpStream, SocketAddr, OwnedSemaphorePermit)> {
    let (tcp, peer) = listener.accept().await?;
    Ok((tcp, peer, permit))
}

async fn preauth_timeout<T>(
    future: impl std::future::Future<Output = io::Result<T>>,
    phase: &str,
) -> io::Result<T> {
    tokio::time::timeout(PREAUTH_TIMEOUT, future)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, format!("{phase} timed out")))?
}

async fn ws_preauth_timeout<T, E>(
    future: impl std::future::Future<Output = Result<T, E>>,
    phase: &str,
) -> io::Result<T>
where
    E: std::fmt::Display,
{
    tokio::time::timeout(PREAUTH_TIMEOUT, future)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, format!("{phase} timed out")))?
        .map_err(|error| io::Error::other(format!("{phase} failed: {error}")))
}

fn build_acceptor(identity: TunnelIdentity) -> io::Result<TlsAcceptor> {
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|error| io::Error::other(format!("{error:?}")))?
            .with_no_client_auth()
            .with_single_cert(identity.certs, identity.key)
            .map_err(|error| io::Error::other(format!("{error:?}")))?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

async fn handle_connection(
    acceptor: TlsAcceptor,
    tcp: tokio::net::TcpStream,
    store: Store,
    hub: TunnelHub,
    permit: OwnedSemaphorePermit,
) -> io::Result<()> {
    let tls = preauth_timeout(acceptor.accept(tcp), "tls handshake").await?;
    let ws =
        ws_preauth_timeout(tokio_tungstenite::accept_async(tls), "websocket handshake").await?;
    let transport = WsTransport::new(ws);

    let (agent_id, channel) = ws_preauth_timeout(
        server_handshake(transport, random_nonce(), |agent_id, credential| {
            let store = store.clone();
            async move { verify_agent(&store, &agent_id, &credential) }
        }),
        "agent credential handshake",
    )
    .await?;
    drop(permit);

    serve_agent(channel, hub, store, agent_id).await;
    Ok(())
}

fn random_nonce() -> [u8; TUNNEL_KEY_BYTES] {
    let mut nonce = [0u8; TUNNEL_KEY_BYTES];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut nonce)
        .expect("system CSPRNG must be available");
    nonce
}

/// A [`FrameTransport`] over a WebSocket: one binary message per frame. Ping/pong
/// are handled by tungstenite; a close (or a text frame) ends the stream.
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
    async fn send(&mut self, frame: Vec<u8>) -> io::Result<()> {
        self.ws
            .send(Message::binary(frame))
            .await
            .map_err(|error| io::Error::other(format!("{error}")))
    }

    async fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        while let Some(message) = self.ws.next().await {
            match message.map_err(|error| io::Error::other(format!("{error}")))? {
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
    use shared::protocol::{client_handshake, ChannelTransport};
    use std::sync::OnceLock;

    const CRED: &str = "aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44";
    static LOG_CAPTURE: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();

    fn captured_logs() -> Arc<Mutex<Vec<u8>>> {
        LOG_CAPTURE
            .get_or_init(|| {
                let buffer = Arc::new(Mutex::new(Vec::new()));
                let writer_buffer = Arc::clone(&buffer);
                let subscriber = tracing_subscriber::fmt()
                    .with_ansi(false)
                    .with_writer(move || CaptureWriter(Arc::clone(&writer_buffer)))
                    .finish();
                let _ = tracing::subscriber::set_global_default(subscriber);
                buffer
            })
            .clone()
    }

    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn contains_field(logs: &str, name: &str, value: &str) -> bool {
        logs.contains(&format!("{name}={value}")) || logs.contains(&format!("{name}=\"{value}\""))
    }

    #[test]
    fn preauth_failures_rate_limit_and_clear_by_peer() {
        let limiter = PreAuthFailures::default();
        let peer = "198.51.100.7".parse().unwrap();
        for _ in 0..PREAUTH_FAILURE_MAX {
            assert!(limiter.allow(peer));
            limiter.record_failure(peer);
        }
        assert!(!limiter.allow(peer));
        limiter.clear(peer);
        assert!(limiter.allow(peer));
    }

    #[tokio::test]
    async fn preauth_accept_waits_for_a_permit_before_taking_a_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let peer = listener.local_addr().unwrap();
        let permits = Arc::new(Semaphore::new(0));
        let permit = acquire_preauth_permit(permits.clone());
        tokio::pin!(permit);

        let client = tokio::net::TcpStream::connect(peer).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut permit)
                .await
                .is_err(),
            "a waiting socket must not start accept/handshake work before a pre-auth permit exists"
        );

        permits.add_permits(1);
        let permit = tokio::time::timeout(Duration::from_secs(1), &mut permit)
            .await
            .expect("permit should become available")
            .expect("pre-auth permit");
        let (accepted, accepted_peer, _permit) = tokio::time::timeout(
            Duration::from_secs(1),
            accept_permitted_socket(&listener, permit),
        )
        .await
        .expect("accept should continue once a pre-auth permit is available")
        .expect("accept with permit");
        assert_eq!(accepted_peer.ip(), client.local_addr().unwrap().ip());
        drop((client, accepted, accepted_peer));
    }

    /// Establish an authenticated central↔"agent" channel pair over an in-memory
    /// transport: `central_channel` is the real central side; `agent_channel` is
    /// the test playing the agent.
    async fn established_pair() -> (AuthChannel<ChannelTransport>, AuthChannel<ChannelTransport>) {
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
        (central_channel, agent_channel)
    }

    fn command(run_id: &str) -> TunnelMessage {
        TunnelMessage::Command {
            run_id: run_id.to_string(),
            method: "ping".into(),
            target: "8.8.8.8".into(),
        }
    }

    async fn wait_for<F: Fn() -> bool>(predicate: F) {
        for _ in 0..100 {
            if predicate() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn tunnel_connect_and_disconnect_logs_are_correlated_and_secret_free() {
        let logs = captured_logs();
        let (central_channel, agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");

        drop(agent_channel);
        serve_agent(central_channel, hub, store, "agent-1".to_string()).await;

        let captured = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        assert!(
            contains_field(&captured, "event", "agent.connect"),
            "{captured}"
        );
        assert!(
            contains_field(&captured, "event", "agent.disconnect"),
            "{captured}"
        );
        assert!(captured.contains("correlation_id="), "{captured}");
        assert!(
            !captured.contains(CRED),
            "credential leaked into tunnel logs"
        );
    }

    // AC7 (central relay half): a command submitted through the hub is relayed
    // down the authenticated channel, the agent's streamed output comes back up,
    // and the run terminates cleanly.
    #[tokio::test]
    async fn hub_relays_a_command_and_streams_output_back() {
        let (central_channel, mut agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");

        let serving = {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            })
        };

        // Wait for the serving task to register the agent.
        for _ in 0..50 {
            if hub.is_connected("agent-1") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let mut events = hub
            .submit(
                "agent-1",
                TunnelMessage::Command {
                    run_id: "r1".into(),
                    method: "ping".into(),
                    target: "8.8.8.8".into(),
                },
            )
            .await
            .expect("agent is connected");

        // The test's "agent" receives the relayed command and streams a reply.
        let received = agent_channel.recv_message().await.unwrap();
        assert_eq!(
            received,
            TunnelMessage::Command {
                run_id: "r1".into(),
                method: "ping".into(),
                target: "8.8.8.8".into()
            }
        );
        agent_channel
            .send_message(&TunnelMessage::Output {
                run_id: "r1".into(),
                line: "64 bytes from 8.8.8.8".into(),
            })
            .await
            .unwrap();
        agent_channel
            .send_message(&TunnelMessage::Done {
                run_id: "r1".into(),
                ok: true,
            })
            .await
            .unwrap();

        assert_eq!(
            events.recv().await,
            Some(RelayEvent::Line("64 bytes from 8.8.8.8".into()))
        );
        assert_eq!(
            events.recv().await,
            Some(RelayEvent::Terminal { error: None })
        );
        // The connection correctly stays open for the next job after a clean run;
        // end the test by cancelling the serving task rather than awaiting it.
        serving.abort();
    }

    // AC41: the agent dropping mid-run surfaces a terminal error to the run's
    // consumer within a bounded time — never a silent hang.
    #[tokio::test]
    async fn agent_drop_mid_run_emits_a_terminal_error() {
        let (central_channel, mut agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }
        for _ in 0..50 {
            if hub.is_connected("agent-1") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let mut events = hub
            .submit(
                "agent-1",
                TunnelMessage::Command {
                    run_id: "r1".into(),
                    method: "ping".into(),
                    target: "8.8.8.8".into(),
                },
            )
            .await
            .expect("agent is connected");

        // The agent receives the command, then drops mid-run without a Done.
        let _ = agent_channel.recv_message().await.unwrap();
        drop(agent_channel);

        let terminal = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("a terminal event must arrive well within the bound — no hang")
            .expect("terminal event present");
        assert!(
            matches!(terminal, RelayEvent::Terminal { error: Some(_) }),
            "an agent drop must surface a terminal error, got {terminal:?}"
        );

        // The agent is unregistered once its connection tears down.
        for _ in 0..50 {
            if !hub.is_connected("agent-1") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            !hub.is_connected("agent-1"),
            "a dropped agent is unregistered"
        );
    }

    // FR-024 / revoke: central refuses the handshake when the agent's at-rest
    // credential hash is gone (a deleted hash = revoked). No channel is issued.
    #[tokio::test]
    async fn handshake_is_refused_when_the_credential_hash_is_missing() {
        let store = Store::open(unique_db_path()).unwrap();
        let (agent_side, central_side) = ChannelTransport::pair();
        let client =
            tokio::spawn(
                async move { client_handshake(agent_side, "ghost", CRED, [1u8; 32]).await },
            );

        // The store has no agent "ghost" → verify_agent fails closed.
        let result = server_handshake(central_side, [2u8; 32], |id, cred| {
            let store = store.clone();
            async move { verify_agent(&store, &id, &cred) }
        })
        .await;

        assert!(result.is_err(), "an unknown/revoked agent must be refused");
        assert!(
            client.await.unwrap().is_err(),
            "the agent's handshake fails closed too"
        );
    }

    // Finding 1 (a): when a run's consumer disconnects mid-stream, the connection
    // is reset rather than reused — the poisoned ordered channel never carries a
    // later run, so no leftover frame can bleed across runs.
    #[tokio::test]
    async fn a_consumer_drop_mid_stream_resets_the_connection() {
        let (central_channel, mut agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }
        wait_for(|| hub.is_connected("agent-1")).await;

        let events = hub
            .submit("agent-1", command("r1"))
            .await
            .expect("connected");
        let _ = agent_channel.recv_message().await.unwrap(); // Command r1

        // The visitor closes the SSE stream mid-run; the agent keeps producing.
        drop(events);
        agent_channel
            .send_message(&TunnelMessage::Output {
                run_id: "r1".into(),
                line: "leftover".into(),
            })
            .await
            .unwrap();

        // The connection is torn down and deregistered — never reused for a next run.
        wait_for(|| !hub.is_connected("agent-1")).await;
        assert!(
            !hub.is_connected("agent-1"),
            "the connection must reset, not be reused"
        );
        assert!(
            hub.submit("agent-1", command("r2")).await.is_err(),
            "a torn-down agent accepts no new run on the poisoned channel"
        );
    }

    // Finding 1 (defense-in-depth): a frame carrying a foreign run_id is a protocol
    // violation — surfaced as a terminal error, never forwarded as this run's
    // output — and the connection is reset.
    #[tokio::test]
    async fn a_frame_for_a_foreign_run_is_rejected_not_forwarded() {
        let (central_channel, mut agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }
        wait_for(|| hub.is_connected("agent-1")).await;

        let mut events = hub
            .submit("agent-1", command("r2"))
            .await
            .expect("connected");
        let _ = agent_channel.recv_message().await.unwrap(); // Command r2
        agent_channel
            .send_message(&TunnelMessage::Output {
                run_id: "r1-stale".into(),
                line: "stale-secret".into(),
            })
            .await
            .unwrap();

        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("no hang")
            .expect("an event");
        assert!(
            matches!(event, RelayEvent::Terminal { error: Some(_) }),
            "a foreign-run frame must surface as a terminal error, got {event:?}"
        );
        assert_ne!(
            event,
            RelayEvent::Line("stale-secret".to_string()),
            "the stale frame must never be forwarded as this run's output"
        );
        wait_for(|| !hub.is_connected("agent-1")).await;
        assert!(
            !hub.is_connected("agent-1"),
            "a protocol violation resets the connection"
        );
    }

    // Finding 3: a second connection for one agent_id supersedes the first; when
    // the older connection ends, its generation-scoped unregister must NOT evict
    // the newer, live entry.
    #[tokio::test]
    async fn an_older_connection_ending_does_not_evict_a_newer_one() {
        let hub = TunnelHub::new();
        let (tx_a, _rx_a) = mpsc::channel::<RelayJob>(1);
        let (shutdown_a, _shutdown_rx_a) = oneshot::channel();
        let gen_a = hub.register(
            "agent-1",
            tx_a,
            shutdown_a,
            Arc::new(AtomicBool::new(false)),
        );
        let (tx_b, mut rx_b) = mpsc::channel::<RelayJob>(1);
        let (shutdown_b, _shutdown_rx_b) = oneshot::channel();
        let _gen_b = hub.register(
            "agent-1",
            tx_b,
            shutdown_b,
            Arc::new(AtomicBool::new(false)),
        ); // connection B supersedes A

        hub.unregister("agent-1", gen_a); // the OLDER connection ends

        assert!(
            hub.is_connected("agent-1"),
            "the newer connection B must remain registered"
        );
        // Prove the live entry is B: a submitted job routes to B's channel.
        let _events = hub
            .submit("agent-1", command("r1"))
            .await
            .expect("B is live");
        assert!(
            rx_b.recv().await.is_some(),
            "the job must route to the newer connection B, not the evicted A"
        );
    }

    fn enrolled_agent(store: &Store, id: &str) {
        store
            .put_agent(&crate::store::Agent {
                id: id.to_string(),
                location_id: "loc-1".to_string(),
                credential_hash: "$argon2id$stub".to_string(),
                enrolled_at: 0,
                last_seen: None,
                revoked: false,
            })
            .unwrap();
    }

    // AC7 (central online half): a completed handshake marks the agent alive
    // immediately — dial-home brings the location online before its first heartbeat.
    #[tokio::test]
    async fn a_connected_agent_is_marked_alive_on_dial_home() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        assert!(
            store
                .get_agent("agent-1")
                .unwrap()
                .unwrap()
                .last_seen
                .is_none(),
            "not yet seen before connecting"
        );

        let (central_channel, _agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }

        wait_for(|| {
            store
                .get_agent("agent-1")
                .unwrap()
                .unwrap()
                .last_seen
                .is_some()
        })
        .await;
        assert!(
            store
                .get_agent("agent-1")
                .unwrap()
                .unwrap()
                .last_seen
                .is_some(),
            "dial-home records proof of life"
        );
        // Keep the agent channel dropped at end so the serving task winds down.
    }

    // Slice 8b: a heartbeat sent up the idle channel is consumed as proof of life
    // (last_seen recorded) and does NOT tear the connection down — the agent stays
    // registered and serviceable.
    #[tokio::test]
    async fn an_idle_heartbeat_keeps_the_agent_alive_and_connected() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let (central_channel, mut agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }
        wait_for(|| hub.is_connected("agent-1")).await;

        // The agent beats up the idle channel; central records it and stays connected.
        agent_channel
            .send_message(&TunnelMessage::Heartbeat)
            .await
            .unwrap();
        wait_for(|| {
            store
                .get_agent("agent-1")
                .unwrap()
                .unwrap()
                .last_seen
                .is_some()
        })
        .await;
        assert!(
            store
                .get_agent("agent-1")
                .unwrap()
                .unwrap()
                .last_seen
                .is_some(),
            "an idle heartbeat advances last_seen"
        );
        // The heartbeat is not fatal: the agent is still connected afterwards.
        assert!(
            hub.is_connected("agent-1"),
            "a heartbeat keeps the connection, it does not tear it down"
        );
        // A subsequent relayed job still routes to the live connection.
        assert!(hub.submit("agent-1", command("r1")).await.is_ok());
    }

    #[tokio::test]
    async fn a_revoked_live_agent_is_disconnected_on_its_next_frame() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let (central_channel, mut agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }
        wait_for(|| hub.is_connected("agent-1")).await;

        assert_eq!(
            store.revoke_agents_for_location("loc-1").unwrap(),
            vec!["agent-1".to_string()]
        );
        agent_channel
            .send_message(&TunnelMessage::Heartbeat)
            .await
            .unwrap();

        wait_for(|| !hub.is_connected("agent-1")).await;
        assert!(
            !hub.is_connected("agent-1"),
            "a revoked credential must be refused on the next authenticated frame"
        );
        assert!(
            hub.submit("agent-1", command("r1")).await.is_err(),
            "a revoked live tunnel accepts no new work"
        );
    }

    #[tokio::test]
    async fn a_stale_sender_cannot_deliver_work_after_revoke() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let (central_channel, mut agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }
        wait_for(|| hub.is_connected("agent-1")).await;
        let stale_jobs = hub
            .agents
            .lock()
            .expect("tunnel hub mutex")
            .get("agent-1")
            .expect("registered agent")
            .jobs
            .clone();

        let revoked = store.revoke_agents_for_location("loc-1").unwrap();
        assert_eq!(hub.kick_agents(&revoked), 1);
        let (events_tx, mut events_rx) = mpsc::channel(RELAY_EVENT_CAPACITY);
        stale_jobs
            .send(RelayJob {
                command: command("r-stale"),
                events: events_tx,
                busy: Arc::new(AtomicBool::new(true)),
            })
            .await
            .expect("the cloned sender still exists");

        if let Ok(Ok(message)) =
            tokio::time::timeout(Duration::from_millis(100), agent_channel.recv_message()).await
        {
            panic!("stale sender delivered post-revoke work to the agent: {message:?}");
        }
        if let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(100), events_rx.recv()).await
        {
            panic!("stale revoked work was accepted: {event:?}");
        }
    }

    #[tokio::test]
    async fn kicking_a_live_agent_drops_the_registered_tunnel() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let (central_channel, _agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }
        wait_for(|| hub.is_connected("agent-1")).await;

        let revoked = store.revoke_agents_for_location("loc-1").unwrap();
        assert_eq!(hub.kick_agents(&revoked), 1);

        wait_for(|| !hub.is_connected("agent-1")).await;
        assert!(
            !hub.is_connected("agent-1"),
            "admin revoke must tear down an already-registered tunnel"
        );
    }

    #[tokio::test]
    async fn kicking_a_live_agent_aborts_even_with_a_stale_sender_clone() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let (central_channel, mut agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }
        wait_for(|| hub.is_connected("agent-1")).await;
        let stale_jobs = hub
            .agents
            .lock()
            .expect("tunnel hub mutex")
            .get("agent-1")
            .expect("registered agent")
            .jobs
            .clone();

        assert_eq!(hub.kick_agents(&["agent-1".to_string()]), 1);
        let (events_tx, _events_rx) = mpsc::channel(RELAY_EVENT_CAPACITY);
        let _ = stale_jobs
            .send(RelayJob {
                command: command("r-stale"),
                events: events_tx,
                busy: Arc::new(AtomicBool::new(true)),
            })
            .await;

        if let Ok(Ok(message)) =
            tokio::time::timeout(Duration::from_millis(100), agent_channel.recv_message()).await
        {
            panic!("stale sender delivered work after kick: {message:?}");
        }
        wait_for(|| !hub.is_connected("agent-1")).await;
        assert!(
            !hub.is_connected("agent-1"),
            "a kicked tunnel must close even when a stale sender clone exists"
        );
    }

    #[tokio::test]
    async fn kicking_an_agent_aborts_an_active_relay_without_waiting_for_timeout() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let (central_channel, mut agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }
        wait_for(|| hub.is_connected("agent-1")).await;

        let mut events = hub
            .submit("agent-1", command("r-active"))
            .await
            .expect("connected");
        let received = agent_channel.recv_message().await.unwrap();
        assert_eq!(received, command("r-active"));

        assert_eq!(hub.kick_agents(&["agent-1".to_string()]), 1);

        let terminal = tokio::time::timeout(Duration::from_millis(200), events.recv())
            .await
            .expect("active relay must close on revoke without waiting for the inter-frame timeout")
            .expect("terminal event");
        assert!(
            matches!(terminal, RelayEvent::Terminal { error: Some(_) }),
            "revoking an active relay must surface a terminal error, got {terminal:?}"
        );
        wait_for(|| !hub.is_connected("agent-1")).await;
        assert!(
            !hub.is_connected("agent-1"),
            "a kicked active relay must unregister the tunnel"
        );
    }

    #[tokio::test]
    async fn kicking_an_agent_aborts_when_relay_output_is_backpressured() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let (central_channel, mut agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }
        wait_for(|| hub.is_connected("agent-1")).await;

        let events = hub
            .submit("agent-1", command("r-backpressure"))
            .await
            .expect("connected");
        let received = agent_channel.recv_message().await.unwrap();
        assert_eq!(received, command("r-backpressure"));

        for n in 0..RELAY_EVENT_CAPACITY {
            agent_channel
                .send_message(&TunnelMessage::Output {
                    run_id: "r-backpressure".into(),
                    line: format!("line {n}"),
                })
                .await
                .unwrap();
        }
        wait_for(|| events.len() == RELAY_EVENT_CAPACITY).await;
        assert_eq!(
            events.len(),
            RELAY_EVENT_CAPACITY,
            "the visitor event channel is full"
        );

        agent_channel
            .send_message(&TunnelMessage::Output {
                run_id: "r-backpressure".into(),
                line: "blocked line".into(),
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(hub.kick_agents(&["agent-1".to_string()]), 1);

        wait_for(|| !hub.is_connected("agent-1")).await;
        assert!(
            !hub.is_connected("agent-1"),
            "a kicked relay blocked on visitor backpressure must tear down promptly"
        );
        drop(events);
    }

    #[tokio::test]
    async fn relay_run_shutdown_wins_while_line_send_is_backpressured() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let (mut central_channel, mut agent_channel) = established_pair().await;
        let mut liveness = Liveness::new(store, "agent-1".to_string());
        let (events_tx, events_rx) = mpsc::channel(1);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let relay = tokio::spawn(async move {
            relay_run(
                &mut central_channel,
                command("r-backpressure"),
                events_tx,
                RELAY_INTER_FRAME_TIMEOUT,
                &mut liveness,
                &mut shutdown_rx,
            )
            .await
        });

        assert_eq!(
            agent_channel.recv_message().await.unwrap(),
            command("r-backpressure")
        );
        agent_channel
            .send_message(&TunnelMessage::Output {
                run_id: "r-backpressure".into(),
                line: "first".into(),
            })
            .await
            .unwrap();
        wait_for(|| events_rx.len() == 1).await;
        assert_eq!(events_rx.len(), 1, "the visitor channel is full");

        agent_channel
            .send_message(&TunnelMessage::Output {
                run_id: "r-backpressure".into(),
                line: "blocked".into(),
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let _ = shutdown_tx.send(());

        let control = tokio::time::timeout(Duration::from_millis(200), relay)
            .await
            .expect("shutdown must abort a relay blocked on visitor backpressure")
            .expect("relay task");
        assert!(matches!(control, ConnectionControl::TearDown));
        drop(events_rx);
    }

    #[tokio::test]
    async fn deleting_a_location_kicks_its_live_agent_tunnel() {
        let store = Store::open(unique_db_path()).unwrap();
        store
            .put_location(&crate::store::Location {
                id: "loc-1".to_string(),
                name: "Remote".to_string(),
                geo_label: "DE".to_string(),
                map_query: None,
                facility: None,
                facility_url: None,
                kind: crate::store::NodeKind::Remote,
                data_plane_origin: None,
                offered_methods: vec![],
                status: crate::store::LocationStatus::Offline,
                created_at: 0,
            })
            .unwrap();
        enrolled_agent(&store, "agent-1");
        let (central_channel, _agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        {
            let hub = hub.clone();
            let store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }
        wait_for(|| hub.is_connected("agent-1")).await;

        let deleted = store.delete_location_with_agents("loc-1").unwrap();
        assert!(deleted.existed);
        assert_eq!(deleted.agent_ids, vec!["agent-1".to_string()]);
        assert_eq!(hub.kick_agents(&deleted.agent_ids), 1);

        wait_for(|| !hub.is_connected("agent-1")).await;
        assert!(
            !hub.is_connected("agent-1"),
            "removing a location must drop its live agent tunnel"
        );
    }

    fn unique_db_path() -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lg-tunnel-test-{}-{}.redb",
            std::process::id(),
            nanos
        ));
        path
    }
}
