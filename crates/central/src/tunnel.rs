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
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, DefaultBodyLimit};
use axum::{Extension, Router};
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use shared::exec::ExecStatus;
use shared::liveness::OFFLINE_AFTER;
use shared::protocol::{
    identity_pin, server_handshake, AuthChannel, CertificateReport, FrameTransport, TunnelError,
    TunnelMessage, TUNNEL_KEY_BYTES,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::{protocol::WebSocketConfig, Message};
use tokio_tungstenite::WebSocketStream;

use crate::auth::{tunnel_argon2_off_runtime, verify_password, DirectTls};
use crate::observability::new_correlation_id;
use crate::ratelimit::IpWindows;
use crate::store::Store;

const ENV_TUNNEL_BIND: &str = "LG_TUNNEL_BIND";
const ENV_TUNNEL_CERT: &str = "LG_TUNNEL_CERT";
const ENV_TUNNEL_KEY: &str = "LG_TUNNEL_KEY";
const TUNNEL_CERT_FILE: &str = "tunnel.crt";
const TUNNEL_KEY_FILE: &str = "tunnel.key";
const DEFAULT_TUNNEL_BIND: &str = "0.0.0.0:8443";

const PREAUTH_TIMEOUT: Duration = Duration::from_secs(10);
const PREAUTH_MAX_CONCURRENT: usize = 64;
const PREAUTH_FAILURE_MAX: u32 = 20;
const PREAUTH_FAILURE_WINDOW: Duration = Duration::from_secs(60);
// A 64 KiB output chunk can expand to roughly 384 KiB when JSON escapes every byte.
// Keep one authenticated protocol message comfortably above that while preventing
// unauthenticated peers from claiming tungstenite's 64 MiB default per connection.
const TUNNEL_MAX_MESSAGE_BYTES: usize = 512 * 1024;
/// An enrollment request is a protocol version and a token; anything larger on
/// the tunnel port is refused before it is buffered.
const ENROLL_MAX_BODY_BYTES: usize = 4 * 1024;

/// Backstop on inter-frame silence during a relayed run: if no frame arrives for
/// this long the connection is torn down so a silent agent can never hang the
/// consumer (AC41). It bounds the gap *between* frames, not total run duration —
/// the real run-duration bound is the agent's own exec deadline, which sends a
/// terminal frame far sooner on a healthy agent. This is not the Slice-8b liveness
/// window; it only catches an agent that stops responding without closing.
const RELAY_INTER_FRAME_TIMEOUT: Duration = Duration::from_secs(120);

/// An authenticated tunnel that hears nothing from its agent for this long is
/// dead, so a node that vanished without a FIN loses its task and hub entry, and
/// a run on it ends, within a minute. Central pings a quiet tunnel every quarter
/// of it; agents heartbeat every 10 s and answer pings (an older agent through
/// its WebSocket library), so a live agent is never cut. It matches the agent's
/// own silence deadline.
const TUNNEL_SILENCE: Duration = Duration::from_secs(2 * OFFLINE_AFTER.as_secs());

/// Bound on the relay event channel — backpressure on a slow consumer.
const RELAY_EVENT_CAPACITY: usize = 64;

/// How long a relayed run may wait on a consumer that stopped reading: the run's
/// own exec deadline, so a stalled visitor never holds the node past the longest
/// legitimate run. A run without saved limits gets the agent's default timeout.
const RELAY_RUN_DEADLINE: Duration = Duration::from_secs(30);

/// How long a relayed line may wait on a visitor who stopped reading before the
/// run is cancelled. Central reads no agent frame while it waits, so this stays
/// well inside the liveness window: a stalled visitor never makes a live location
/// read offline, however long the run's saved timeout.
const RELAY_STALL_TIMEOUT: Duration = Duration::from_secs(OFFLINE_AFTER.as_secs() / 2);

/// How long a cancelled run may take to reach its terminal frame before the
/// connection is torn down instead.
const RELAY_CANCEL_TIMEOUT: Duration = Duration::from_secs(10);

/// Pre-auth attempts per peer, on the shared [`IpWindows`] counter.
#[derive(Default)]
struct PreAuthFailures {
    windows: Mutex<IpWindows>,
}

impl PreAuthFailures {
    /// Count one attempt from `peer` and say whether it may start pre-auth. An
    /// attempt counts when admitted, not when it fails, so idle sockets from one
    /// address use up its allowance at once instead of each holding a pre-auth
    /// slot until it times out. A hold lasts at most 3 x [`PREAUTH_TIMEOUT`],
    /// inside one window, so one address holds at most 2 x
    /// [`PREAUTH_FAILURE_MAX`] (40) of the [`PREAUTH_MAX_CONCURRENT`] (64) slots.
    fn admit(&self, peer: IpAddr) -> bool {
        let mut windows = self.windows.lock().expect("tunnel preauth limiter");
        let now = Instant::now();
        if windows.count(peer, PREAUTH_FAILURE_WINDOW, now) >= PREAUTH_FAILURE_MAX {
            return false;
        }
        windows.hit(peer, PREAUTH_FAILURE_WINDOW, now);
        true
    }

    fn clear(&self, peer: IpAddr) {
        self.windows
            .lock()
            .expect("tunnel preauth limiter")
            .clear(peer);
    }
}

/// What a relayed run surfaces to its consumer (the visitor's SSE stream, wired
/// in Slice 10). Exactly one [`RelayEvent::Terminal`] is emitted per run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayEvent {
    /// One line of the agent's output.
    Line(String),
    /// The run ended with `status`, as a local run's `done` reports it. `error`
    /// carries a clear message for a failure the status does not explain — a
    /// refused start, a failed spawn, or the agent dropping mid-run (AC41).
    Terminal {
        error: Option<String>,
        status: ExecStatus,
    },
}

impl RelayEvent {
    fn failed(message: &str) -> Self {
        RelayEvent::Terminal {
            error: Some(message.to_string()),
            status: ExecStatus::Failed,
        }
    }
}

const REVOKED: &str = "the remote agent was revoked";
const DROPPED: &str = "the remote node dropped the connection";

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
    liveness: Arc<LivenessBatch>,
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
async fn verify_agent(store: &Store, agent_id: &str, credential: &str) -> bool {
    match store.get_agent(agent_id) {
        Ok(Some(agent)) if !agent.revoked => {
            let credential = credential.to_owned();
            tunnel_argon2_off_runtime(move || verify_password(&credential, &agent.credential_hash))
                .await
                .unwrap_or(false)
        }
        Err(error) => {
            tracing::error!(agent_id, %error, "could not read the agent to verify its credential");
            false
        }
        _ => false,
    }
}

/// Whether the connection survives a relayed run or must be torn down. Because a
/// single ordered channel carries one run at a time, any early exit (consumer
/// gone, inter-frame backstop, channel error, or a foreign `run_id`) leaves the
/// channel in an unsafe state — reusing it would forward this run's leftover
/// frames as the next run's, and the agent would keep running an abandoned
/// process. So those cases tear the whole connection down; the agent reconnects
/// with a fresh session and its exec engine reaps the abandoned run. The one
/// exception is a consumer that left or stalled on an agent that accepts
/// `Cancel`: that run is cancelled and drained to its terminal (see `cancel_run`).
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
    batch: Arc<LivenessBatch>,
    agent_id: String,
    last_written: u64,
    /// The data-plane origin last sent on this connection: sent on connect, and
    /// again after an admin edit, noticed on the agent's next idle frame.
    sent_origin: Option<String>,
}

impl Liveness {
    fn new(store: Store, batch: Arc<LivenessBatch>, agent_id: String) -> Self {
        Self {
            store,
            batch,
            agent_id,
            last_written: 0,
            sent_origin: None,
        }
    }

    fn touch(&mut self) {
        let now = crate::store::unix_now();
        if now <= self.last_written {
            return;
        }
        self.last_written = now;
        self.batch.record(&self.store, &self.agent_id, now);
    }

    fn is_revoked_or_missing(&self) -> bool {
        match self.store.get_agent(&self.agent_id) {
            Ok(Some(agent)) => agent.revoked,
            Ok(None) => true,
            Err(error) => {
                tracing::error!(agent_id = %self.agent_id, %error, "could not read the agent; closing its tunnel");
                true
            }
        }
    }

    /// The agent's location's data-plane origin, if it has one the admin API
    /// would accept today: one saved before the https-on-443 rule is withheld.
    fn data_plane_origin(&self) -> Option<String> {
        let location = self
            .store
            .get_agent(&self.agent_id)
            .and_then(|agent| match agent {
                Some(agent) => self.store.get_location(&agent.location_id),
                None => Ok(None),
            })
            .inspect_err(|error| {
                tracing::error!(agent_id = %self.agent_id, %error, "could not read the agent's data-plane origin");
            })
            .ok()??;
        crate::admin_api::clean_data_plane_origin(location.kind, location.data_plane_origin).ok()?
    }

    /// Store the agent's data-plane certificate status for the admin editor,
    /// only when the report is for the location's current origin: one that
    /// crossed an admin's origin edit (or a cleared origin) is dropped; the store
    /// keeps a status only for the origin its location had when it was recorded.
    /// A report names its origin; an older agent's does not, so it is taken to
    /// be for the origin last sent on this connection.
    fn record_certificate(&self, report: &CertificateReport) {
        let Some(origin) = report.origin.as_ref().or(self.sent_origin.as_ref()) else {
            return;
        };
        if Some(origin) != self.data_plane_origin().as_ref() {
            return;
        }
        let recorded = self
            .store
            .get_agent(&self.agent_id)
            .and_then(|agent| match agent {
                Some(agent) => {
                    self.store
                        .put_certificate_status(&agent.location_id, origin, &report.status)
                }
                None => Ok(false),
            });
        if let Err(error) = recorded {
            tracing::warn!(agent_id = %self.agent_id, %error, "failed to record the certificate status");
        }
    }
}

/// Hand the agent its location's data-plane origin when it differs from the one
/// last sent on this connection. Only an agent whose hello accepted
/// [`TunnelMessage::DataPlane`] is ever sent one.
// ponytail: a cleared origin is not sent; the agent keeps serving the last one until it restarts.
async fn sync_data_plane<T: FrameTransport>(
    channel: &mut AuthChannel<T>,
    liveness: &mut Liveness,
) -> Result<(), TunnelError> {
    if !channel.peer_accepts_data_plane() {
        return Ok(());
    }
    let Some(origin) = liveness.data_plane_origin() else {
        return Ok(());
    };
    if liveness.sent_origin.as_deref() == Some(origin.as_str()) {
        return Ok(());
    }
    channel
        .send_message(&TunnelMessage::DataPlane {
            origin: origin.clone(),
        })
        .await?;
    liveness.sent_origin = Some(origin);
    Ok(())
}

/// Coalesces every connected agent's liveness writes into as few redb commits as
/// possible, off the async runtime. Each commit fsyncs: inline, a burst of
/// heartbeats parked the async workers (stalling run output and /health), and one
/// commit per heartbeat capped a 500-agent burst beyond the 10s interval. Here
/// the newest timestamp per agent waits in `pending` while a single blocking-pool
/// flusher drains it, one transaction per drain.
#[derive(Default)]
struct LivenessBatch {
    pending: Mutex<HashMap<String, u64>>,
    flushing: AtomicBool,
}

impl LivenessBatch {
    fn record(self: &Arc<Self>, store: &Store, agent_id: &str, ts: u64) {
        {
            let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            let slot = pending.entry(agent_id.to_string()).or_insert(ts);
            *slot = (*slot).max(ts);
        }
        if self.flushing.swap(true, Ordering::AcqRel) {
            return;
        }
        let batch = Arc::clone(self);
        let store = store.clone();
        tokio::task::spawn_blocking(move || batch.flush(&store));
    }

    fn flush(&self, store: &Store) {
        loop {
            let beats: Vec<(String, u64)> = self
                .pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .drain()
                .collect();
            if beats.is_empty() {
                self.flushing.store(false, Ordering::Release);
                // A record landing between the drain and the flag drop found the
                // flag still set and did not spawn: reclaim and keep draining.
                let idle = self
                    .pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_empty();
                if idle || self.flushing.swap(true, Ordering::AcqRel) {
                    return;
                }
                continue;
            }
            #[cfg(test)]
            if let Some(gate) = tests::liveness_write_gate_for(store) {
                gate.enter();
            }
            if let Err(error) = store.touch_agents_last_seen(&beats) {
                // Liveness is best-effort telemetry; a failed write must not tear
                // down a healthy tunnel. Surface it, keep serving.
                tracing::warn!(agents = beats.len(), %error, "failed to record agent liveness");
            }
        }
    }
}

/// Relay one command down an authenticated channel and stream the agent's output
/// back as [`RelayEvent`]s. Emits exactly one [`RelayEvent::Terminal`]. Returns
/// [`ConnectionControl::KeepAlive`] only on a clean run-terminal (the agent's
/// `Done`/`Error`), including a cancelled run drained to its terminal; every
/// other early exit returns [`ConnectionControl::TearDown`].
async fn relay_run<T: FrameTransport>(
    channel: &mut AuthChannel<T>,
    command: TunnelMessage,
    events: mpsc::Sender<RelayEvent>,
    run_deadline: Duration,
    previous_run: Option<&str>,
    liveness: &mut Liveness,
    shutdown: &mut oneshot::Receiver<()>,
) -> ConnectionControl {
    let active_run = command.run_id().unwrap_or_default().to_string();
    let run_deadline = tokio::time::Instant::now() + run_deadline;
    let tear_down = |events, message| {
        send_terminal(events, run_deadline, RelayEvent::failed(message));
        ConnectionControl::TearDown
    };
    if liveness.is_revoked_or_missing() {
        return tear_down(events, REVOKED);
    }
    let sent = tokio::select! {
        biased;
        _ = &mut *shutdown => return tear_down(events, REVOKED),
        sent = channel.send_message(&command) => sent,
    };
    if sent.is_err() {
        return tear_down(events, DROPPED);
    }
    loop {
        let received = tokio::select! {
            biased;
            _ = &mut *shutdown => None,
            // The visitor left: stop just this run.
            _ = events.closed() => return cancel_run(channel, &active_run, liveness).await,
            received = tokio::time::timeout(RELAY_INTER_FRAME_TIMEOUT, channel.recv_message()) => Some(received),
        };
        let message = match received {
            None => return tear_down(events, REVOKED),
            Some(Err(_elapsed)) => {
                return tear_down(events, "the remote node did not respond in time");
            }
            Some(Ok(Ok(message))) => message,
            // Bad tag / replay / closed transport: the agent dropped or the
            // channel is compromised. Surface a terminal error (AC41) and tear
            // the tunnel down — never continue past an auth failure.
            Some(Ok(Err(_channel_error))) => return tear_down(events, DROPPED),
        };

        if liveness.is_revoked_or_missing() {
            return tear_down(events, REVOKED);
        }

        // Any authenticated frame received during a run is proof the agent is alive.
        liveness.touch();

        // Correlate every run-bearing frame with the active run: a frame for
        // another run is a protocol violation, never forwarded as this run's output.
        if let Some(frame_run) = message.run_id() {
            if frame_run != active_run {
                // An agent that predates one terminal per run follows its
                // `Error` with a `Done`; that late `Done` for the previous run
                // on this connection is dropped. Any other run is a violation.
                if matches!(message, TunnelMessage::Done { .. }) && previous_run == Some(frame_run)
                {
                    continue;
                }
                return tear_down(events, "the remote node sent a frame for a different run");
            }
        }

        let status = message.terminal_status();
        match message {
            TunnelMessage::Output { line, .. } => {
                // Bounded by the run deadline and the stall bound, so a visitor
                // who stopped reading cannot hold the node past its run, or leave
                // its heartbeats unread until the location reads offline.
                let stalled_at =
                    (tokio::time::Instant::now() + RELAY_STALL_TIMEOUT).min(run_deadline);
                let sent = tokio::select! {
                    biased;
                    _ = &mut *shutdown => None,
                    sent = tokio::time::timeout_at(stalled_at, events.send(RelayEvent::Line(line))) => Some(sent),
                };
                match sent {
                    None => return tear_down(events, REVOKED),
                    Some(Ok(Ok(()))) => {}
                    Some(_) => {
                        // The visitor left or stalled mid-stream: stop just this run.
                        drop(events);
                        return cancel_run(channel, &active_run, liveness).await;
                    }
                }
            }
            TunnelMessage::Done { ok, .. } => {
                let terminal = match status {
                    Some(status) => RelayEvent::Terminal {
                        error: None,
                        status,
                    },
                    // An agent that predates `status` reports only `ok`.
                    None if ok => RelayEvent::Terminal {
                        error: None,
                        status: ExecStatus::Completed { success: true },
                    },
                    None => RelayEvent::failed("the diagnostic finished with an error"),
                };
                send_terminal(events, run_deadline, terminal);
                return ConnectionControl::KeepAlive;
            }
            TunnelMessage::Error { message, .. } => {
                // An agent that predates `status` reports every error as failed.
                let terminal = RelayEvent::Terminal {
                    error: Some(message),
                    status: status.unwrap_or(ExecStatus::Failed),
                };
                send_terminal(events, run_deadline, terminal);
                return ConnectionControl::KeepAlive;
            }
            // A heartbeat (Slice 8b) or an unexpected up-frame is ignored, not fatal.
            TunnelMessage::Certificate(report) => liveness.record_certificate(&report),
            TunnelMessage::Heartbeat
            | TunnelMessage::Command { .. }
            | TunnelMessage::Cancel { .. }
            | TunnelMessage::DataPlane { .. } => {}
        }
    }
}

/// Hand a run its one terminal without letting a visitor who stopped reading
/// hold the node: if the channel is full, a background send delivers it once
/// the visitor reads, bounded by the run deadline like a forwarded line.
fn send_terminal(
    events: mpsc::Sender<RelayEvent>,
    run_deadline: tokio::time::Instant,
    terminal: RelayEvent,
) {
    if let Err(mpsc::error::TrySendError::Full(terminal)) = events.try_send(terminal) {
        tokio::spawn(async move {
            let _ = tokio::time::timeout_at(run_deadline, events.send(terminal)).await;
        });
    }
}

/// Stop the active run after its consumer left or stalled. An agent whose hello
/// accepted [`TunnelMessage::Cancel`] stops just that run: its in-flight frames
/// are drained up to the run's terminal and the connection is kept. An older
/// agent cannot decode `Cancel`, so its connection is torn down as before (the
/// agent reconnects and its exec engine reaps the abandoned run).
async fn cancel_run<T: FrameTransport>(
    channel: &mut AuthChannel<T>,
    active_run: &str,
    liveness: &mut Liveness,
) -> ConnectionControl {
    if !channel.peer_accepts_cancel() {
        return ConnectionControl::TearDown;
    }
    let drained = tokio::time::timeout(RELAY_CANCEL_TIMEOUT, async {
        let cancel = TunnelMessage::Cancel {
            run_id: active_run.to_string(),
        };
        if channel.send_message(&cancel).await.is_err() {
            return false;
        }
        while let Ok(message) = channel.recv_message().await {
            liveness.touch();
            match message.run_id() {
                Some(run) if run != active_run => return false,
                _ if matches!(
                    message,
                    TunnelMessage::Done { .. } | TunnelMessage::Error { .. }
                ) =>
                {
                    return true
                }
                _ => {}
            }
        }
        false
    })
    .await;
    if drained == Ok(true) {
        ConnectionControl::KeepAlive
    } else {
        ConnectionControl::TearDown
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
    let mut liveness = Liveness::new(store, Arc::clone(&hub.liveness), agent_id.clone());
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
    let mut previous_run: Option<String> = None;
    let mut synced = sync_data_plane(&mut channel, &mut liveness).await.is_ok();
    while synced {
        tokio::select! {
            biased;
            _ = &mut shutdown_rx => {
                break;
            }
            job = job_rx.recv() => {
                let Some(job) = job else { break };
                let run_id = job.command.run_id().map(str::to_owned);
                let deadline = match &job.command {
                    TunnelMessage::Command { limits: Some(limits), .. } => {
                        Duration::from_secs(limits.timeout_secs)
                    }
                    _ => RELAY_RUN_DEADLINE,
                };
                let control = relay_run(
                    &mut channel,
                    job.command,
                    job.events,
                    deadline,
                    previous_run.as_deref(),
                    &mut liveness,
                    &mut shutdown_rx,
                )
                .await;
                match control {
                    ConnectionControl::KeepAlive => {
                        previous_run = run_id;
                        job.busy.store(false, Ordering::Release);
                    }
                    ConnectionControl::TearDown => break,
                }
            }
            frame = channel.recv_message() => {
                match frame {
                    // An idle up-frame (a heartbeat) is proof of life; a certificate
                    // report is also stored for the admin editor.
                    Ok(message) => {
                        if liveness.is_revoked_or_missing() {
                            break;
                        }
                        liveness.touch();
                        if let TunnelMessage::Certificate(report) = message {
                            liveness.record_certificate(&report);
                        }
                        synced = sync_data_plane(&mut channel, &mut liveness).await.is_ok();
                    }
                    // A closed / forged / replayed frame ends the connection — fail
                    // closed, never continue past an auth failure.
                    Err(_error) => break,
                }
            }
        }
    }
    // A job still queued when the connection ends (a revoke landing first) is
    // never relayed, but its visitor still gets one terminal. Closing first
    // lets `recv` return the jobs already queued, then `None`.
    job_rx.close();
    while let Some(job) = job_rx.recv().await {
        let ended = if liveness.is_revoked_or_missing() {
            REVOKED
        } else {
            DROPPED
        };
        send_terminal(
            job.events,
            tokio::time::Instant::now() + RELAY_RUN_DEADLINE,
            RelayEvent::failed(ended),
        );
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
/// listener presents, and the SHA-256 of the end-entity certificate's public key
/// ([`identity_pin`]) — the value the install command carries and the agent pins
/// (Slice 7).
pub struct TunnelIdentity {
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    fingerprint: String,
}

impl TunnelIdentity {
    /// Resolve the tunnel identity at startup. With `LG_TUNNEL_CERT` and
    /// `LG_TUNNEL_KEY` both set, load those files and write nothing. With both
    /// unset, use (or on first start generate) `tunnel.crt`/`tunnel.key` beside
    /// the database. `None` disables the tunnel listener (fail-safe, not
    /// fail-open); the web surface keeps serving.
    pub fn from_env(db_path: &str, store: &Store) -> Option<Self> {
        let cert_path = std::env::var(ENV_TUNNEL_CERT).unwrap_or_default();
        let key_path = std::env::var(ENV_TUNNEL_KEY).unwrap_or_default();
        Self::resolve(&cert_path, &key_path, db_path, store)
    }

    fn resolve(cert_path: &str, key_path: &str, db_path: &str, store: &Store) -> Option<Self> {
        if cert_path.is_empty() && key_path.is_empty() {
            let dir = Path::new(db_path)
                .parent()
                .unwrap_or_else(|| Path::new("."));
            return Self::from_dir(dir, store);
        }
        if cert_path.is_empty() || key_path.is_empty() {
            tracing::warn!(
                "only one of {ENV_TUNNEL_CERT}/{ENV_TUNNEL_KEY} is set — agent tunnel listener \
                 DISABLED; no remote agents can connect until both are configured"
            );
            return None;
        }
        match Self::load(cert_path, key_path) {
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

    /// Load `tunnel.crt`/`tunnel.key` from `dir`, or generate both when neither
    /// exists. Anything else (one file alone, unreadable, corrupt) is left
    /// untouched: overwriting it would strand every enrolled agent's pin.
    fn from_dir(dir: &Path, store: &Store) -> Option<Self> {
        let cert_path = dir.join(TUNNEL_CERT_FILE);
        let key_path = dir.join(TUNNEL_KEY_FILE);
        let resolved = match (file_present(&cert_path), file_present(&key_path)) {
            (Ok(false), Ok(false)) => {
                Self::generate(&cert_path, &key_path).map(|identity| (identity, "generated"))
            }
            (Ok(true), Ok(true)) => {
                warn_if_key_exposed(&key_path);
                Self::load(&cert_path, &key_path).map(|identity| (identity, "loaded"))
            }
            (Err(error), _) | (_, Err(error)) => Err(error),
            _ => Err(io::Error::other(format!(
                "{TUNNEL_CERT_FILE} and {TUNNEL_KEY_FILE} must both exist; found only one"
            ))),
        };
        let (identity, source) = match resolved {
            Ok(resolved) => resolved,
            Err(error) => {
                tracing::error!(
                    %error,
                    path = %dir.display(),
                    "tunnel identity files not usable and left untouched — agent tunnel \
                     listener DISABLED; fix or remove both files to continue"
                );
                return None;
            }
        };
        tracing::info!(
            fingerprint = %identity.fingerprint,
            path = %dir.display(),
            "tunnel identity {source}"
        );
        if source == "generated" {
            match store.all_agents() {
                Ok(agents) => {
                    let stranded = agents.iter().filter(|agent| !agent.revoked).count();
                    if stranded > 0 {
                        tracing::error!(
                            agents = stranded,
                            path = %dir.display(),
                            "generated a new tunnel identity while {stranded} agents are enrolled; \
                             they no longer match its pin and must re-enroll"
                        );
                    }
                }
                Err(error) => tracing::error!(
                    %error,
                    path = %dir.display(),
                    "generated a new tunnel identity but could not count enrolled agents; \
                     any enrolled agent must re-enroll"
                ),
            }
        }
        Some(identity)
    }

    /// A fresh self-signed identity. The agent pins the SPKI, not the name, so
    /// `localhost` is enough. Each file lands by temp-file-then-rename.
    fn generate(cert_path: &Path, key_path: &Path) -> io::Result<Self> {
        let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .map_err(io::Error::other)?;
        write_owner_only(key_path, &generated.signing_key.serialize_pem())?;
        write_owner_only(cert_path, &generated.cert.pem())?;
        // Make both renames durable, so a crash cannot leave only one file.
        #[cfg(unix)]
        {
            let dir = cert_path
                .parent()
                .filter(|dir| !dir.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            std::fs::File::open(dir)?.sync_all()?;
        }
        Self::load(cert_path, key_path)
    }

    fn load(cert_path: impl AsRef<Path>, key_path: impl AsRef<Path>) -> io::Result<Self> {
        let certs = CertificateDer::pem_file_iter(cert_path)
            .map_err(pem_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(pem_error)?;
        let key = PrivateKeyDer::from_pem_file(key_path).map_err(pem_error)?;
        let end_entity = certs
            .first()
            .ok_or_else(|| io::Error::other("tunnel certificate file contains no certificate"))?;
        let fingerprint = identity_pin(end_entity.as_ref());
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

/// Whether `path` exists, without following a symlink. Only "not found" counts
/// as absent; any other error keeps the caller from writing.
fn file_present(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Warn when the default `tunnel.key` is not an owner-only regular file: others
/// on the host may read the tunnel's private key. It still loads.
fn warn_if_key_exposed(key_path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(key_path) else {
        return;
    };
    #[cfg(unix)]
    let group_or_other = std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o077;
    #[cfg(not(unix))]
    let group_or_other = 0;
    if !metadata.is_file() || group_or_other != 0 {
        tracing::warn!(
            path = %key_path.display(),
            "{TUNNEL_KEY_FILE} is not an owner-only regular file; make it a plain file \
             with mode 0600 so only central can read the tunnel private key"
        );
    }
}

/// Write `contents` to a fresh 0600 temp file beside `path`, then rename it in.
fn write_owner_only(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write;

    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&tmp)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
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
/// proxy (FR-071b). A connection that opens with `POST ` is served by `enroll`
/// instead, so an agent can enroll under the same pinned key.
pub async fn serve(
    bind: SocketAddr,
    identity: TunnelIdentity,
    store: Store,
    hub: TunnelHub,
    enroll: Router,
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
        let (tcp, peer, permit) = match accept_permitted_socket(&listener, permit).await {
            Ok(accepted) => accepted,
            // EMFILE or a pending network error is transient: stopping here
            // would lock every agent out until central restarts.
            Err(error) => {
                tracing::warn!(%error, "agent tunnel accept failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        // Gate the peer before its socket may keep the pre-auth slot: a refused
        // socket closes here and its slot goes straight back to other agents.
        if !preauth_failures.admit(peer.ip()) {
            tracing::info!(%peer, "agent tunnel pre-auth rate limit exceeded");
            continue;
        }
        let acceptor = acceptor.clone();
        let store = store.clone();
        let hub = hub.clone();
        let enroll = enroll.clone();
        let preauth_failures = preauth_failures.clone();
        tokio::spawn(async move {
            match handle_connection(acceptor, tcp, peer, store, hub, enroll, permit).await {
                // An enrollment exchange, even a successful one, keeps its
                // admission: only an authenticated agent clears the window.
                Ok(authenticated) => {
                    if authenticated {
                        preauth_failures.clear(peer.ip());
                    }
                }
                Err(error) => {
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
    #[cfg(test)]
    tests::injected_accept_error(listener)?;
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

/// Serve one tunnel connection; `Ok(true)` when an agent authenticated on it.
/// The pre-auth `permit` is held until then, or until an enrollment ends.
async fn handle_connection<S>(
    acceptor: TlsAcceptor,
    tcp: S,
    peer: SocketAddr,
    store: Store,
    hub: TunnelHub,
    enroll: Router,
    permit: OwnedSemaphorePermit,
) -> io::Result<bool>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // The request's first bytes pick its server; they ride the TLS step's
    // timeout so a connection still spends at most three pre-auth steps.
    let (method, reader, writer) = preauth_timeout(
        async {
            let (mut reader, writer) = tokio::io::split(acceptor.accept(tcp).await?);
            let mut method = [0u8; 5];
            reader.read_exact(&mut method).await?;
            Ok((method, reader, writer))
        },
        "tls handshake",
    )
    .await?;
    // Replay the consumed bytes to whichever server takes the connection.
    let stream = tokio::io::join(std::io::Cursor::new(method).chain(reader), writer);
    if &method == b"POST " {
        preauth_timeout(serve_enrollment(stream, peer, enroll), "enrollment").await?;
        return Ok(false);
    }
    let ws = ws_preauth_timeout(
        tokio_tungstenite::accept_async_with_config(stream, Some(tunnel_websocket_config())),
        "websocket handshake",
    )
    .await?;
    let transport = WsTransport::new(ws, TUNNEL_SILENCE);

    let (agent_id, channel) = ws_preauth_timeout(
        server_handshake(transport, random_nonce(), |agent_id, credential| {
            let store = store.clone();
            async move { verify_agent(&store, &agent_id, &credential).await }
        }),
        "agent credential handshake",
    )
    .await?;
    drop(permit);

    serve_agent(channel, hub, store, agent_id).await;
    Ok(true)
}

/// Serve one HTTP/1 request with `enroll`, then close. [`DirectTls`] tells the
/// handler the request came over central's own TLS, which a proxy header on the
/// web port cannot claim. Any other route answers 404 without reaching the store.
async fn serve_enrollment<S>(stream: S, peer: SocketAddr, enroll: Router) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::server::conn::auto::Builder;
    use hyper_util::service::TowerToHyperService;

    let app = enroll
        .layer(DefaultBodyLimit::max(ENROLL_MAX_BODY_BYTES))
        .layer(Extension(DirectTls))
        .layer(Extension(ConnectInfo(peer)));
    let mut builder = Builder::new(TokioExecutor::new()).http1_only();
    builder.http1().keep_alive(false);
    builder
        .serve_connection(TokioIo::new(stream), TowerToHyperService::new(app))
        .await
        .map_err(io::Error::other)
}

fn tunnel_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(TUNNEL_MAX_MESSAGE_BYTES))
        .max_frame_size(Some(TUNNEL_MAX_MESSAGE_BYTES))
}

fn random_nonce() -> [u8; TUNNEL_KEY_BYTES] {
    let mut nonce = [0u8; TUNNEL_KEY_BYTES];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut nonce)
        .expect("system CSPRNG must be available");
    nonce
}

/// A [`FrameTransport`] over a WebSocket: one binary message per frame. Pongs to
/// the agent's pings are handled by tungstenite; a close ends the stream.
///
/// Hearing nothing from the agent for `silence` ends the stream with an error.
/// While it waits, the transport pings every quarter of that window, so a live
/// but quiet agent keeps the tunnel.
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
    async fn send(&mut self, frame: Vec<u8>) -> io::Result<()> {
        // An agent that stopped reading fills the socket; give up rather than
        // hold this agent's task until TCP gives up.
        tokio::time::timeout(self.silence, self.ws.send(Message::binary(frame)))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "the agent stopped reading"))?
            .map_err(|error| io::Error::other(format!("{error}")))
    }

    async fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        let silent = || io::Error::new(io::ErrorKind::TimedOut, "the agent went silent");
        loop {
            // Absolute instants: `serve_agent`'s select drops this future when a
            // job arrives, and that must not restart the clock.
            let dead_at = self.heard + self.silence;
            let ping_at = self.pinged + self.silence / 4;
            let Ok(message) = tokio::time::timeout_at(dead_at.min(ping_at), self.ws.next()).await
            else {
                if tokio::time::Instant::now() >= dead_at {
                    return Err(silent());
                }
                self.pinged = tokio::time::Instant::now();
                tokio::time::timeout_at(dead_at, self.ws.send(Message::Ping(Default::default())))
                    .await
                    .map_err(|_| silent())?
                    .map_err(|error| io::Error::other(format!("{error}")))?;
                continue;
            };
            let Some(message) = message else {
                return Ok(None);
            };
            self.heard = tokio::time::Instant::now();
            match message.map_err(|error| io::Error::other(format!("{error}")))? {
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
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::crypto::WebPkiSupportedAlgorithms;
    use rustls::pki_types::{ServerName, UnixTime};
    use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
    use shared::protocol::{
        client_handshake, verify_pinned_identity, ChannelTransport, TunnelAccept, TunnelHello,
        PROTOCOL_VERSION,
    };
    use std::sync::{Condvar, OnceLock};
    use tokio_rustls::TlsConnector;
    use tokio_tungstenite::client_async;
    use tower::ServiceExt;

    const CRED: &str = "aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44";
    const TEST_CERT: &str = "-----BEGIN CERTIFICATE-----\nMIIDHzCCAgegAwIBAgIUE9j6GO1vmvEiR+U8AWUAQgAta5AwDQYJKoZIhvcNAQEL\nBQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDcxODE2MjgyOFoXDTI2MDcx\nOTE2MjgyOFowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\nAAOCAQ8AMIIBCgKCAQEAqYMxqyibqUTXEqhZwC9l98xvsCbsy7rIT9smbo1wkEuK\nC7vzt2+35VZoCdetrufP7AeMlipMtboVvCfol/nJDv6sS7fEue1KF224pxaK+fJ2\nepRUKTZtSuGAlBOlZ9hq9ArihghBi27GaC9D6VXhzwFcgw8wXBaUlC612ENMIxZp\nrTJ3I98MSZJw6URqfM9Jy4w3jzLLAyQnhE+QOYoxXCU+w497A+jgkY4X6bL998PT\nGaXmoFyjaVwYZYuEPcClwzANu+NKM6HY+UaYAxYOcWOrwYw3zP4VQogSrkqIngM4\nFpnPcmCBlHt8SitpoDmBUQl0s/MYmI+kZDeIEFGKeQIDAQABo2kwZzAdBgNVHQ4E\nFgQUNO9xNnkuVM5DKp77D9I/MhTykHwwHwYDVR0jBBgwFoAUNO9xNnkuVM5DKp77\nD9I/MhTykHwwDwYDVR0TAQH/BAUwAwEB/zAUBgNVHREEDTALgglsb2NhbGhvc3Qw\nDQYJKoZIhvcNAQELBQADggEBAJUFLLXcqJwlcE2evAICc7vvgk1hBaJf3Di5CsFQ\nLCGZgfffUeGc9u4FlB6hYJh9dBBUhT5vZxC3zcix42XBcw0OUINwET8EaaY9uAEl\nE1qZUSRFLu35VkuaBanRU/zC9mxVFr1z51mEDycWAg3IvG8V4EENNVX4EsCuzjkZ\ngknfR+HootmuWU3RnaKVgNH65uveP+kmR6RISX+s9q+2oBapgSyx6IorMKcM5vGP\nCVUItooWGUMtlEjJKQshBDAViBt+LXUZEKGF01y4mqYXQflk3IR91Y84daWefsoe\nbwbZf+XByMOy3UZliVC08FfAYbsDq/J4V5yPyXWNJ+bedTc=\n-----END CERTIFICATE-----\n";
    const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCpgzGrKJupRNcS\nqFnAL2X3zG+wJuzLushP2yZujXCQS4oLu/O3b7flVmgJ162u58/sB4yWKky1uhW8\nJ+iX+ckO/qxLt8S57UoXbbinFor58nZ6lFQpNm1K4YCUE6Vn2Gr0CuKGCEGLbsZo\nL0PpVeHPAVyDDzBcFpSULrXYQ0wjFmmtMncj3wxJknDpRGp8z0nLjDePMssDJCeE\nT5A5ijFcJT7Dj3sD6OCRjhfpsv33w9MZpeagXKNpXBhli4Q9wKXDMA2740ozodj5\nRpgDFg5xY6vBjDfM/hVCiBKuSoieAzgWmc9yYIGUe3xKK2mgOYFRCXSz8xiYj6Rk\nN4gQUYp5AgMBAAECggEALdKdNpt/mL5XNV/1AxLNCbNl7cRX9qrDQ3MGbJQnfZot\n8wYX19qHZ6N39FEtTj6z4iYYRu+gVO+8uGRBZ/PJ+he2E7HVqD0Q7kxmwiRB5Vc5\n1+EI7ysbWEalL2IwMGY8Y0QeAAVzUnHbiIZeYVEp/X9strEAbaRc/cGyvodSqZkQ\nIJq2s1uEnKRfsTDnHYnC/XlDKo+9vtKL17b03X8BI9+S4VsKCPL8oSapvFndk0EF\n3K2yX0yRmyzP0pEMOwVQo10BGUtRaQbLUFjPStka/SB9oeZf2yIQk6pm+dS4Mmsm\nrh3pwyPmVaDWmVdMstcLsANqnLnLBlhF5u0+FJEJUQKBgQDol7xfXjdtCkHBbz6t\n45Gt8zrESpYqmALY3oOae4yRUBwJR2Ebp9I61CcxwJDaQdy37FYwHjiIe8d3Mn2O\nxV1K3WevQbgZbxP32qXDuwVYL1xRqsl9nhz+ahSlsOvsQP9ptK4AfGebiglem5PF\n3dLDA71pbj6l0Cck8ABiWFdxAwKBgQC6klPxTfmViNLZIOKIFIRoPWiA6VwsmzdZ\nQBWlzAnId03WKHUv9pNNEkOKgLUZZ+RQyqbVIOCjdRayM62BsbRH/KXpvnu5zwNi\nM4zkpONBAWo1lP9kjuqhbU2HK46OY1MX1D4eXS46PA0E2pvx/aaidJ1U92D8zy5Y\nvSwmjz130wKBgGYKq8nrO8XKyi5i78y6Gh+GpjGXx2nIZvdeJ76OlYzq6GHpvuCz\nL7g/ezKImQQoAP1v4iAaIhM+urPAovUQAW3m1KY+3tXJtaj3c+H7Gs0legsaMmu6\nAl5bi9NlWxu7KFLnwa7U5V+Hn7Sx7JLSTrTf3ylyBGoaeBHseT6sIzChAoGATMAF\naC77jVhL5KZyiihmj7szUlStZmwzyLNkNGBLZfwuOPtLuf9leT8aKc/osBrdAZ9c\nIjD0OEninExGBCRmVXbJie6iVz2h1rP+MdDi68r5NjGlHmjsfJvKWODCNDEH7bWS\nGEucyLgLYwPLQzFla08tqdZaP6W7GyY3E2W5k6ECgYEAsRe55Rf8Lvlb8XwEGLNe\nbNAU4xx8/fElMFb5oDSC0dgp+U3IBViMrjTVAKy19xJCuz/dbgQAJyzX5jl/fx41\n7DZ4iIxAW6tGN6xFA81gyczkJhB9yFAlm8QFxPg7UuI1yALR8GiG0BtucPvmB1Wy\nAJaiJD9UQtSneCNnCnNn/uw=\n-----END PRIVATE KEY-----\n";
    static LOG_CAPTURE: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    static LIVENESS_WRITE_GATE: OnceLock<Mutex<Option<Arc<LivenessWriteGate>>>> = OnceLock::new();
    static LIVENESS_WRITE_GATE_TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    pub(super) struct LivenessWriteGate {
        /// Only a flush into this store claims the gate, so another test's
        /// heartbeats cannot; `None` is claimed only by a direct `enter`.
        store: Option<Store>,
        claimed: AtomicBool,
        state: Mutex<LivenessWriteGateState>,
        changed: Condvar,
    }

    #[derive(Default)]
    struct LivenessWriteGateState {
        entered: bool,
        held: bool,
        departed: bool,
        health_completed: bool,
        output_completed: bool,
        released: bool,
    }

    #[derive(Clone, Copy, Debug)]
    struct LivenessWriteGateTransitions {
        entered: bool,
        held: bool,
        departed: bool,
        health_completed: bool,
        output_completed: bool,
        released: bool,
    }

    impl LivenessWriteGate {
        fn new(store: Option<Store>) -> Arc<Self> {
            Arc::new(Self {
                store,
                claimed: AtomicBool::new(false),
                state: Mutex::new(LivenessWriteGateState::default()),
                changed: Condvar::new(),
            })
        }

        pub(super) fn enter(&self) {
            if self
                .claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.entered = true;
            state.held = true;
            self.changed.notify_all();
            while !state.released {
                let (next, timeout) = self
                    .changed
                    .wait_timeout(state, Duration::from_secs(2))
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                assert!(
                    !timeout.timed_out(),
                    "liveness write gate was not released within the responsiveness budget"
                );
                state = next;
            }
            state.held = false;
            state.departed = true;
            self.changed.notify_all();
        }

        fn wait_for_entry(&self) {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while !state.entered {
                let (next, timeout) = self
                    .changed
                    .wait_timeout(state, Duration::from_secs(2))
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                assert!(
                    !timeout.timed_out(),
                    "no dispatched heartbeat entered the liveness write gate"
                );
                state = next;
            }
        }

        fn release(&self) {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(state.entered, "release requires a live liveness write gate");
            assert!(state.held, "release requires a held liveness write gate");
            assert!(
                !state.departed,
                "release requires the liveness write not to have departed"
            );
            assert!(
                state.health_completed,
                "release requires the health probe to complete"
            );
            assert!(
                state.output_completed,
                "release requires the output probe to complete"
            );
            state.released = true;
            self.changed.notify_all();
        }

        fn record_health_completion(&self) {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(state.entered, "health completion requires gate entry");
            assert!(
                state.held && !state.departed,
                "health completion requires a held, not-departed liveness write"
            );
            state.health_completed = true;
        }

        fn record_output_completion(&self) {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(state.entered, "output completion requires gate entry");
            assert!(
                state.held && !state.departed,
                "output completion requires a held, not-departed liveness write"
            );
            state.output_completed = true;
        }

        fn transitions(&self) -> LivenessWriteGateTransitions {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            LivenessWriteGateTransitions {
                entered: state.entered,
                held: state.held,
                departed: state.departed,
                health_completed: state.health_completed,
                output_completed: state.output_completed,
                released: state.released,
            }
        }
    }

    pub(super) fn liveness_write_gate() -> Option<Arc<LivenessWriteGate>> {
        LIVENESS_WRITE_GATE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .expect("liveness write gate registry")
            .clone()
    }

    pub(super) fn liveness_write_gate_for(store: &Store) -> Option<Arc<LivenessWriteGate>> {
        liveness_write_gate().filter(|gate| {
            gate.store
                .as_ref()
                .is_some_and(|armed| Arc::ptr_eq(&armed.database(), &store.database()))
        })
    }

    struct LivenessWriteGateGuard {
        gate: Arc<LivenessWriteGate>,
    }

    impl std::ops::Deref for LivenessWriteGateGuard {
        type Target = LivenessWriteGate;

        fn deref(&self) -> &Self::Target {
            &self.gate
        }
    }

    impl Drop for LivenessWriteGateGuard {
        fn drop(&mut self) {
            {
                let mut state = self
                    .gate
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.released = true;
                self.gate.changed.notify_all();
            }
            let mut slot = LIVENESS_WRITE_GATE
                .get_or_init(|| Mutex::new(None))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot
                .as_ref()
                .is_some_and(|armed| Arc::ptr_eq(armed, &self.gate))
            {
                *slot = None;
            }
        }
    }

    fn arm_liveness_write_gate(store: Option<&Store>) -> LivenessWriteGateGuard {
        let gate = LivenessWriteGate::new(store.cloned());
        let mut slot = LIVENESS_WRITE_GATE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .expect("liveness write gate registry");
        assert!(slot.is_none(), "only one liveness load gate may be armed");
        *slot = Some(gate.clone());
        LivenessWriteGateGuard { gate }
    }

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

    fn test_identity() -> TunnelIdentity {
        let certs = CertificateDer::pem_slice_iter(TEST_CERT.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .expect("test certificate");
        let key = PrivateKeyDer::from_pem_slice(TEST_KEY.as_bytes()).expect("test private key");
        TunnelIdentity {
            fingerprint: identity_pin(certs[0].as_ref()),
            certs,
            key,
        }
    }

    #[derive(Debug)]
    struct TestPinnedCentral {
        fingerprint: String,
        algorithms: WebPkiSupportedAlgorithms,
    }

    impl ServerCertVerifier for TestPinnedCentral {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            verify_pinned_identity(end_entity.as_ref(), &self.fingerprint)
                .map(|()| ServerCertVerified::assertion())
                .map_err(|_| rustls::Error::General("test tunnel identity mismatch".to_string()))
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

    fn test_client_config(fingerprint: String) -> ClientConfig {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(TestPinnedCentral {
            fingerprint,
            algorithms: provider.signature_verification_algorithms,
        });
        ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("test TLS versions")
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth()
    }

    async fn start_test_connection(
        store: Store,
        hub: TunnelHub,
    ) -> (
        SocketAddr,
        tokio::task::JoinHandle<io::Result<bool>>,
        ClientConfig,
    ) {
        start_enroll_connection(store, hub, Router::new()).await
    }

    async fn start_enroll_connection(
        store: Store,
        hub: TunnelHub,
        enroll: Router,
    ) -> (
        SocketAddr,
        tokio::task::JoinHandle<io::Result<bool>>,
        ClientConfig,
    ) {
        let identity = test_identity();
        let client = test_client_config(identity.fingerprint().to_string());
        let acceptor = build_acceptor(identity).expect("test TLS acceptor");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener");
        let address = listener.local_addr().expect("test listener address");
        let task = tokio::spawn(async move {
            let (tcp, peer) = listener.accept().await?;
            let permit = Arc::new(Semaphore::new(1))
                .acquire_owned()
                .await
                .expect("test pre-auth permit");
            handle_connection(acceptor, tcp, peer, store, hub, enroll, permit).await
        });
        (address, task, client)
    }

    async fn test_websocket_client(
        address: SocketAddr,
        client: ClientConfig,
    ) -> WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
        let tcp = tokio::net::TcpStream::connect(address)
            .await
            .expect("connect test tunnel");
        let tls = TlsConnector::from(Arc::new(client))
            .connect(
                ServerName::try_from("localhost").expect("test server name"),
                tcp,
            )
            .await
            .expect("test TLS handshake");
        client_async("ws://localhost/", tls)
            .await
            .expect("test WebSocket handshake")
            .0
    }

    // The pin the install command carries, and the one the tunnel listener
    // logs, is the SHA-256 of the leaf's SubjectPublicKeyInfo, so a renewal that
    // keeps the key keeps every enrolled agent's pin.
    #[test]
    fn central_pins_its_certificate_by_public_key() {
        let cert = CertificateDer::from_pem_slice(TEST_CERT.as_bytes()).expect("test certificate");
        let parsed = rustls::server::ParsedCertificate::try_from(&cert).expect("parse certificate");
        let expected = shared::protocol::sha256_hex(parsed.subject_public_key_info().as_ref());

        let minted = crate::enroll::CentralIdentity::from_material(cert.as_ref().to_vec());
        assert_eq!(minted.fingerprint(), expected, "install command pin");

        let dir = std::env::temp_dir().join(format!(
            "lg-tunnel-identity-{}-{}",
            std::process::id(),
            crate::auth::random_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let (cert_path, key_path) = (dir.join("tunnel.crt"), dir.join("tunnel.key"));
        std::fs::write(&cert_path, TEST_CERT).unwrap();
        std::fs::write(&key_path, TEST_KEY).unwrap();
        let loaded = TunnelIdentity::load(cert_path.to_str().unwrap(), key_path.to_str().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            loaded.expect("load tunnel identity").fingerprint(),
            expected,
            "tunnel listener pin"
        );
    }

    fn identity_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lg-tunnel-{label}-{}-{}",
            std::process::id(),
            crate::auth::random_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_identity_is_generated_once_then_reloaded_with_the_same_pin() {
        let logs = captured_logs();
        let dir = identity_dir("generate");
        let db_path = dir.join("lookingglass.redb");
        let store = Store::open(&db_path).expect("test store");
        let db_path = db_path.to_str().unwrap();

        let generated = TunnelIdentity::resolve("", "", db_path, &store).expect("generated");
        let cert = std::fs::read(dir.join("tunnel.crt")).expect("tunnel.crt written");
        let key = std::fs::read(dir.join("tunnel.key")).expect("tunnel.key written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("tunnel.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "tunnel.key is owner-only");
        }
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files renamed away: {leftovers:?}"
        );

        let reloaded = TunnelIdentity::resolve("", "", db_path, &store).expect("reloaded");
        assert_eq!(reloaded.fingerprint(), generated.fingerprint());
        assert_eq!(std::fs::read(dir.join("tunnel.crt")).unwrap(), cert);
        assert_eq!(std::fs::read(dir.join("tunnel.key")).unwrap(), key);

        let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        let path = dir.display().to_string();
        for source in ["generated", "loaded"] {
            let message = format!("tunnel identity {source}");
            let line = logs
                .lines()
                .find(|line| line.contains(&path) && line.contains(&message))
                .unwrap_or_else(|| panic!("{message} logged"));
            assert!(line.contains("INFO"), "{line}");
            assert!(
                contains_field(line, "fingerprint", generated.fingerprint()),
                "{line}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn configured_identity_writes_nothing_beside_the_database() {
        let configured = identity_dir("configured");
        let (cert_path, key_path) = (configured.join("custom.crt"), configured.join("custom.key"));
        std::fs::write(&cert_path, TEST_CERT).unwrap();
        std::fs::write(&key_path, TEST_KEY).unwrap();
        let data = identity_dir("configured-data");
        let db_path = data.join("lookingglass.redb");
        let store = Store::open(&db_path).expect("test store");
        let db_path = db_path.to_str().unwrap();
        let (cert_path, key_path) = (cert_path.to_str().unwrap(), key_path.to_str().unwrap());

        let identity =
            TunnelIdentity::resolve(cert_path, key_path, db_path, &store).expect("env identity");
        assert_eq!(identity.fingerprint(), test_identity().fingerprint());
        assert!(TunnelIdentity::resolve(cert_path, "", db_path, &store).is_none());
        assert!(TunnelIdentity::resolve("", key_path, db_path, &store).is_none());

        for name in ["tunnel.crt", "tunnel.key"] {
            assert!(!data.join(name).exists(), "{name} must not be generated");
        }
        let _ = std::fs::remove_dir_all(&configured);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn damaged_default_identity_is_left_untouched_and_disables_the_tunnel() {
        let cases: [(&str, Option<&str>, Option<&str>); 3] = [
            ("corrupt-key", Some(TEST_CERT), Some("not a key\n")),
            ("lone-cert", Some(TEST_CERT), None),
            ("lone-key", None, Some(TEST_KEY)),
        ];
        for (label, cert, key) in cases {
            let dir = identity_dir(label);
            let db_path = dir.join("lookingglass.redb");
            let store = Store::open(&db_path).expect("test store");
            for (name, contents) in [("tunnel.crt", cert), ("tunnel.key", key)] {
                if let Some(contents) = contents {
                    std::fs::write(dir.join(name), contents).unwrap();
                }
            }

            let identity = TunnelIdentity::resolve("", "", db_path.to_str().unwrap(), &store);
            assert!(identity.is_none(), "{label}: tunnel stays disabled");
            for (name, contents) in [("tunnel.crt", cert), ("tunnel.key", key)] {
                let on_disk = std::fs::read_to_string(dir.join(name)).ok();
                assert_eq!(on_disk.as_deref(), contents, "{label}: {name} untouched");
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn generating_a_new_identity_logs_how_many_agents_must_reenroll() {
        let logs = captured_logs();
        let dir = identity_dir("reenroll");
        let db_path = dir.join("lookingglass.redb");
        let store = Store::open(&db_path).expect("test store");
        enrolled_agent(&store, "a1");
        enrolled_agent(&store, "a2");
        store
            .put_agent(&crate::store::Agent {
                id: "a3".to_string(),
                location_id: "loc-1".to_string(),
                credential_hash: "$argon2id$stub".to_string(),
                enrolled_at: 0,
                last_seen: None,
                revoked: true,
            })
            .unwrap();

        TunnelIdentity::resolve("", "", db_path.to_str().unwrap(), &store).expect("generated");

        let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        let path = dir.display().to_string();
        let line = logs
            .lines()
            .find(|line| line.contains(&path) && line.contains("must re-enroll"))
            .expect("re-enroll error logged");
        assert!(line.contains("ERROR"), "{line}");
        assert!(contains_field(line, "agents", "2"), "{line}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A default tunnel.key that others may read, or that is not a regular
    // file, still loads, with a warning; an owner-only one loads quietly.
    #[cfg(unix)]
    #[test]
    fn an_exposed_default_key_loads_with_a_warning() {
        use std::os::unix::fs::PermissionsExt;
        let logs = captured_logs();
        let dir = identity_dir("exposed-key");
        let db_path = dir.join("lookingglass.redb");
        let store = Store::open(&db_path).expect("test store");
        let db_path = db_path.to_str().unwrap();
        let key_path = dir.join("tunnel.key");
        std::fs::write(dir.join("tunnel.crt"), TEST_CERT).unwrap();
        std::fs::write(&key_path, TEST_KEY).unwrap();
        let key = key_path.display().to_string();
        let warnings = || {
            let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
            logs.lines()
                .filter(|line| line.contains("WARN") && line.contains(&key))
                .filter(|line| line.contains("owner-only"))
                .count()
        };
        let mode = |path: &Path, mode| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap()
        };

        mode(&key_path, 0o600);
        assert!(TunnelIdentity::resolve("", "", db_path, &store).is_some());
        assert_eq!(warnings(), 0, "an owner-only key loads quietly");

        mode(&key_path, 0o640);
        assert!(TunnelIdentity::resolve("", "", db_path, &store).is_some());
        assert_eq!(warnings(), 1, "a group-readable key warns");

        // A symlink is not a regular file, whatever its target's mode.
        let target = dir.join("real.key");
        std::fs::rename(&key_path, &target).unwrap();
        mode(&target, 0o600);
        std::os::unix::fs::symlink(&target, &key_path).unwrap();
        assert!(TunnelIdentity::resolve("", "", db_path, &store).is_some());
        assert_eq!(warnings(), 2, "a symlinked key warns");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preauth_failures_rate_limit_and_clear_by_peer() {
        let limiter = PreAuthFailures::default();
        let peer = "198.51.100.7".parse().unwrap();
        for _ in 0..PREAUTH_FAILURE_MAX {
            assert!(limiter.admit(peer));
        }
        assert!(!limiter.admit(peer));
        limiter.clear(peer);
        assert!(limiter.admit(peer));
    }

    #[test]
    fn preauth_failures_key_ipv6_by_64() {
        let limiter = PreAuthFailures::default();
        for i in 1..=PREAUTH_FAILURE_MAX {
            assert!(limiter.admit(format!("2001:db8::{i:x}").parse().unwrap()));
        }
        assert!(!limiter.admit("2001:db8::ffff".parse().unwrap()));
        assert!(limiter.admit("2001:db8:0:1::1".parse().unwrap()));
    }

    #[test]
    fn preauth_websocket_messages_are_bounded_to_protocol_scale() {
        let config = tunnel_websocket_config();
        assert_eq!(config.max_message_size, Some(512 * 1024));
        assert_eq!(config.max_frame_size, Some(512 * 1024));
    }

    #[tokio::test]
    async fn real_handler_rejects_oversized_preauth_hello_before_auth_and_accepts_a_valid_control()
    {
        let store = Store::open(unique_db_path()).expect("test store");
        store
            .put_agent(&crate::store::Agent {
                id: "agent-1".to_string(),
                location_id: "loc-1".to_string(),
                credential_hash: crate::auth::hash_password(CRED).expect("test credential hash"),
                enrolled_at: 0,
                last_seen: None,
                revoked: false,
            })
            .expect("enroll test agent");

        let hub = TunnelHub::new();
        let (address, handler, client) = start_test_connection(store.clone(), hub.clone()).await;
        let mut oversized = test_websocket_client(address, client).await;
        let payload = serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "agent_id": "agent-1",
            "credential": CRED,
            "client_nonce": vec![1u8; TUNNEL_KEY_BYTES],
            "padding": "x".repeat(TUNNEL_MAX_MESSAGE_BYTES),
        })
        .to_string()
        .into_bytes();
        assert!(
            payload.len() > TUNNEL_MAX_MESSAGE_BYTES,
            "test payload exceeds cap"
        );
        // The handler may reject the frame from its header and close while the
        // rest is still being written, so the send can fail with a broken pipe or
        // a reset. Either is the rejection this test expects; anything else fails.
        match oversized.send(Message::Binary(payload.into())).await {
            Ok(()) => {}
            Err(tokio_tungstenite::tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                ) => {}
            Err(error) => panic!("send oversized pre-auth hello: {error:?}"),
        }
        let rejected = tokio::time::timeout(Duration::from_secs(2), handler)
            .await
            .expect("oversized pre-auth message must be bounded")
            .expect("handler task");
        assert!(
            rejected.is_err(),
            "oversized message must reject the real handler"
        );
        assert!(
            !hub.is_connected("agent-1"),
            "oversized hello must not reach credential verification or agent registration"
        );
        assert!(
            store
                .get_agent("agent-1")
                .unwrap()
                .unwrap()
                .last_seen
                .is_none(),
            "oversized hello must not mark the agent as connected"
        );
        drop(oversized);

        // The valid control's credential check runs debug-build Argon2, which can
        // take seconds under parallel load. This bound only guards the positive
        // control against a hang; the rejection bound above stays 2 s.
        const VALID_CONTROL_BOUND: Duration = Duration::from_secs(30);
        let (address, handler, client) = start_test_connection(store, hub.clone()).await;
        let mut control = test_websocket_client(address, client).await;
        let hello = TunnelHello {
            protocol_version: PROTOCOL_VERSION,
            agent_id: "agent-1".to_string(),
            credential: CRED.to_string(),
            client_nonce: [2u8; TUNNEL_KEY_BYTES],
            accepts_cancel: true,
            accepts_data_plane: true,
        };
        control
            .send(Message::Binary(serde_json::to_vec(&hello).unwrap().into()))
            .await
            .expect("send protocol-sized hello");
        let accept = tokio::time::timeout(VALID_CONTROL_BOUND, control.next())
            .await
            .expect("valid hello must reach auth path")
            .expect("valid hello response")
            .expect("valid WebSocket response");
        assert!(matches!(
            accept,
            Message::Binary(bytes) if serde_json::from_slice::<TunnelAccept>(&bytes).is_ok()
        ));
        let registered = tokio::time::timeout(VALID_CONTROL_BOUND, async {
            while !hub.is_connected("agent-1") {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(
            registered.is_ok(),
            "valid hello must register after authentication"
        );
        drop(control);
        assert!(
            tokio::time::timeout(VALID_CONTROL_BOUND, handler)
                .await
                .expect("valid control handler must finish")
                .expect("valid control handler task")
                .is_ok(),
            "valid control connection must complete normally"
        );
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

    /// Listener ports whose next accept fails, as EMFILE or a pending network
    /// error would.
    static FAILING_ACCEPTS: Mutex<Vec<u16>> = Mutex::new(Vec::new());

    pub(super) fn injected_accept_error(listener: &TcpListener) -> io::Result<()> {
        let port = listener.local_addr()?.port();
        let mut failing = FAILING_ACCEPTS.lock().unwrap();
        match failing.iter().position(|failing| *failing == port) {
            Some(index) => {
                failing.remove(index);
                Err(io::Error::other("injected accept failure"))
            }
            None => Ok(()),
        }
    }

    // F-214: one failed accept() must not stop the agent tunnel listener for
    // good; it logs, waits briefly and accepts the next agent.
    #[tokio::test]
    async fn the_tunnel_listener_keeps_accepting_after_an_accept_error() {
        let store = Store::open(unique_db_path()).unwrap();
        let identity = test_identity();
        let client = Arc::new(test_client_config(identity.fingerprint().to_string()));
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        FAILING_ACCEPTS.lock().unwrap().push(port);
        let address = SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
        tokio::spawn(serve(
            address,
            identity,
            store,
            TunnelHub::new(),
            Router::new(),
        ));

        let handshake = async {
            loop {
                if let Ok(tcp) = tokio::net::TcpStream::connect(address).await {
                    let server = ServerName::try_from("localhost").expect("test server name");
                    if TlsConnector::from(client.clone())
                        .connect(server, tcp)
                        .await
                        .is_ok()
                    {
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        assert!(
            tokio::time::timeout(Duration::from_secs(5), handshake)
                .await
                .is_ok(),
            "an agent must still get a TLS handshake after one failed accept"
        );
    }

    // F-208: an agent that vanishes without a FIN (no frame, no pong) ends its
    // tunnel within the silence deadline, so its task and hub entry go; a quiet
    // agent that answers central's pings keeps its tunnel.
    #[tokio::test(start_paused = true)]
    async fn a_silent_agent_tunnel_ends_and_a_quiet_live_one_is_pinged() {
        use tokio_tungstenite::tungstenite::protocol::Role;
        let silence = Duration::from_millis(400);

        // The vanished peer's socket still takes bytes (its buffer is not
        // full) but nothing ever comes back.
        let (central_io, mut vanished) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move { tokio::io::copy(&mut vanished, &mut tokio::io::sink()).await });
        let mut dead = WsTransport::new(
            WebSocketStream::from_raw_socket(central_io, Role::Server, None).await,
            silence,
        );
        let ended = tokio::time::timeout(Duration::from_secs(3), dead.recv())
            .await
            .expect("a silent tunnel must end within its silence deadline");
        assert!(ended.is_err(), "{ended:?}");

        let (central_io, agent_io) = tokio::io::duplex(64 * 1024);
        let mut live = WsTransport::new(
            WebSocketStream::from_raw_socket(central_io, Role::Server, None).await,
            silence,
        );
        let mut agent = WebSocketStream::from_raw_socket(agent_io, Role::Client, None).await;
        // Reading is what makes tungstenite answer a ping, as an agent does.
        let pings = tokio::spawn(async move {
            let mut pings = 0;
            while let Some(Ok(message)) = agent.next().await {
                pings += usize::from(message.is_ping());
            }
            pings
        });
        assert!(
            tokio::time::timeout(silence * 4, live.recv())
                .await
                .is_err(),
            "a quiet agent that answers pings must keep its tunnel"
        );
        drop(live);
        assert!(pings.await.unwrap() > 0, "central must ping a quiet tunnel");
    }

    // F-361: an agent that stops reading cannot hold central's send forever;
    // like the agent's, the send gives up within the silence window.
    #[tokio::test(start_paused = true)]
    async fn a_send_to_an_agent_that_stops_reading_times_out() {
        use tokio_tungstenite::tungstenite::protocol::Role;
        let silence = Duration::from_millis(400);
        let (central_io, _stalled) = tokio::io::duplex(1024);
        let mut transport = WsTransport::new(
            WebSocketStream::from_raw_socket(central_io, Role::Server, None).await,
            silence,
        );
        let sent = tokio::time::timeout(Duration::from_secs(3), transport.send(vec![0; 64 * 1024]))
            .await
            .expect("a send to a stalled agent must end within its bound");
        assert!(sent.is_err(), "{sent:?}");
    }

    // F-346: a login flood holding every login Argon2 permit must not keep a
    // reconnecting agent's credential check past its pre-auth deadline.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn agent_handshake_completes_while_login_argon2_permits_are_saturated() {
        use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};

        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let (started_tx, mut started) = mpsc::unbounded_channel();
        let mut release = Vec::new();
        let logins: Vec<_> = (0..cores)
            .map(|_| {
                let (hold, held) = std::sync::mpsc::channel::<()>();
                release.push(hold);
                let started_tx = started_tx.clone();
                tokio::spawn(crate::auth::argon2_off_runtime(move || {
                    started_tx.send(()).unwrap();
                    let _ = held.recv();
                }))
            })
            .collect();
        for _ in 0..cores {
            started.recv().await.unwrap();
        }

        // Cheap Argon2 params keep the debug-build verify itself well inside the
        // deadline; only the wait for a permit is under test.
        let cheap = argon2::Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            argon2::Params::new(8, 1, 1, None).unwrap(),
        )
        .hash_password(CRED.as_bytes(), &SaltString::generate(&mut OsRng))
        .unwrap()
        .to_string();
        let store = Store::open(unique_db_path()).unwrap();
        store
            .put_agent(&crate::store::Agent {
                id: "agent-1".to_string(),
                location_id: "loc-1".to_string(),
                credential_hash: cheap,
                enrolled_at: 0,
                last_seen: None,
                revoked: false,
            })
            .unwrap();
        let (address, _handler, client) = start_test_connection(store, TunnelHub::new()).await;
        let mut agent = test_websocket_client(address, client).await;
        let hello = TunnelHello {
            protocol_version: PROTOCOL_VERSION,
            agent_id: "agent-1".to_string(),
            credential: CRED.to_string(),
            client_nonce: [2u8; TUNNEL_KEY_BYTES],
            accepts_cancel: true,
            accepts_data_plane: true,
        };
        agent
            .send(Message::Binary(serde_json::to_vec(&hello).unwrap().into()))
            .await
            .unwrap();
        let reply =
            tokio::time::timeout(PREAUTH_TIMEOUT + Duration::from_secs(2), agent.next()).await;

        drop(release);
        for login in logins {
            assert!(login.await.unwrap().is_some());
        }
        assert!(
            matches!(
                &reply,
                Ok(Some(Ok(Message::Binary(bytes))))
                    if serde_json::from_slice::<TunnelAccept>(bytes).is_ok()
            ),
            "the handshake must be accepted within its deadline, got {reply:?}"
        );
    }

    // Idle sockets from one address cannot take every pre-auth slot; a new
    // agent from another address still gets its TLS handshake promptly.
    #[tokio::test]
    async fn idle_preauth_sockets_from_one_address_do_not_block_a_new_agent() {
        let store = Store::open(unique_db_path()).unwrap();
        let identity = test_identity();
        let client = test_client_config(identity.fingerprint().to_string());
        let port = std::net::TcpListener::bind("[::]:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        tokio::spawn(serve(
            SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)),
            identity,
            store,
            TunnelHub::new(),
            Router::new(),
        ));
        let attacker = SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
        let mut holders = Vec::new();
        for _ in 0..50 {
            if let Ok(socket) = tokio::net::TcpStream::connect(attacker).await {
                holders.push(socket);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        while holders.len() < PREAUTH_MAX_CONCURRENT {
            holders.push(tokio::net::TcpStream::connect(attacker).await.unwrap());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;

        let tcp = tokio::net::TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, port))
            .await
            .unwrap();
        let handshake = tokio::time::timeout(
            Duration::from_secs(3),
            TlsConnector::from(Arc::new(client)).connect(
                ServerName::try_from("localhost").expect("test server name"),
                tcp,
            ),
        )
        .await;
        assert!(
            matches!(handshake, Ok(Ok(_))),
            "a new agent must not wait behind {} idle sockets from one address",
            holders.len()
        );
        drop(holders);
    }

    /// App state whose install command pins the test tunnel certificate, with
    /// no trusted proxy, so only the tunnel's own TLS can make a request secure.
    fn tunnel_enroll_state(store: Store) -> crate::AppState {
        let cert = CertificateDer::from_pem_slice(TEST_CERT.as_bytes()).expect("test certificate");
        crate::AppState {
            store,
            transport: crate::TransportConfig::new(std::iter::empty()),
            login_limiter: Arc::new(crate::LoginLimiter::default()),
            setup_token: None,
            run: crate::RunService::for_test(8, Duration::from_secs(30), 100),
            files_root: Arc::from(std::env::temp_dir().as_path()),
            enroll: crate::EnrollConfig::for_test("https://central.test", cert.as_ref().to_vec()),
            tunnel_hub: TunnelHub::new(),
        }
    }

    /// A store with one remote location holding a live and an expired token;
    /// returns the store, the live token and the expired one.
    fn enrollment_store() -> (Store, String, String) {
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
                asn: None,
                offered_methods: vec![],
                status: crate::store::LocationStatus::Offline,
                created_at: 0,
            })
            .unwrap();
        let (live, expired) = ("live-enrollment-token", "expired-enrollment-token");
        for (id, token, expires_at) in [("t-live", live, u64::MAX), ("t-expired", expired, 1)] {
            store
                .put_enrollment_token(&crate::store::EnrollmentToken {
                    id: id.to_string(),
                    location_id: "loc-1".to_string(),
                    token_hash: shared::protocol::sha256_hex(token.as_bytes()),
                    expires_at,
                    used_at: None,
                })
                .unwrap();
        }
        (store, live.to_string(), expired.to_string())
    }

    fn token_used(store: &Store, token: &str) -> bool {
        store
            .find_token_by_hash(&shared::protocol::sha256_hex(token.as_bytes()))
            .unwrap()
            .expect("seeded token")
            .used_at
            .is_some()
    }

    fn http_request(head: &str, token: &str) -> String {
        let body =
            serde_json::json!({ "protocol_version": PROTOCOL_VERSION, "token": token }).to_string();
        format!(
            "{head} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    fn response_body(response: &str) -> &str {
        response.split_once("\r\n\r\n").map_or("", |(_, body)| body)
    }

    /// Send `request` over a pinned TLS connection and read until central closes.
    async fn tunnel_exchange(address: SocketAddr, client: ClientConfig, request: &str) -> String {
        use tokio::io::AsyncWriteExt;
        let tcp = tokio::net::TcpStream::connect(address)
            .await
            .expect("connect test tunnel");
        let mut tls = TlsConnector::from(Arc::new(client))
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .expect("pinned TLS handshake");
        tls.write_all(request.as_bytes())
            .await
            .expect("send request");
        let mut response = Vec::new();
        // A refused request may close without close_notify; what arrived counts.
        let _ = tls.read_to_end(&mut response).await;
        String::from_utf8_lossy(&response).into_owned()
    }

    /// One request on a fresh tunnel connection, pinned to the install
    /// command's fingerprint; returns the response and the handler's result.
    async fn tunnel_request(state: &crate::AppState, request: &str) -> (String, io::Result<bool>) {
        let enroll = crate::enroll::agent_route().with_state(state.clone());
        let (address, handler, _) =
            start_enroll_connection(state.store.clone(), state.tunnel_hub.clone(), enroll).await;
        let client = test_client_config(state.enroll.identity.fingerprint());
        let response = tunnel_exchange(address, client, request).await;
        let served = tokio::time::timeout(Duration::from_secs(30), handler)
            .await
            .expect("the tunnel connection must end")
            .expect("handler task");
        (response, served)
    }

    // An agent enrolls on the tunnel port under the key the install command
    // pins, spending its token once; a spent, expired or unknown token gets the
    // web route's 401.
    #[tokio::test]
    async fn an_agent_enrolls_on_the_tunnel_port_under_the_install_pin() {
        let (store, live, expired) = enrollment_store();
        let state = tunnel_enroll_state(store.clone());
        assert_eq!(
            state.enroll.identity.fingerprint(),
            test_identity().fingerprint(),
            "the install command pins the key the tunnel presents"
        );

        let (response, served) =
            tunnel_request(&state, &http_request("POST /api/enroll", &live)).await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(
            matches!(served, Ok(false)),
            "an enrollment never clears the peer's admissions: {served:?}"
        );
        let issued: shared::protocol::EnrollResponse =
            serde_json::from_str(response_body(&response)).expect("enroll response");
        let agent = store
            .get_agent(&issued.agent_id)
            .unwrap()
            .expect("agent stored");
        assert_eq!(agent.location_id, "loc-1");
        assert!(verify_password(&issued.credential, &agent.credential_hash));
        assert!(token_used(&store, &live), "the token is spent");

        let refused =
            axum::response::IntoResponse::into_response(crate::auth::ApiError::Unauthorized);
        let refused = axum::body::to_bytes(refused.into_body(), usize::MAX)
            .await
            .unwrap();
        for token in [live.as_str(), expired.as_str(), "unknown-token"] {
            let (response, _) =
                tunnel_request(&state, &http_request("POST /api/enroll", token)).await;
            assert!(response.starts_with("HTTP/1.1 401"), "{token}: {response}");
            assert_eq!(response_body(&response).as_bytes(), &refused[..], "{token}");
        }
        assert_eq!(store.all_agents().unwrap().len(), 1, "one credential only");
    }

    // Only POST /api/enroll and a WebSocket GET / are served on the tunnel
    // port; anything else closes without reaching the store, valid token or not.
    #[tokio::test]
    async fn other_requests_on_the_tunnel_port_close_without_touching_the_store() {
        let (store, live, _) = enrollment_store();
        let state = tunnel_enroll_state(store.clone());
        for request in [
            http_request("POST /api/enroll/extra", &live),
            http_request("POST /", &live),
            http_request("PUT /api/enroll", &live),
            http_request("GET /api/enroll", &live),
            "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string(),
        ] {
            let (response, served) = tunnel_request(&state, &request).await;
            assert!(!matches!(served, Ok(true)), "{request}");
            assert!(
                !response.starts_with("HTTP/1.1 200"),
                "{request}: {response}"
            );
            assert!(!token_used(&store, &live), "{request}");
            assert!(store.all_agents().unwrap().is_empty(), "{request}");
        }
    }

    // The tunnel port refuses an enrollment body past its small bound before
    // the handler runs, live token or not.
    #[tokio::test]
    async fn an_oversized_enrollment_body_on_the_tunnel_port_is_refused() {
        let (store, live, _) = enrollment_store();
        let state = tunnel_enroll_state(store.clone());
        let body =
            serde_json::json!({ "protocol_version": PROTOCOL_VERSION, "token": live }).to_string();
        // Trailing whitespace keeps it valid JSON: only its size is wrong.
        let body = format!("{body}{}", " ".repeat(ENROLL_MAX_BODY_BYTES));
        let request = format!(
            "POST /api/enroll HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );

        let (response, _) = tunnel_request(&state, &request).await;

        assert!(response.starts_with("HTTP/1.1 413"), "{response}");
        assert!(!token_used(&store, &live), "the token is not spent");
        assert!(store.all_agents().unwrap().is_empty(), "no agent is stored");
    }

    /// Open a pinned TLS connection and send `request` as two TLS records, the
    /// first `split` bytes alone, so central reads them before the rest arrives.
    async fn split_request(
        address: SocketAddr,
        client: ClientConfig,
        request: &str,
        split: usize,
    ) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
        use tokio::io::AsyncWriteExt;
        let tcp = tokio::net::TcpStream::connect(address)
            .await
            .expect("connect test tunnel");
        let mut tls = TlsConnector::from(Arc::new(client))
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .expect("pinned TLS handshake");
        let (first, rest) = request.as_bytes().split_at(split);
        tls.write_all(first).await.expect("send the first bytes");
        tls.flush().await.expect("flush the first bytes");
        tokio::time::sleep(Duration::from_millis(100)).await;
        tls.write_all(rest).await.expect("send the rest");
        tls.flush().await.expect("flush the rest");
        tls
    }

    // A client may send a request's first bytes in a TLS record of their own;
    // central still routes the request by its whole method.
    #[tokio::test]
    async fn a_method_split_across_tls_records_still_reaches_its_server() {
        let (store, live, _) = enrollment_store();
        let state = tunnel_enroll_state(store.clone());
        let enroll = crate::enroll::agent_route().with_state(state.clone());
        let (address, handler, client) =
            start_enroll_connection(store.clone(), state.tunnel_hub.clone(), enroll).await;
        let mut tls =
            split_request(address, client, &http_request("POST /api/enroll", &live), 2).await;
        let mut response = Vec::new();
        let _ = tls.read_to_end(&mut response).await;
        let response = String::from_utf8_lossy(&response);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(token_used(&store, &live), "the split enrollment is served");
        let served = tokio::time::timeout(Duration::from_secs(30), handler)
            .await
            .expect("the enrollment connection must end")
            .expect("handler task");
        assert!(matches!(served, Ok(false)), "{served:?}");

        let (address, handler, client) = start_test_connection(store, TunnelHub::new()).await;
        let upgrade = "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\n\
                       Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
                       Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
        let mut tls = split_request(address, client, upgrade, 2).await;
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            let mut byte = [0u8; 1];
            match tls.read(&mut byte).await {
                Ok(1) => response.push(byte[0]),
                _ => break,
            }
        }
        let response = String::from_utf8_lossy(&response);
        assert!(
            response.starts_with("HTTP/1.1 101"),
            "WebSocket handshake: {response}"
        );
        drop(tls);
        let ended = tokio::time::timeout(Duration::from_secs(30), handler)
            .await
            .expect("the tunnel connection must end")
            .expect("handler task");
        assert!(!matches!(ended, Ok(true)), "no agent authenticated");
    }

    // A stalled enrollment holds one pre-auth slot and is cut at the pre-auth
    // timeout, its token unspent.
    #[tokio::test(start_paused = true)]
    async fn a_stalled_enrollment_is_cut_at_the_preauth_timeout() {
        use tokio::io::AsyncWriteExt;
        let (store, live, _) = enrollment_store();
        let enroll = crate::enroll::agent_route().with_state(tunnel_enroll_state(store.clone()));
        let identity = test_identity();
        let client = test_client_config(identity.fingerprint().to_string());
        let acceptor = build_acceptor(identity).unwrap();
        let (central_io, agent_io) = tokio::io::duplex(64 * 1024);
        let slots = Arc::new(Semaphore::new(1));
        let permit = slots.clone().acquire_owned().await.unwrap();
        let peer = SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 40000));
        let handler = tokio::spawn(handle_connection(
            acceptor,
            central_io,
            peer,
            store.clone(),
            TunnelHub::new(),
            enroll,
            permit,
        ));

        let mut tls = TlsConnector::from(Arc::new(client))
            .connect(ServerName::try_from("localhost").unwrap(), agent_io)
            .await
            .expect("pinned TLS handshake");
        let request = http_request("POST /api/enroll", &live);
        // Headers and most of the body, then silence.
        tls.write_all(&request.as_bytes()[..request.len() - 4])
            .await
            .unwrap();
        tls.flush().await.unwrap();
        let started = tokio::time::Instant::now();

        tokio::time::sleep(PREAUTH_TIMEOUT - Duration::from_secs(1)).await;
        assert!(!handler.is_finished(), "cut too early");
        assert_eq!(slots.available_permits(), 0, "the stall holds its slot");

        let error = tokio::time::timeout(PREAUTH_TIMEOUT * 2, handler)
            .await
            .expect("a stalled enrollment must be cut")
            .unwrap()
            .expect_err("a stalled enrollment ends in an error");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() <= PREAUTH_TIMEOUT + Duration::from_secs(1));
        assert_eq!(slots.available_permits(), 1, "the slot comes back");
        assert!(!token_used(&store, &live));
        drop(tls);
    }

    /// Establish an authenticated central↔"agent" channel pair over an in-memory
    /// transport: `central_channel` is the real central side; `agent_channel` is
    /// the test playing the agent.
    async fn established_pair() -> (AuthChannel<ChannelTransport>, AuthChannel<ChannelTransport>) {
        established_pair_for("agent-1").await
    }

    async fn established_pair_for(
        agent_id: &str,
    ) -> (AuthChannel<ChannelTransport>, AuthChannel<ChannelTransport>) {
        let (agent_side, central_side) = ChannelTransport::pair();
        let agent_id = agent_id.to_owned();
        let client =
            tokio::spawn(
                async move { client_handshake(agent_side, &agent_id, CRED, [1u8; 32]).await },
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
            limits: None,
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
                    limits: None,
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
                target: "8.8.8.8".into(),
                limits: None,
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
                status: None,
            })
            .await
            .unwrap();

        assert_eq!(
            events.recv().await,
            Some(RelayEvent::Line("64 bytes from 8.8.8.8".into()))
        );
        assert_eq!(
            events.recv().await,
            Some(RelayEvent::Terminal {
                error: None,
                status: shared::exec::ExecStatus::Completed { success: true },
            })
        );
        // The connection correctly stays open for the next job after a clean run;
        // end the test by cancelling the serving task rather than awaiting it.
        serving.abort();
    }

    // F-154: the relay deadline follows the run's saved timeout, so a visitor
    // who stopped reading is cut when the run's own timeout ends (here 1 s),
    // not at a fixed 30 s.
    #[tokio::test(start_paused = true)]
    async fn a_stalled_visitor_is_cut_at_the_saved_timeout() {
        let (central_channel, mut agent_channel) = established_pair().await;
        let hub = TunnelHub::new();
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let serving = {
            let hub = hub.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            })
        };
        wait_for(|| hub.is_connected("agent-1")).await;

        let events = hub
            .submit(
                "agent-1",
                TunnelMessage::Command {
                    run_id: "r1".into(),
                    method: "ping".into(),
                    target: "8.8.8.8".into(),
                    limits: Some(shared::protocol::RunLimits {
                        timeout_secs: 1,
                        max_output_bytes: 1024,
                    }),
                },
            )
            .await
            .expect("agent is connected");
        assert!(matches!(
            agent_channel.recv_message().await.unwrap(),
            TunnelMessage::Command { .. }
        ));
        let started = tokio::time::Instant::now();
        // One line more than the unread visitor channel holds.
        for _ in 0..=RELAY_EVENT_CAPACITY {
            agent_channel
                .send_message(&TunnelMessage::Output {
                    run_id: "r1".into(),
                    line: "x".into(),
                })
                .await
                .unwrap();
        }

        let cancelled = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let TunnelMessage::Cancel { run_id } =
                    agent_channel.recv_message().await.unwrap()
                {
                    break run_id;
                }
            }
        })
        .await
        .expect("central must cancel the stalled run at its saved timeout");
        assert_eq!(cancelled, "r1");
        assert!(started.elapsed() < Duration::from_secs(3), "{started:?}");
        drop(events);
        serving.abort();
    }

    /// A run with a 45 s saved timeout, relayed to a served test agent.
    async fn relayed_run_with_45s_timeout(
    ) -> (AuthChannel<ChannelTransport>, mpsc::Receiver<RelayEvent>) {
        let (central_channel, mut agent) = established_pair().await;
        let hub = serving_hub(central_channel).await;
        let mut long_run = command("r1");
        if let TunnelMessage::Command { limits, .. } = &mut long_run {
            *limits = Some(shared::protocol::RunLimits {
                timeout_secs: 45,
                max_output_bytes: 1 << 20,
            });
        }
        let events = hub.submit("agent-1", long_run.clone()).await.unwrap();
        assert_eq!(agent.recv_message().await.unwrap(), long_run);
        (agent, events)
    }

    // F-327: a long saved timeout must not let a visitor who stopped reading
    // stop central reading the agent's frames (its heartbeats) until the
    // location derives offline: the stalled run is cancelled within the window.
    #[tokio::test(start_paused = true)]
    async fn a_stalled_visitor_is_cut_before_the_node_goes_offline() {
        let (mut agent, events) = relayed_run_with_45s_timeout().await;
        fill_visitor_channel(&mut agent, &events, "r1").await;
        let started = tokio::time::Instant::now();
        agent
            .send_message(&TunnelMessage::Output {
                run_id: "r1".into(),
                line: "blocked".into(),
            })
            .await
            .unwrap();
        let cancelled = tokio::time::timeout(OFFLINE_AFTER, async {
            loop {
                if let TunnelMessage::Cancel { run_id } = agent.recv_message().await.unwrap() {
                    break run_id;
                }
            }
        })
        .await
        .expect("a stalled visitor must be cut before the node's heartbeats go unread for the liveness window");
        assert_eq!(cancelled, "r1");
        assert!(started.elapsed() < OFFLINE_AFTER, "{:?}", started.elapsed());
    }

    // F-154 kept: a visitor who keeps reading is not cut before the run's
    // saved timeout, even when its channel is full for a while after 30 s.
    #[tokio::test(start_paused = true)]
    async fn a_reading_visitor_keeps_a_long_run_past_30s() {
        let (mut agent, mut events) = relayed_run_with_45s_timeout().await;
        tokio::time::sleep(Duration::from_secs(35)).await;
        fill_visitor_channel(&mut agent, &events, "r1").await;
        agent
            .send_message(&TunnelMessage::Output {
                run_id: "r1".into(),
                line: "after 35 s".into(),
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        for n in 0..RELAY_EVENT_CAPACITY {
            assert_eq!(
                events.recv().await,
                Some(RelayEvent::Line(format!("line {n}")))
            );
        }
        assert_eq!(
            events.recv().await,
            Some(RelayEvent::Line("after 35 s".into()))
        );
        send_raw(
            &mut agent,
            serde_json::json!({"Done": {"run_id": "r1", "ok": true}}),
        )
        .await;
        assert!(matches!(
            events.recv().await,
            Some(RelayEvent::Terminal { error: None, .. })
        ));
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
                    limits: None,
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
            matches!(terminal, RelayEvent::Terminal { error: Some(_), .. }),
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
            async move { verify_agent(&store, &id, &cred).await }
        })
        .await;

        assert!(result.is_err(), "an unknown/revoked agent must be refused");
        assert!(
            client.await.unwrap().is_err(),
            "the agent's handshake fails closed too"
        );
    }

    /// A central channel whose peer is an agent built before `Cancel`: its hello
    /// has no `accepts_cancel`. The raw transport is returned so a test sees every
    /// frame central sends.
    async fn legacy_pair() -> (AuthChannel<ChannelTransport>, ChannelTransport) {
        let (mut agent_side, central_side) = ChannelTransport::pair();
        let hello = serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "agent_id": "agent-1",
            "credential": CRED,
            "client_nonce": vec![1u8; TUNNEL_KEY_BYTES],
        });
        agent_side
            .send(serde_json::to_vec(&hello).unwrap())
            .await
            .unwrap();
        let (_id, central_channel) = server_handshake(
            central_side,
            [2u8; 32],
            |_, cred| async move { cred == CRED },
        )
        .await
        .expect("server handshake");
        let _accept = agent_side.recv().await.unwrap();
        (central_channel, agent_side)
    }

    // Finding 1 (a) + compatibility: an agent built before `Cancel` cannot decode
    // it (its relay loop errors out and drops the tunnel), so central never sends
    // it one. When such an agent's consumer disconnects mid-run the connection is
    // reset rather than reused, so no leftover frame can bleed into a later run.
    #[tokio::test]
    async fn a_consumer_drop_resets_a_legacy_agent_without_sending_it_cancel() {
        let (central_channel, mut legacy) = legacy_pair().await;
        assert!(!central_channel.peer_accepts_cancel());
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
        drop(events);

        let mut frames = Vec::new();
        while let Some(frame) = tokio::time::timeout(Duration::from_secs(2), legacy.recv())
            .await
            .expect("central must close the legacy tunnel promptly")
            .unwrap()
        {
            // counter(8) + tag(32), then the JSON message.
            frames.push(serde_json::from_slice::<TunnelMessage>(&frame[40..]).unwrap());
        }
        assert_eq!(
            frames,
            vec![command("r1")],
            "a legacy agent must only ever be sent frames it can decode"
        );
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

    // A visitor leaving a remote run cancels that run only. The tunnel
    // stays up, output already in flight is drained, and the node takes the next
    // run on the same connection.
    #[tokio::test]
    async fn cancelling_a_remote_run_keeps_the_node_available() {
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
            .submit("agent-1", command("r1"))
            .await
            .expect("connected");
        assert_eq!(agent_channel.recv_message().await.unwrap(), command("r1"));
        agent_channel
            .send_message(&TunnelMessage::Output {
                run_id: "r1".into(),
                line: "first".into(),
            })
            .await
            .unwrap();
        assert_eq!(events.recv().await, Some(RelayEvent::Line("first".into())));

        // The visitor closes the stream mid-run.
        drop(events);
        let cancel = tokio::time::timeout(Duration::from_secs(2), agent_channel.recv_message())
            .await
            .expect("central must cancel the run rather than drop the tunnel")
            .unwrap();
        assert_eq!(
            cancel,
            TunnelMessage::Cancel {
                run_id: "r1".into()
            }
        );
        agent_channel
            .send_message(&TunnelMessage::Output {
                run_id: "r1".into(),
                line: "in flight".into(),
            })
            .await
            .unwrap();
        agent_channel
            .send_message(&TunnelMessage::Done {
                run_id: "r1".into(),
                ok: false,
                status: None,
            })
            .await
            .unwrap();

        let mut next = None;
        for _ in 0..200 {
            match hub.submit("agent-1", command("r2")).await {
                Ok(events) => {
                    next = Some(events);
                    break;
                }
                Err(SubmitError::Busy) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(error) => panic!("the node must stay connected after a cancel: {error:?}"),
            }
        }
        let mut next = next.expect("the node must be free for the next run");
        assert_eq!(agent_channel.recv_message().await.unwrap(), command("r2"));
        agent_channel
            .send_message(&TunnelMessage::Done {
                run_id: "r2".into(),
                ok: true,
                status: None,
            })
            .await
            .unwrap();
        assert_eq!(
            next.recv().await,
            Some(RelayEvent::Terminal {
                error: None,
                status: shared::exec::ExecStatus::Completed { success: true },
            }),
            "the cancelled run's leftovers must not bleed into the next run"
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
            matches!(event, RelayEvent::Terminal { error: Some(_), .. }),
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
        // D-AFK-1: the queued visitor is not left without a terminal. It gets
        // exactly one event, the revoke terminal, and no work output.
        let terminal = tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
            .await
            .expect("the queued visitor is not left hanging");
        assert!(
            matches!(
                &terminal,
                Some(RelayEvent::Terminal { error: Some(message), status: ExecStatus::Failed })
                    if message == REVOKED
            ),
            "stale revoked work must end with the revoke terminal only, got {terminal:?}"
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
                .await
                .expect("the channel closes after the terminal"),
            None,
            "nothing follows the revoke terminal"
        );
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

    #[tokio::test(start_paused = true)]
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
            matches!(terminal, RelayEvent::Terminal { error: Some(_), .. }),
            "revoking an active relay must surface a terminal error, got {terminal:?}"
        );
        wait_for(|| !hub.is_connected("agent-1")).await;
        assert!(
            !hub.is_connected("agent-1"),
            "a kicked active relay must unregister the tunnel"
        );
    }

    #[derive(Debug)]
    struct LivenessLoadMetrics {
        agents: usize,
        completed: Duration,
        p50: Duration,
        p95: Duration,
        max: Duration,
        heartbeat_failures: usize,
        health_max: Duration,
        output: Duration,
        gate: LivenessWriteGateTransitions,
    }

    async fn liveness_load(agents: usize) -> LivenessLoadMetrics {
        let store = Store::open(unique_db_path()).expect("open load store");
        let hub = TunnelHub::new();
        let mut channels = Vec::with_capacity(agents);

        for index in 0..agents {
            let agent_id = format!("load-agent-{index}");
            enrolled_agent(&store, &agent_id);
            let (central_channel, agent_channel) = established_pair_for(&agent_id).await;
            let serving_hub = hub.clone();
            let serving_store = store.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, serving_hub, serving_store, agent_id).await;
            });
            channels.push(agent_channel);
        }

        let connected_deadline = Instant::now() + Duration::from_secs(10);
        while (0..agents).any(|index| {
            let agent_id = format!("load-agent-{index}");
            !hub.is_connected(&agent_id)
                || store
                    .get_agent(&agent_id)
                    .expect("read connected agent")
                    .and_then(|agent| agent.last_seen)
                    .is_none()
        }) {
            assert!(
                Instant::now() < connected_deadline,
                "{agents} agent connections or initial liveness writes exceeded the 10 second heartbeat interval"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let output_channel = channels.remove(0);

        // `Liveness` writes once per second. Cross a second boundary so these frames
        // exercise the production Store::touch_agent_last_seen write rather than its
        // intentional same-second coalescing.
        let latest_seen = (1..agents)
            .map(|index| {
                store
                    .get_agent(&format!("load-agent-{index}"))
                    .expect("read liveness")
                    .expect("agent exists")
                    .last_seen
                    .expect("connected agent has liveness")
            })
            .max()
            .expect("heartbeat agents");
        while crate::store::unix_now() <= latest_seen {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let mut writes = Vec::with_capacity(agents);
        let mut heartbeat_failures = 0;
        let dispatched = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let previous_liveness = (1..agents)
            .map(|index| {
                let agent_id = format!("load-agent-{index}");
                let last_seen = store
                    .get_agent(&agent_id)
                    .expect("read before heartbeat")
                    .expect("agent exists")
                    .last_seen
                    .expect("connected agent has liveness");
                (agent_id, last_seen)
            })
            .collect::<Vec<_>>();
        let writes_started = Instant::now();
        let gate = arm_liveness_write_gate(Some(&store));
        for (index, mut channel) in channels.into_iter().enumerate() {
            let dispatched = dispatched.clone();
            writes.push(tokio::spawn(async move {
                channel
                    .send_message(&TunnelMessage::Heartbeat)
                    .await
                    .expect("heartbeat transport");
                dispatched.fetch_add(1, Ordering::Release);
                index
            }));
        }

        let expected_heartbeats = agents - 1;
        let overlap_deadline = Instant::now() + Duration::from_secs(2);
        while dispatched.load(Ordering::Acquire) != expected_heartbeats {
            assert!(
                Instant::now() < overlap_deadline,
                "heartbeats were not all dispatched before the responsiveness probes"
            );
            tokio::task::yield_now().await;
        }
        gate.wait_for_entry();
        let monitor_store = store.clone();
        let write_monitor = tokio::spawn(async move {
            let mut latencies = vec![None; previous_liveness.len()];
            let deadline = Instant::now() + Duration::from_secs(10);
            while latencies.iter().any(Option::is_none) {
                for (index, (agent_id, previous)) in previous_liveness.iter().enumerate() {
                    if latencies[index].is_none()
                        && monitor_store
                            .get_agent(agent_id)
                            .expect("read after heartbeat")
                            .expect("agent exists")
                            .last_seen
                            .is_some_and(|seen| seen > *previous)
                    {
                        latencies[index] = Some(writes_started.elapsed());
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "heartbeat writes exceeded the 10 second liveness interval"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            latencies
                .into_iter()
                .map(|latency| latency.expect("completed heartbeat latency"))
                .collect::<Vec<_>>()
        });

        let health_router = crate::with_routes(axum::Router::new());
        let health_probe = tokio::spawn(async move {
            let mut latencies = Vec::new();
            for _ in 0..3 {
                let probe_started = Instant::now();
                let response = health_router
                    .clone()
                    .oneshot(
                        axum::http::Request::builder()
                            .uri("/health")
                            .body(axum::body::Body::empty())
                            .expect("health request"),
                    )
                    .await
                    .expect("health response");
                assert_eq!(response.status(), axum::http::StatusCode::OK);
                latencies.push(probe_started.elapsed());
            }
            latencies
        });

        let mut events = hub
            .submit("load-agent-0", command("load-output"))
            .await
            .expect("the output probe agent is connected");
        let output_started = Instant::now();
        let output_task = tokio::spawn(async move {
            let mut channel = output_channel;
            assert_eq!(
                channel.recv_message().await.expect("output probe command"),
                command("load-output")
            );
            channel
                .send_message(&TunnelMessage::Output {
                    run_id: "load-output".into(),
                    line: "load probe".into(),
                })
                .await
                .expect("stream output");
            channel
                .send_message(&TunnelMessage::Done {
                    run_id: "load-output".into(),
                    ok: true,
                    status: None,
                })
                .await
                .expect("complete output probe");
        });

        let output = tokio::time::timeout(Duration::from_secs(2), async {
            output_task.await.expect("output task");
            assert_eq!(
                events.recv().await,
                Some(RelayEvent::Line("load probe".into()))
            );
            assert_eq!(
                events.recv().await,
                Some(RelayEvent::Terminal {
                    error: None,
                    status: shared::exec::ExecStatus::Completed { success: true },
                })
            );
            output_started.elapsed()
        })
        .await
        .expect("output stalled beyond the two second responsiveness budget");
        gate.record_output_completion();
        let health_latencies = health_probe.await.expect("health probe task");
        gate.record_health_completion();
        gate.release();

        for write in writes {
            match write.await {
                Ok(_) => {}
                Err(_) => heartbeat_failures += 1,
            }
        }
        let gate_transitions = gate.transitions();
        assert!(gate_transitions.entered, "liveness gate entered");
        assert!(
            gate_transitions.health_completed,
            "health probe completed while the liveness gate was held and not departed"
        );
        assert!(
            gate_transitions.output_completed,
            "output probe completed while the liveness gate was held and not departed"
        );
        assert!(gate_transitions.released, "liveness gate released");
        let mut latencies = write_monitor.await.expect("heartbeat write monitor");
        let gate_transitions = gate.transitions();
        assert!(
            !gate_transitions.held && gate_transitions.departed,
            "the liveness write departed only after release and monitor completion"
        );
        let health_max = *health_latencies.iter().max().expect("health samples");
        assert!(
            health_max <= Duration::from_secs(2),
            "health stalled for {health_max:?} while {agents} liveness writes ran"
        );
        assert_eq!(heartbeat_failures, 0, "heartbeat tasks failed");
        assert_eq!(
            latencies.len(),
            agents - 1,
            "every heartbeat write completed"
        );
        latencies.push(output);
        latencies.sort_unstable();
        // Measured from the heartbeat burst, not test start: enrolling and
        // connecting the agents is fixture cost (500 sequential fsynced inserts,
        // seconds on macOS) and already has its own 10s connect assertion above.
        let completed = writes_started.elapsed();
        assert!(
            completed <= Duration::from_secs(10),
            "{agents} liveness writes completed in {completed:?}, over the 10 second heartbeat interval"
        );

        LivenessLoadMetrics {
            agents,
            completed,
            p50: latencies[(latencies.len() - 1) / 2],
            p95: latencies[(latencies.len() - 1) * 95 / 100],
            max: *latencies.last().expect("write latencies"),
            heartbeat_failures,
            health_max,
            output,
            gate: gate_transitions,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn liveness_writes_hold_health_and_output_responsive_at_100_and_500_agents() {
        let _gate_test_lock = LIVENESS_WRITE_GATE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        for round in 1..=2 {
            for agents in [100, 500] {
                let metrics = liveness_load(agents).await;
                assert!(
                    liveness_write_gate().is_none(),
                    "liveness load gate registry disarms after every round"
                );
                eprintln!(
                    "liveness-load round={round} agents={} completed_ms={} p50_ms={} p95_ms={} max_ms={} heartbeat_failures={} health_max_ms={} output_ms={} gate=entry>held>health+output>release>departed entry={} held={} health_completed={} output_completed={} released={} departed={}",
                    metrics.agents,
                    metrics.completed.as_millis(),
                    metrics.p50.as_millis(),
                    metrics.p95.as_millis(),
                    metrics.max.as_millis(),
                    metrics.heartbeat_failures,
                    metrics.health_max.as_millis(),
                    metrics.output.as_millis(),
                    metrics.gate.entered,
                    metrics.gate.held,
                    metrics.gate.health_completed,
                    metrics.gate.output_completed,
                    metrics.gate.released,
                    metrics.gate.departed,
                );
                assert!(metrics.output <= Duration::from_secs(2));
            }
        }
    }

    #[tokio::test]
    async fn liveness_write_gate_cleanup_releases_and_disarms_after_entry() {
        let _gate_test_lock = LIVENESS_WRITE_GATE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let gate = arm_liveness_write_gate(None);
        let entered_gate = gate.gate.clone();
        let observed_gate = entered_gate.clone();
        let blocked_touch = std::thread::spawn(move || entered_gate.enter());
        gate.wait_for_entry();
        drop(gate);
        blocked_touch.join().expect("released liveness gate task");
        let transitions = observed_gate.transitions();
        assert!(transitions.released, "cleanup releases an entered gate");
        assert!(
            !transitions.held && transitions.departed,
            "cleanup lets an entered liveness write depart"
        );
        assert!(
            liveness_write_gate().is_none(),
            "cleanup disarms the liveness write gate registry"
        );
    }

    #[tokio::test]
    async fn liveness_write_gate_cleanup_releases_and_disarms_after_assertion() {
        let _gate_test_lock = LIVENESS_WRITE_GATE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let gate = arm_liveness_write_gate(None);
        let entered_gate = gate.gate.clone();
        let observed_gate = entered_gate.clone();
        let blocked_touch = std::thread::spawn(move || entered_gate.enter());
        gate.wait_for_entry();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gate.release())).is_err(),
            "release rejects a gate whose probes have not completed"
        );
        drop(gate);
        blocked_touch
            .join()
            .expect("assertion cleanup releases liveness gate task");
        let transitions = observed_gate.transitions();
        assert!(transitions.released, "assertion cleanup releases the gate");
        assert!(
            !transitions.held && transitions.departed,
            "assertion cleanup lets the liveness write depart"
        );
        assert!(
            liveness_write_gate().is_none(),
            "assertion cleanup disarms the liveness write gate registry"
        );
    }

    #[tokio::test]
    async fn liveness_write_gate_cleanup_disarms_before_late_entry() {
        let _gate_test_lock = LIVENESS_WRITE_GATE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let gate = arm_liveness_write_gate(None);
        let late_gate = gate.gate.clone();
        let observed_gate = late_gate.clone();
        drop(gate);
        std::thread::spawn(move || late_gate.enter())
            .join()
            .expect("late liveness gate entry returns after cleanup");
        let transitions = observed_gate.transitions();
        assert!(transitions.entered, "late entry reaches the test gate");
        assert!(transitions.released, "cleanup releases a late entry");
        assert!(
            !transitions.held && transitions.departed,
            "late liveness gate entry departs without blocking"
        );
        assert!(
            liveness_write_gate().is_none(),
            "late-entry cleanup leaves the liveness write gate disarmed"
        );
    }

    // F-372: the gate is process-wide, so a heartbeat flush into another test's
    // store must not claim it while a gate test holds it armed.
    #[tokio::test]
    async fn another_stores_liveness_flush_cannot_claim_an_armed_gate() {
        let _gate_test_lock = LIVENESS_WRITE_GATE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let gate = arm_liveness_write_gate(Some(&Store::open(unique_db_path()).unwrap()));
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let batch = Arc::new(LivenessBatch::default());
        batch
            .pending
            .lock()
            .unwrap()
            .insert("agent-1".to_string(), 1);
        batch.flushing.store(true, Ordering::Release);

        let flushed = std::thread::spawn(move || batch.flush(&store)).join();

        assert!(
            flushed.is_ok() && !gate.transitions().entered,
            "a foreign store's flush claimed the armed liveness write gate"
        );
    }

    // F-371: a store fault reading the agent still fails closed, but its cause
    // reaches the operator log instead of passing for a revoke.
    #[tokio::test]
    async fn agent_store_failure_fails_closed_and_is_logged_with_its_cause() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let txn = store.database().begin_write().unwrap();
        txn.delete_table(crate::store::AGENT).unwrap();
        txn.open_table(redb::TableDefinition::<u64, u64>::new("agent"))
            .unwrap();
        let cause = txn.open_table(crate::store::AGENT).unwrap_err().to_string();
        txn.commit().unwrap();
        let (logs, _guard) = crate::auth::tests::capture_logs();

        assert!(!verify_agent(&store, "agent-1", CRED).await);
        let liveness = Liveness::new(store, Arc::default(), "agent-1".to_string());
        assert!(liveness.is_revoked_or_missing());

        let logs = logs.text();
        let logged = logs
            .lines()
            .filter(|line| line.contains("ERROR") && line.contains(&cause))
            .count();
        assert_eq!(logged, 2, "{logs}");
    }

    // F-379: a store fault reading the agent or its location still withholds the
    // data-plane origin, but its cause reaches the operator log.
    #[tokio::test]
    async fn data_plane_origin_store_failures_are_logged_with_their_cause() {
        let (logs, _guard) = crate::auth::tests::capture_logs();
        let mut causes = Vec::new();
        for (table, name) in [
            (crate::store::AGENT, "agent"),
            (crate::store::LOCATION, "location"),
        ] {
            let store = Store::open(unique_db_path()).unwrap();
            enrolled_agent(&store, "agent-1");
            let txn = store.database().begin_write().unwrap();
            txn.delete_table(table).unwrap();
            txn.open_table(redb::TableDefinition::<u64, u64>::new(name))
                .unwrap();
            causes.push(txn.open_table(table).unwrap_err().to_string());
            txn.commit().unwrap();

            let liveness = Liveness::new(store, Arc::default(), "agent-1".to_string());
            assert_eq!(liveness.data_plane_origin(), None);
        }

        let logs = logs.text();
        for cause in causes {
            assert!(
                logs.lines()
                    .any(|line| line.contains("ERROR") && line.contains(&cause)),
                "{cause} not logged at ERROR: {logs}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
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

        // The hub forgets the agent at once; the relay itself proves it tore
        // down by dropping the connection, well before its stall bound.
        let closed = tokio::time::timeout(Duration::from_secs(1), agent_channel.recv_message())
            .await
            .expect("a kicked relay blocked on visitor backpressure must tear down promptly");
        assert!(
            closed.is_err(),
            "the kicked connection must close: {closed:?}"
        );
        assert!(!hub.is_connected("agent-1"));
        drop(events);
    }

    #[tokio::test(start_paused = true)]
    async fn relay_run_shutdown_wins_while_line_send_is_backpressured() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let (mut central_channel, mut agent_channel) = established_pair().await;
        let mut liveness = Liveness::new(store, Arc::default(), "agent-1".to_string());
        let (events_tx, events_rx) = mpsc::channel(1);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let relay = tokio::spawn(async move {
            relay_run(
                &mut central_channel,
                command("r-backpressure"),
                events_tx,
                RELAY_RUN_DEADLINE,
                None,
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

    // A visitor who stops reading must not pin the node. Once the run
    // deadline passes with the consumer still full, central cancels the run and
    // keeps the connection for the next job.
    #[tokio::test(start_paused = true)]
    async fn a_stalled_consumer_is_released_after_the_run_deadline() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let (mut central_channel, mut agent_channel) = established_pair().await;
        let mut liveness = Liveness::new(store, Arc::default(), "agent-1".to_string());
        let (events_tx, events_rx) = mpsc::channel(1);
        let (_shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let relay = tokio::spawn(async move {
            relay_run(
                &mut central_channel,
                command("r-stall"),
                events_tx,
                Duration::from_millis(200),
                None,
                &mut liveness,
                &mut shutdown_rx,
            )
            .await
        });

        assert_eq!(
            agent_channel.recv_message().await.unwrap(),
            command("r-stall")
        );
        for line in ["first", "blocked"] {
            agent_channel
                .send_message(&TunnelMessage::Output {
                    run_id: "r-stall".into(),
                    line: line.into(),
                })
                .await
                .unwrap();
        }

        let cancel = tokio::time::timeout(Duration::from_secs(2), agent_channel.recv_message())
            .await
            .expect("a stalled consumer must be released after the run deadline")
            .unwrap();
        assert_eq!(
            cancel,
            TunnelMessage::Cancel {
                run_id: "r-stall".into()
            }
        );
        agent_channel
            .send_message(&TunnelMessage::Done {
                run_id: "r-stall".into(),
                ok: false,
                status: None,
            })
            .await
            .unwrap();
        let control = tokio::time::timeout(Duration::from_secs(2), relay)
            .await
            .expect("the relay must end once the cancelled run terminates")
            .expect("relay task");
        assert!(matches!(control, ConnectionControl::KeepAlive));
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
                asn: None,
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

    /// A store holding remote location "loc-1" (data-plane `origin`) and its
    /// enrolled "agent-1".
    fn remote_store(origin: Option<&str>) -> Store {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let store = Store::open(unique_db_path().with_extension(format!("dp{n}.redb"))).unwrap();
        store
            .put_location(&crate::store::Location {
                id: "loc-1".to_string(),
                name: "Remote".to_string(),
                geo_label: "DE".to_string(),
                map_query: None,
                facility: None,
                facility_url: None,
                kind: crate::store::NodeKind::Remote,
                data_plane_origin: origin.map(str::to_string),
                asn: None,
                offered_methods: vec![],
                status: crate::store::LocationStatus::Offline,
                created_at: 0,
            })
            .unwrap();
        enrolled_agent(&store, "agent-1");
        store
    }

    fn serve_loc1(store: &Store, central_channel: AuthChannel<ChannelTransport>) -> TunnelHub {
        let hub = TunnelHub::new();
        let (hub_task, store) = (hub.clone(), store.clone());
        tokio::spawn(async move {
            serve_agent(central_channel, hub_task, store, "agent-1".to_string()).await
        });
        hub
    }

    // Central hands a connected agent its location's data-plane origin,
    // and hands it the new one after an admin edit (checked on the agent's next
    // idle frame, so within one heartbeat).
    #[tokio::test]
    async fn central_sends_the_location_data_plane_origin_to_the_agent() {
        let store = remote_store(Some("https://node.example.test"));
        let (central_channel, mut agent) = established_pair().await;
        let _hub = serve_loc1(&store, central_channel);

        let first = tokio::time::timeout(Duration::from_secs(2), agent.recv_message())
            .await
            .expect("central must send the data-plane origin on connect")
            .unwrap();
        assert_eq!(
            first,
            TunnelMessage::DataPlane {
                origin: "https://node.example.test".to_string()
            }
        );

        let mut location = store.get_location("loc-1").unwrap().unwrap();
        location.data_plane_origin = Some("https://203.0.113.10".to_string());
        store.update_location(&location).unwrap();
        agent.send_message(&TunnelMessage::Heartbeat).await.unwrap();
        let changed = tokio::time::timeout(Duration::from_secs(2), agent.recv_message())
            .await
            .expect("an edited origin reaches the agent on its next frame")
            .unwrap();
        assert_eq!(
            changed,
            TunnelMessage::DataPlane {
                origin: "https://203.0.113.10".to_string()
            }
        );
        // An unchanged origin is not re-sent on every heartbeat.
        agent.send_message(&TunnelMessage::Heartbeat).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), agent.recv_message())
                .await
                .is_err(),
            "an unchanged origin is sent once"
        );
    }

    // An origin stored before the https-on-443 rule (http://, or another port)
    // is withheld from the agent until the admin re-saves a valid one.
    #[tokio::test]
    async fn a_stored_origin_the_rule_now_refuses_is_not_sent_to_the_agent() {
        for legacy in ["http://node.example.test", "https://node.example.test:8443"] {
            let store = remote_store(Some(legacy));
            let (central_channel, mut agent) = established_pair().await;
            let _hub = serve_loc1(&store, central_channel);
            let sent = tokio::time::timeout(Duration::from_millis(500), agent.recv_message()).await;
            assert!(sent.is_err(), "{legacy:?} must not be sent: {sent:?}");

            let mut location = store.get_location("loc-1").unwrap().unwrap();
            location.data_plane_origin = Some("https://node.example.test".to_string());
            store.update_location(&location).unwrap();
            agent.send_message(&TunnelMessage::Heartbeat).await.unwrap();
            let resaved = tokio::time::timeout(Duration::from_secs(2), agent.recv_message())
                .await
                .expect("the re-saved origin reaches the agent on its next frame")
                .unwrap();
            assert_eq!(
                resaved,
                TunnelMessage::DataPlane {
                    origin: "https://node.example.test".to_string()
                }
            );
        }
    }

    // Compatibility: an agent built before `DataPlane` cannot decode it, so
    // central never sends it one, even when its location has an origin.
    #[tokio::test]
    async fn a_legacy_agent_is_never_sent_a_data_plane_origin() {
        let store = remote_store(Some("https://node.example.test"));
        let (central_channel, mut legacy) = legacy_pair().await;
        assert!(!central_channel.peer_accepts_data_plane());
        let hub = serve_loc1(&store, central_channel);
        wait_for(|| hub.is_connected("agent-1")).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(500), legacy.recv())
                .await
                .is_err(),
            "a legacy agent must only ever be sent frames it can decode"
        );
        assert!(hub.is_connected("agent-1"));
    }

    // The agent's certificate report lands in the store for the admin
    // editor, whether it arrives while the node is idle or mid-run, and the run
    // it interleaves with still ends normally on the same connection.
    #[tokio::test]
    async fn an_agent_certificate_report_reaches_the_store() {
        let store = remote_store(Some("https://node.example.test"));
        let (central_channel, mut agent) = established_pair().await;
        let hub = serve_loc1(&store, central_channel);
        wait_for(|| hub.is_connected("agent-1")).await;

        let idle = shared::protocol::CertificateStatus {
            issued_at: Some(100),
            expires_at: Some(700),
            last_error: None,
        };
        send_raw(
            &mut agent,
            serde_json::json!({"Certificate": {"issued_at": 100, "expires_at": 700, "last_error": null}}),
        )
        .await;
        wait_for(|| store.get_certificate_status("loc-1").unwrap().is_some()).await;
        assert_eq!(store.get_certificate_status("loc-1").unwrap(), Some(idle));

        let mut events = submit_when_free(&hub, "r1").await;
        loop {
            if let TunnelMessage::Command { .. } = agent.recv_message().await.unwrap() {
                break;
            }
        }
        let failed = shared::protocol::CertificateStatus {
            issued_at: Some(100),
            expires_at: Some(700),
            last_error: Some("renewal failed".to_string()),
        };
        agent
            .send_message(&TunnelMessage::Certificate(CertificateReport {
                status: failed.clone(),
                origin: None,
            }))
            .await
            .unwrap();
        agent
            .send_message(&TunnelMessage::done(
                "r1",
                ExecStatus::Completed { success: true },
            ))
            .await
            .unwrap();
        assert!(matches!(
            events.recv().await,
            Some(RelayEvent::Terminal { error: None, .. })
        ));
        assert_eq!(store.get_certificate_status("loc-1").unwrap(), Some(failed));
        assert!(hub.is_connected("agent-1"), "the run kept the tunnel");
    }

    // F-207: a report that crosses an admin's origin edit is for the old
    // origin, so it is not recorded as the new origin's certificate.
    #[tokio::test]
    async fn a_certificate_report_for_a_replaced_origin_is_not_recorded() {
        let store = remote_store(Some("https://node.example.test"));
        let (central_channel, mut agent) = established_pair().await;
        let _hub = serve_loc1(&store, central_channel);
        assert!(matches!(
            agent.recv_message().await.unwrap(),
            TunnelMessage::DataPlane { .. }
        ));

        let mut location = store.get_location("loc-1").unwrap().unwrap();
        location.data_plane_origin = Some("https://203.0.113.10".to_string());
        store.update_location(&location).unwrap();
        let old_origin = shared::protocol::CertificateStatus {
            issued_at: Some(100),
            expires_at: Some(700),
            last_error: None,
        };
        agent
            .send_message(&TunnelMessage::Certificate(CertificateReport {
                status: old_origin,
                origin: None,
            }))
            .await
            .unwrap();
        // Central handles the report before it syncs the new origin.
        assert_eq!(
            agent.recv_message().await.unwrap(),
            TunnelMessage::DataPlane {
                origin: "https://203.0.113.10".to_string()
            }
        );
        assert_eq!(
            store.get_certificate_status("loc-1").unwrap(),
            None,
            "the old origin's certificate must not be recorded for the new one"
        );
    }

    // F-207: a report names the origin it was obtained for, so one for an
    // origin since replaced is dropped even when it arrives after central sent
    // the new origin (a report sent across a reconnect, or crossing the new
    // origin in flight); a report for the current origin is recorded.
    #[tokio::test]
    async fn a_certificate_report_is_recorded_only_for_the_origin_it_names() {
        let store = remote_store(Some("https://node.example.test"));
        let (central_channel, mut agent) = established_pair().await;
        let _hub = serve_loc1(&store, central_channel);
        assert!(matches!(
            agent.recv_message().await.unwrap(),
            TunnelMessage::DataPlane { .. }
        ));
        let set_origin = |origin: &str| {
            let mut location = store.get_location("loc-1").unwrap().unwrap();
            location.data_plane_origin = Some(origin.to_string());
            store.update_location(&location).unwrap();
        };
        let report = |issued_at: u64, origin: &str| {
            TunnelMessage::Certificate(CertificateReport {
                status: shared::protocol::CertificateStatus {
                    issued_at: Some(issued_at),
                    expires_at: Some(700),
                    last_error: None,
                },
                origin: Some(origin.to_string()),
            })
        };

        set_origin("https://203.0.113.10");
        agent.send_message(&TunnelMessage::Heartbeat).await.unwrap();
        assert_eq!(
            agent.recv_message().await.unwrap(),
            TunnelMessage::DataPlane {
                origin: "https://203.0.113.10".to_string()
            }
        );
        // Central has sent the new origin; a report for the old one follows.
        agent
            .send_message(&report(100, "https://node.example.test"))
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while tokio::time::Instant::now() < deadline {
            assert_eq!(
                store.get_certificate_status("loc-1").unwrap(),
                None,
                "a report for the old origin must not be recorded for the new one"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        agent
            .send_message(&report(200, "https://203.0.113.10"))
            .await
            .unwrap();
        wait_for(|| store.get_certificate_status("loc-1").unwrap().is_some()).await;
        assert_eq!(
            store
                .get_certificate_status("loc-1")
                .unwrap()
                .and_then(|status| status.issued_at),
            Some(200),
            "a report for the current origin is recorded"
        );
    }

    /// Start `serve_agent` for an enrolled "agent-1" and wait until it is registered.
    async fn serving_hub(central_channel: AuthChannel<ChannelTransport>) -> TunnelHub {
        // A counter on top of `unique_db_path`, whose clock can repeat across
        // parallel tests ("Database already open").
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let store = Store::open(unique_db_path().with_extension(format!("{n}.redb"))).unwrap();
        enrolled_agent(&store, "agent-1");
        let hub = TunnelHub::new();
        {
            let hub = hub.clone();
            tokio::spawn(async move {
                serve_agent(central_channel, hub, store, "agent-1".to_string()).await
            });
        }
        wait_for(|| hub.is_connected("agent-1")).await;
        hub
    }

    /// Submit `run_id` once the node has finished its previous run.
    async fn submit_when_free(hub: &TunnelHub, run_id: &str) -> mpsc::Receiver<RelayEvent> {
        for _ in 0..200 {
            match hub.submit("agent-1", command(run_id)).await {
                Ok(events) => return events,
                Err(SubmitError::Busy) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(error) => panic!("the node must stay connected: {error:?}"),
            }
        }
        panic!("the node must be free for run {run_id}");
    }

    /// Send a frame exactly as JSON, the way an agent of another version encodes it.
    async fn send_raw(agent: &mut AuthChannel<ChannelTransport>, frame: serde_json::Value) {
        agent
            .send(&serde_json::to_vec(&frame).unwrap())
            .await
            .unwrap();
    }

    async fn sse_body(response: impl axum::response::IntoResponse) -> String {
        let body = axum::body::to_bytes(response.into_response().into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    /// An SSE body with its `done` elapsed time blanked: the relay measures its
    /// own, so only the rest of the stream is compared with a local run.
    fn without_elapsed(body: &str) -> String {
        let (head, tail) = body.split_once(r#""elapsed_ms":"#).expect("a done event");
        let tail = tail.trim_start_matches(|c: char| c.is_ascii_digit());
        format!(r#"{head}"elapsed_ms":_{tail}"#)
    }

    /// What a local run streams when it fails with `error` (if any) and ends
    /// with `status`.
    async fn local_sse(error: Option<&str>, status: shared::exec::ExecStatus) -> String {
        let (tx, rx) = mpsc::channel(2);
        if let Some(message) = error {
            tx.send(shared::exec::ExecEvent::Failed(message.into()))
                .await
                .unwrap();
        }
        tx.send(shared::exec::ExecEvent::Done {
            status,
            elapsed_ms: 0,
        })
        .await
        .unwrap();
        drop(tx);
        sse_body(crate::stream::sse_run(
            shared::exec::ExecHandle { events: rx },
            (),
        ))
        .await
    }

    // Older agent, newer central: an agent that sends Done after Error
    // must not end the next run with that late Done.
    #[tokio::test]
    async fn a_late_done_after_an_error_does_not_end_the_next_run() {
        let (central_channel, mut agent) = established_pair().await;
        let hub = serving_hub(central_channel).await;

        let mut first = hub.submit("agent-1", command("r1")).await.unwrap();
        assert_eq!(agent.recv_message().await.unwrap(), command("r1"));
        agent
            .send_message(&TunnelMessage::Error {
                run_id: "r1".into(),
                message: "the diagnostic tool is not available on this node".into(),
                status: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            first.recv().await,
            Some(RelayEvent::Terminal { error: Some(_), .. })
        ));

        let mut next = submit_when_free(&hub, "r2").await;
        assert_eq!(agent.recv_message().await.unwrap(), command("r2"));
        send_raw(
            &mut agent,
            serde_json::json!({"Done": {"run_id": "r1", "ok": false}}),
        )
        .await;
        agent
            .send_message(&TunnelMessage::Output {
                run_id: "r2".into(),
                line: "second run".into(),
            })
            .await
            .unwrap();
        send_raw(
            &mut agent,
            serde_json::json!({"Done": {"run_id": "r2", "ok": true}}),
        )
        .await;

        let event = tokio::time::timeout(Duration::from_secs(2), next.recv())
            .await
            .expect("no hang");
        assert_eq!(
            event,
            Some(RelayEvent::Line("second run".into())),
            "the previous run's late Done must not end this run"
        );
        assert!(matches!(
            next.recv().await,
            Some(RelayEvent::Terminal { error: None, .. })
        ));
        assert!(hub.is_connected("agent-1"));
    }

    // Run ids still match strictly: only the previous run's Done is dropped.
    #[tokio::test]
    async fn a_done_for_a_run_that_is_not_the_previous_one_is_rejected() {
        let (central_channel, mut agent) = established_pair().await;
        let hub = serving_hub(central_channel).await;

        let mut first = hub.submit("agent-1", command("r1")).await.unwrap();
        assert_eq!(agent.recv_message().await.unwrap(), command("r1"));
        agent
            .send_message(&TunnelMessage::Error {
                run_id: "r1".into(),
                message: "refused".into(),
                status: None,
            })
            .await
            .unwrap();
        assert!(first.recv().await.is_some());

        let mut next = submit_when_free(&hub, "r2").await;
        assert_eq!(agent.recv_message().await.unwrap(), command("r2"));
        send_raw(
            &mut agent,
            serde_json::json!({"Done": {"run_id": "r0", "ok": true}}),
        )
        .await;
        let event = tokio::time::timeout(Duration::from_secs(2), next.recv())
            .await
            .expect("a foreign Done is rejected at once, not skipped");
        assert!(matches!(
            event,
            Some(RelayEvent::Terminal { error: Some(_), .. })
        ));
        wait_for(|| !hub.is_connected("agent-1")).await;
        assert!(!hub.is_connected("agent-1"));
    }

    // Only a late `Done` is dropped: a late `Output` or `Error` from the previous
    // run is still a frame for a different run and ends the connection.
    #[tokio::test]
    async fn a_late_output_or_error_from_the_previous_run_is_rejected() {
        for late in [
            TunnelMessage::Output {
                run_id: "r1".into(),
                line: "late".into(),
            },
            TunnelMessage::Error {
                run_id: "r1".into(),
                message: "late".into(),
                status: None,
            },
        ] {
            let (central_channel, mut agent) = established_pair().await;
            let hub = serving_hub(central_channel).await;
            let mut first = hub.submit("agent-1", command("r1")).await.unwrap();
            assert_eq!(agent.recv_message().await.unwrap(), command("r1"));
            agent
                .send_message(&TunnelMessage::Error {
                    run_id: "r1".into(),
                    message: "refused".into(),
                    status: None,
                })
                .await
                .unwrap();
            assert!(first.recv().await.is_some());

            let mut next = submit_when_free(&hub, "r2").await;
            assert_eq!(agent.recv_message().await.unwrap(), command("r2"));
            agent.send_message(&late).await.unwrap();
            let event = tokio::time::timeout(Duration::from_secs(2), next.recv())
                .await
                .expect("a late non-Done frame is rejected at once");
            assert!(
                matches!(event, Some(RelayEvent::Terminal { error: Some(_), .. })),
                "{late:?} must end the run: {event:?}"
            );
            wait_for(|| !hub.is_connected("agent-1")).await;
            assert!(!hub.is_connected("agent-1"), "{late:?}");
        }
    }

    // The tolerated late `Done` follows the connection's most recent run, not its
    // first one: over three runs, the second run's late `Done` is dropped.
    #[tokio::test]
    async fn the_late_done_rule_tracks_the_most_recent_run() {
        let (central_channel, mut agent) = established_pair().await;
        let hub = serving_hub(central_channel).await;
        for run in ["r1", "r2"] {
            let mut events = submit_when_free(&hub, run).await;
            assert_eq!(agent.recv_message().await.unwrap(), command(run));
            agent
                .send_message(&TunnelMessage::Error {
                    run_id: run.into(),
                    message: "refused".into(),
                    status: None,
                })
                .await
                .unwrap();
            assert!(events.recv().await.is_some());
        }

        let mut third = submit_when_free(&hub, "r3").await;
        assert_eq!(agent.recv_message().await.unwrap(), command("r3"));
        send_raw(
            &mut agent,
            serde_json::json!({"Done": {"run_id": "r2", "ok": false}}),
        )
        .await;
        agent
            .send_message(&TunnelMessage::Output {
                run_id: "r3".into(),
                line: "third run".into(),
            })
            .await
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), third.recv())
            .await
            .expect("no hang");
        assert_eq!(
            event,
            Some(RelayEvent::Line("third run".into())),
            "the second run's late Done must be dropped, not treated as foreign"
        );
        assert!(hub.is_connected("agent-1"));
    }

    // A cancel drain keeps the connection only when it reaches the cancelled
    // run's own terminal. Another run's frame, or a lost channel, tears it down.
    #[tokio::test]
    async fn a_cancel_drain_tears_down_on_a_foreign_frame_or_a_lost_channel() {
        for foreign in [
            TunnelMessage::Output {
                run_id: "other".into(),
                line: "foreign".into(),
            },
            TunnelMessage::Done {
                run_id: "other".into(),
                ok: true,
                status: None,
            },
        ] {
            let store = Store::open(unique_db_path()).unwrap();
            enrolled_agent(&store, "agent-1");
            let (mut central_channel, mut agent) = established_pair().await;
            let mut liveness = Liveness::new(store, Arc::default(), "agent-1".to_string());
            let drain =
                tokio::spawn(
                    async move { cancel_run(&mut central_channel, "r1", &mut liveness).await },
                );
            assert_eq!(
                agent.recv_message().await.unwrap(),
                TunnelMessage::Cancel {
                    run_id: "r1".into()
                }
            );
            agent.send_message(&foreign).await.unwrap();
            agent
                .send_message(&TunnelMessage::Done {
                    run_id: "r1".into(),
                    ok: false,
                    status: None,
                })
                .await
                .unwrap();
            let control = tokio::time::timeout(Duration::from_secs(2), drain)
                .await
                .expect("the drain ends")
                .unwrap();
            assert!(
                matches!(control, ConnectionControl::TearDown),
                "{foreign:?} during a drain must tear the connection down"
            );
        }

        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let (mut central_channel, agent) = established_pair().await;
        let mut liveness = Liveness::new(store, Arc::default(), "agent-1".to_string());
        drop(agent);
        let control = tokio::time::timeout(
            Duration::from_secs(2),
            cancel_run(&mut central_channel, "r1", &mut liveness),
        )
        .await
        .expect("the drain ends");
        assert!(
            matches!(control, ConnectionControl::TearDown),
            "a drain whose channel is gone must tear the connection down"
        );
    }

    // An agent that never answers a cancel is dropped after the 10 s cancel
    // timeout instead of holding the node. The bound is a literal on purpose:
    // deriving it from RELAY_CANCEL_TIMEOUT would move with the constant.
    #[tokio::test(start_paused = true)]
    async fn a_cancel_drain_gives_up_after_the_cancel_timeout() {
        let store = Store::open(unique_db_path()).unwrap();
        enrolled_agent(&store, "agent-1");
        let (mut central_channel, mut agent) = established_pair().await;
        let mut liveness = Liveness::new(store, Arc::default(), "agent-1".to_string());
        let started = tokio::time::Instant::now();
        let control = tokio::time::timeout(
            Duration::from_secs(11),
            cancel_run(&mut central_channel, "r1", &mut liveness),
        )
        .await
        .expect("the drain must give up after 10 s");
        assert!(matches!(control, ConnectionControl::TearDown));
        assert!(started.elapsed() >= Duration::from_secs(10));
        assert_eq!(
            agent.recv_message().await.unwrap(),
            TunnelMessage::Cancel {
                run_id: "r1".into()
            }
        );
    }

    // A relayed timeout or non-zero exit streams what the same outcome
    // streams on a local node.
    #[tokio::test]
    async fn a_relayed_timeout_or_failed_exit_streams_like_a_local_run() {
        for (label, status) in [
            ("timeout", shared::exec::ExecStatus::TimedOut),
            (
                "completed",
                shared::exec::ExecStatus::Completed { success: false },
            ),
        ] {
            let (central_channel, mut agent) = established_pair().await;
            let hub = serving_hub(central_channel).await;
            let events = hub.submit("agent-1", command("r1")).await.unwrap();
            assert_eq!(agent.recv_message().await.unwrap(), command("r1"));
            send_raw(
                &mut agent,
                serde_json::json!({"Done": {"run_id": "r1", "ok": false, "status": label}}),
            )
            .await;
            assert_eq!(
                without_elapsed(&sse_body(crate::stream::sse_relay(events)).await),
                without_elapsed(&local_sse(None, status).await),
                "a relayed {label} run"
            );
        }
    }

    // A relayed run that hits the output cap ends as truncated, as a local run
    // does; an older agent's Error carries no status and stays failed.
    #[tokio::test]
    async fn a_relayed_truncated_run_streams_like_a_local_run() {
        let truncated = "output limit reached — the run was truncated";
        for (status, local) in [
            (Some("truncated"), shared::exec::ExecStatus::OutputCapped),
            (None, shared::exec::ExecStatus::Failed),
        ] {
            let (central_channel, mut agent) = established_pair().await;
            let hub = serving_hub(central_channel).await;
            let events = hub.submit("agent-1", command("r1")).await.unwrap();
            assert_eq!(agent.recv_message().await.unwrap(), command("r1"));
            let mut error = serde_json::json!({"run_id": "r1", "message": truncated});
            if let Some(status) = status {
                error["status"] = status.into();
            }
            send_raw(&mut agent, serde_json::json!({ "Error": error })).await;
            assert_eq!(
                without_elapsed(&sse_body(crate::stream::sse_relay(events)).await),
                without_elapsed(&local_sse(Some(truncated), local).await),
                "an Error with status {status:?}"
            );
        }
    }

    // Older agent, newer central: a Done without a status keeps today's terminal.
    #[tokio::test]
    async fn a_done_without_a_status_keeps_the_legacy_terminal() {
        let (central_channel, mut agent) = established_pair().await;
        let hub = serving_hub(central_channel).await;
        let events = hub.submit("agent-1", command("r1")).await.unwrap();
        assert_eq!(agent.recv_message().await.unwrap(), command("r1"));
        send_raw(
            &mut agent,
            serde_json::json!({"Done": {"run_id": "r1", "ok": false}}),
        )
        .await;
        let body = sse_body(crate::stream::sse_relay(events)).await;
        assert!(
            body.contains("event: run-error\ndata: the diagnostic finished with an error\n"),
            "{body}"
        );
        assert!(body.contains(r#""status":"failed""#), "{body}");
    }

    /// Fill the visitor's channel for `run_id` with lines it has not read.
    async fn fill_visitor_channel(
        agent: &mut AuthChannel<ChannelTransport>,
        events: &mpsc::Receiver<RelayEvent>,
        run_id: &str,
    ) {
        for n in 0..RELAY_EVENT_CAPACITY {
            agent
                .send_message(&TunnelMessage::Output {
                    run_id: run_id.into(),
                    line: format!("line {n}"),
                })
                .await
                .unwrap();
        }
        wait_for(|| events.len() == RELAY_EVENT_CAPACITY).await;
        assert_eq!(events.len(), RELAY_EVENT_CAPACITY, "the visitor is full");
    }

    // A revoke that lands while a line is blocked on a slow visitor still
    // ends that visitor's run as revoked, after the lines already queued.
    #[tokio::test]
    async fn a_revoke_during_a_blocked_line_send_ends_the_run_as_revoked() {
        let (central_channel, mut agent) = established_pair().await;
        let hub = serving_hub(central_channel).await;
        let mut events = hub.submit("agent-1", command("r1")).await.unwrap();
        assert_eq!(agent.recv_message().await.unwrap(), command("r1"));
        fill_visitor_channel(&mut agent, &events, "r1").await;
        agent
            .send_message(&TunnelMessage::Output {
                run_id: "r1".into(),
                line: "blocked".into(),
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(hub.kick_agents(&["agent-1".to_string()]), 1);
        wait_for(|| !hub.is_connected("agent-1")).await;

        for n in 0..RELAY_EVENT_CAPACITY {
            assert_eq!(
                events.recv().await,
                Some(RelayEvent::Line(format!("line {n}")))
            );
        }
        let terminal = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("no hang");
        assert!(
            matches!(
                &terminal,
                Some(RelayEvent::Terminal { error: Some(message), .. })
                    if message == "the remote agent was revoked"
            ),
            "got {terminal:?}"
        );
    }

    // A run that ends while its visitor's channel is full must
    // not pin the node on the terminal send; the visitor still gets the terminal.
    #[tokio::test(start_paused = true)]
    async fn a_run_ending_on_a_full_visitor_channel_does_not_pin_the_node() {
        let (central_channel, mut agent) = established_pair().await;
        let hub = serving_hub(central_channel).await;
        let mut events = hub.submit("agent-1", command("r1")).await.unwrap();
        assert_eq!(agent.recv_message().await.unwrap(), command("r1"));
        fill_visitor_channel(&mut agent, &events, "r1").await;
        send_raw(
            &mut agent,
            serde_json::json!({"Done": {"run_id": "r1", "ok": true}}),
        )
        .await;

        let mut next = None;
        for _ in 0..100 {
            match hub.submit("agent-1", command("r2")).await {
                Ok(events) => {
                    next = Some(events);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
        assert!(
            next.is_some(),
            "a full visitor channel must not hold the node after its run ended"
        );
        for n in 0..RELAY_EVENT_CAPACITY {
            assert_eq!(
                events.recv().await,
                Some(RelayEvent::Line(format!("line {n}")))
            );
        }
        assert!(matches!(
            events.recv().await,
            Some(RelayEvent::Terminal { error: None, .. })
        ));
    }

    // The background terminal send is bounded by the run deadline: a visitor
    // that never reads does not keep it (or its channel) alive past it.
    #[tokio::test(start_paused = true)]
    async fn an_undelivered_terminal_is_abandoned_at_the_run_deadline() {
        let store = Store::open(unique_db_path().with_extension("t015.redb")).unwrap();
        enrolled_agent(&store, "agent-1");
        let (mut central_channel, mut agent_channel) = established_pair().await;
        let mut liveness = Liveness::new(store, Arc::default(), "agent-1".to_string());
        let (events_tx, mut events_rx) = mpsc::channel(1);
        let (_shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let relay = tokio::spawn(async move {
            relay_run(
                &mut central_channel,
                command("r1"),
                events_tx,
                Duration::from_millis(200),
                None,
                &mut liveness,
                &mut shutdown_rx,
            )
            .await
        });
        assert_eq!(agent_channel.recv_message().await.unwrap(), command("r1"));
        agent_channel
            .send_message(&TunnelMessage::Output {
                run_id: "r1".into(),
                line: "first".into(),
            })
            .await
            .unwrap();
        wait_for(|| events_rx.len() == 1).await;
        send_raw(
            &mut agent_channel,
            serde_json::json!({"Done": {"run_id": "r1", "ok": true}}),
        )
        .await;
        let control = tokio::time::timeout(Duration::from_secs(1), relay)
            .await
            .expect("the relay ends without waiting for the visitor")
            .unwrap();
        assert!(matches!(control, ConnectionControl::KeepAlive));

        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            events_rx.recv().await,
            Some(RelayEvent::Line("first".into()))
        );
        assert_eq!(
            events_rx.recv().await,
            None,
            "past the run deadline the terminal send gives up and drops the channel"
        );
    }

    /// A store path no other test in this process shares: the clock alone can
    /// repeat across parallel tests (macOS reports microseconds), which made two
    /// tests open one file ("Database already open"). The counter cannot repeat.
    fn unique_db_path() -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        static NEXT_DB: AtomicU64 = AtomicU64::new(0);
        let n = NEXT_DB.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lg-tunnel-test-{}-{n}-{}.redb",
            std::process::id(),
            nanos
        ));
        path
    }

    #[test]
    fn unique_db_path_never_repeats_across_parallel_callers() {
        let paths: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| (0..500).map(|_| unique_db_path()).collect::<Vec<_>>()))
            .flat_map(|caller| caller.join().unwrap())
            .collect();
        let distinct: std::collections::HashSet<_> = paths.iter().collect();
        assert_eq!(
            distinct.len(),
            paths.len(),
            "two tests got the same store path"
        );
    }
}
