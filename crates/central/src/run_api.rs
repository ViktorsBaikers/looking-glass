//! The public looking-glass run endpoint: a visitor runs a diagnostic on the
//! built-in local node and streams its output live over SSE. This module owns
//! the *transport policy* only — the same-origin guard, the per-client exec rate
//! limit, and the method/target validation that gates a run. The audited process
//! engine and the **one** global concurrency cap live in `shared::exec`; central
//! does not re-cap.
//!
//! The endpoint is public and unauthenticated (no session to ride), so the
//! guards are: an `Origin`/`Referer` same-site check (the session-less equivalent
//! of a CSRF token, `/rite-vet` decision), a per-client rate limit keyed on the
//! trusted-proxy client identity (AC39, reused from Slice 2), and the global cap.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use shared::exec::{ExecEngine, ExecHandle, ExecLimits, StartError};
use shared::liveness::is_online;
use shared::protocol::TunnelMessage;
use shared::template::{CommandTemplate, DaemonProbe, Method, PathDaemonProbe};
use shared::validate::{
    bgp_arg, validate_target, BgpArgError, DnsResolver, HostResolver, PrefixFamily, TargetError,
    ValidatedTarget,
};
use tokio::sync::{mpsc, Semaphore};

use crate::auth::{random_id, ClientContext};
use crate::observability::{correlation_id, log_validation_rejected};
use crate::store::{
    derive_location_status, unix_now, Agent, GlobalSettings, LocationStatus, NodeKind,
    RunnableMethod, Store, StoreError,
};
use crate::stream::{sse_refusal, sse_relay, sse_run};
use crate::{AppState, RelayEvent, SubmitError};

const RATE_LIMITER_PRUNE_THRESHOLD: usize = 4096;

/// The run subsystem carried in application state: the shared engine, admission
/// domain, DNS resolver, and per-client exec rate limiter. Cheap to clone (all
/// shared handles).
#[derive(Clone)]
pub struct RunService {
    runtime: RunRuntime,
    settings_update: Arc<Mutex<()>>,
    resolver: Arc<DnsResolver>,
    /// Detects the local built-in node's routing daemon for a BGP run (FR-036).
    /// Defaults to a `PATH` probe; a test injects a stub via [`Self::with_daemon_probe`].
    daemon_probe: Arc<dyn DaemonProbe>,
}

#[derive(Clone)]
struct RunRuntime {
    engine: ExecEngine,
    admission: Arc<RunAdmission>,
}

impl RunRuntime {
    fn from_settings(settings: &GlobalSettings) -> Self {
        Self::new(
            ExecLimits {
                max_concurrent: settings.exec_max_concurrent,
                timeout: Duration::from_secs(settings.exec_timeout_secs),
                max_output_bytes: settings.exec_max_output_kib * 1024,
                channel_capacity: 64,
            },
            RateLimit {
                max: settings.exec_rate_max,
                window: Duration::from_secs(settings.exec_rate_window_secs),
            },
        )
    }

    fn new(limits: ExecLimits, rate: RateLimit) -> Self {
        let max_concurrent = limits.max_concurrent;
        Self {
            engine: ExecEngine::new(ExecLimits {
                max_concurrent: Semaphore::MAX_PERMITS,
                ..limits
            }),
            admission: Arc::new(RunAdmission::new(max_concurrent, rate)),
        }
    }
}

impl RunService {
    /// Build from an explicit config — used by tests to pin a tiny cap or rate.
    pub fn new(limits: ExecLimits, rate: RateLimit, resolver: Arc<DnsResolver>) -> Self {
        Self {
            runtime: RunRuntime::new(limits, rate),
            settings_update: Arc::new(Mutex::new(())),
            resolver,
            daemon_probe: Arc::new(PathDaemonProbe),
        }
    }

    /// Override the routing-daemon probe — used by tests to simulate a local BGP
    /// daemon present or absent without a live BIRD/FRR install.
    pub fn with_daemon_probe(mut self, probe: Arc<dyn DaemonProbe>) -> Self {
        self.daemon_probe = probe;
        self
    }

    /// Build the run subsystem from the admin-editable global settings (AC25):
    /// the global concurrency cap, per-run timeout, output bound, and per-client
    /// exec rate limit all come from `GlobalSettings`, whose defaults read the
    /// `LG_EXEC_*` env vars as the fallback. A settings change takes effect on the
    /// run path the next time this is built or reconfigured.
    pub fn from_settings(settings: &GlobalSettings) -> Self {
        let resolver = Arc::new(
            DnsResolver::from_system().expect("build system DNS resolver for target validation"),
        );
        Self {
            runtime: RunRuntime::from_settings(settings),
            settings_update: Arc::new(Mutex::new(())),
            resolver,
            daemon_probe: Arc::new(PathDaemonProbe),
        }
    }

    /// Test constructor: an explicit global cap, timeout, and per-client rate,
    /// using the system resolver. Lets an integration test pin a saturated cap
    /// (`max_concurrent = 0` → every run refused "node busy") or a tiny rate.
    pub fn for_test(max_concurrent: usize, timeout: Duration, rate_max: u32) -> Self {
        let resolver = Arc::new(
            DnsResolver::from_system().expect("build system DNS resolver for target validation"),
        );
        Self::new(
            ExecLimits {
                max_concurrent,
                timeout,
                max_output_bytes: 64 * 1024,
                channel_capacity: 32,
            },
            RateLimit {
                max: rate_max,
                window: Duration::from_secs(60),
            },
            resolver,
        )
    }

    pub(crate) fn save_settings(
        &self,
        store: &Store,
        settings: &GlobalSettings,
    ) -> Result<(), StoreError> {
        self.save_settings_with(settings, || store.persist_settings(settings))
    }

    fn save_settings_with(
        &self,
        settings: &GlobalSettings,
        persist: impl FnOnce() -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let _update = self.lock_settings_update();
        persist()?;
        self.runtime.admission.update(settings);
        Ok(())
    }

    fn lock_settings_update(&self) -> std::sync::MutexGuard<'_, ()> {
        self.settings_update.lock().expect("settings update lock")
    }

    fn snapshot(&self) -> RunRuntime {
        self.runtime.clone()
    }

    #[cfg(test)]
    fn available_permits(&self) -> usize {
        self.runtime.admission.available_permits()
    }
}

/// Per-client exec rate limit (AC39 / FR-035): N run requests per window, keyed
/// on the trusted-proxy client identity so a spoofed `X-Forwarded-For` lands on
/// the same key. Applied at the boundary before any work, so spamming even
/// invalid requests is bounded. (A near-twin of the login limiter; kept separate
/// so exec and login tune independently — consolidating the two fixed-window
/// limiters is a follow-up outside this slice's files.)
#[derive(Clone, Copy, Debug)]
pub struct RateLimit {
    pub max: u32,
    pub window: Duration,
}

struct Window {
    count: u32,
    start: Instant,
}

struct AdmissionState {
    max_concurrent: usize,
    active: usize,
    rate: RateLimit,
    windows: HashMap<IpAddr, Window>,
}

struct RunAdmission {
    state: Mutex<AdmissionState>,
}

struct RunAdmissionPermit(Arc<RunAdmission>);

impl Drop for RunAdmissionPermit {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().expect("run admission mutex");
        state.active -= 1;
    }
}

impl RunAdmission {
    fn new(max_concurrent: usize, rate: RateLimit) -> Self {
        Self {
            state: Mutex::new(AdmissionState {
                max_concurrent,
                active: 0,
                rate,
                windows: HashMap::new(),
            }),
        }
    }

    fn allow(&self, client: IpAddr) -> bool {
        let now = Instant::now();
        let mut state = self.state.lock().expect("run admission mutex");
        if state.windows.len() > RATE_LIMITER_PRUNE_THRESHOLD {
            let window = state.rate.window;
            state
                .windows
                .retain(|_, entry| now.duration_since(entry.start) < window);
        }
        let rate = state.rate;
        let window = state.windows.entry(client).or_insert(Window {
            count: 0,
            start: now,
        });
        if now.duration_since(window.start) >= rate.window {
            window.count = 0;
            window.start = now;
        }
        window.count += 1;
        window.count <= rate.max
    }

    fn acquire(self: &Arc<Self>) -> Option<RunAdmissionPermit> {
        let mut state = self.state.lock().expect("run admission mutex");
        if state.active >= state.max_concurrent {
            return None;
        }
        state.active += 1;
        Some(RunAdmissionPermit(Arc::clone(self)))
    }

    fn update(&self, settings: &GlobalSettings) {
        let mut state = self.state.lock().expect("run admission mutex");
        state.max_concurrent = settings.exec_max_concurrent;
        state.rate = RateLimit {
            max: settings.exec_rate_max,
            window: Duration::from_secs(settings.exec_rate_window_secs),
        };
    }

    #[cfg(test)]
    fn available_permits(&self) -> usize {
        let state = self.state.lock().expect("run admission mutex");
        state.max_concurrent.saturating_sub(state.active)
    }
}

/// The diagnostic methods the built-in local node offers when a run does not name
/// a location — the direct built-in-node path. A run that *does* name a location is
/// gated instead on that location's [`Location::runnable_methods`], so the
/// admin-configured offered set (including any BGP offering) holds on the live path.
/// AC13 rejects any method outside the gating set. BGP is exposed only through an
/// explicit per-location offering, never this bare fallback.
const LOCAL_OFFERED: [Method; 6] = [
    Method::Ping,
    Method::Ping6,
    Method::Mtr,
    Method::Mtr6,
    Method::Traceroute,
    Method::Traceroute6,
];

#[derive(Deserialize)]
pub struct RunParams {
    method: String,
    target: String,
    /// The location the visitor selected. When present it gates the run on that
    /// location's runnable methods; absent (or empty) falls back to the built-in
    /// local node's [`LOCAL_OFFERED`] set.
    #[serde(default)]
    location: Option<String>,
}

/// Why a run was refused before (or instead of) streaming output. Each carries a
/// clear, non-technical message (AC41) with no stack trace or internal detail.
#[derive(Debug)]
enum RunRefusal {
    LocationUnavailable,
    MethodNotOffered,
    InvalidTarget(String),
    RateLimited,
    NodeBusy,
    RemoteUnavailable,
    BgpUnavailable,
}

impl RunRefusal {
    fn code(&self) -> &'static str {
        match self {
            RunRefusal::LocationUnavailable => "location_unavailable",
            RunRefusal::MethodNotOffered => "method_not_offered",
            RunRefusal::InvalidTarget(_) => "invalid_target",
            RunRefusal::RateLimited => "rate_limited",
            RunRefusal::NodeBusy => "node_busy",
            RunRefusal::RemoteUnavailable => "remote_unavailable",
            RunRefusal::BgpUnavailable => "bgp_unavailable",
        }
    }

    fn is_validation_rejection(&self) -> bool {
        matches!(
            self,
            RunRefusal::MethodNotOffered
                | RunRefusal::InvalidTarget(_)
                | RunRefusal::BgpUnavailable
        )
    }

    fn message(self) -> String {
        match self {
            RunRefusal::LocationUnavailable => {
                "that location is not available for diagnostics right now".to_string()
            }
            RunRefusal::MethodNotOffered => {
                "that method is not available on this location".to_string()
            }
            RunRefusal::InvalidTarget(message) => message,
            RunRefusal::RateLimited => {
                "too many requests — please wait a moment and try again".to_string()
            }
            RunRefusal::NodeBusy => {
                "the node is busy right now — please try again in a moment".to_string()
            }
            RunRefusal::RemoteUnavailable => {
                "the remote node is not connected right now".to_string()
            }
            RunRefusal::BgpUnavailable => {
                "BGP is not available on this node — no supported routing daemon is present"
                    .to_string()
            }
        }
    }
}

/// A run request resolved against the location's offered set: either a target-based
/// diagnostic or a prefix-based BGP query (each takes a different argument grammar
/// and execution path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedRun {
    Diagnostic(Method),
    Bgp(PrefixFamily),
}

enum RunDestination {
    Local {
        offered: Vec<RunnableMethod>,
    },
    Remote {
        agent_id: String,
        offered: Vec<RunnableMethod>,
    },
}

impl RunDestination {
    fn offered(&self) -> &[RunnableMethod] {
        match self {
            RunDestination::Local { offered } | RunDestination::Remote { offered, .. } => offered,
        }
    }
}

enum RunOutput {
    Local(ExecHandle, RunAdmissionPermit),
    Remote(mpsc::Receiver<RelayEvent>),
}

pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/api/run/stream", get(run_stream))
        .route("/api/visitor", get(visitor))
        .with_state(state)
}

/// The visitor's detected IP for the network block (FR-042/AC19). The address is
/// the trusted-proxy-derived [`ClientContext`] identity — the same spoof-resistant
/// source the rate limiter keys on — never a blindly-trusted forwarded header, so
/// what the page shows a visitor is what the node actually sees.
#[derive(Serialize)]
struct VisitorInfo {
    ip: Option<String>,
}

async fn visitor(ctx: ClientContext) -> Json<VisitorInfo> {
    Json(VisitorInfo {
        ip: ctx.ip.map(|ip| ip.to_string()),
    })
}

async fn run_stream(
    State(state): State<AppState>,
    ctx: ClientContext,
    headers: HeaderMap,
    Query(params): Query<RunParams>,
) -> Response {
    let correlation_id = correlation_id(&headers);
    // Cross-site guard: a request whose Origin/Referer is a different site is
    // refused outright (the session-less CSRF equivalent). Legitimate visitors
    // are same-origin, so they never see this.
    if !same_origin(&headers) {
        log_command_run(&correlation_id, &params, "rejected", "cross_origin");
        log_validation_rejected(&correlation_id, "command.run", "cross_origin");
        return (
            StatusCode::FORBIDDEN,
            "cross-origin run requests are refused",
        )
            .into_response();
    }

    let destination = match run_destination(&state, params.location.as_deref()) {
        Ok(destination) => destination,
        Err(refusal) => {
            log_command_refusal(&correlation_id, &params, &refusal);
            return sse_refusal(refusal.message()).into_response();
        }
    };
    let runtime = state.run.snapshot();

    match evaluate(&state, &runtime, &ctx, &params, destination).await {
        Ok(RunOutput::Local(handle, admission)) => {
            log_command_run(&correlation_id, &params, "started", "local");
            sse_run(handle, admission).into_response()
        }
        Ok(RunOutput::Remote(events)) => {
            log_command_run(&correlation_id, &params, "started", "remote");
            sse_relay(events).into_response()
        }
        // Everything past the Origin gate is delivered in-band as SSE so an
        // EventSource client sees the reason (a non-200 body is invisible to it).
        Err(refusal) => {
            log_command_refusal(&correlation_id, &params, &refusal);
            sse_refusal(refusal.message()).into_response()
        }
    }
}

fn log_command_refusal(correlation_id: &str, params: &RunParams, refusal: &RunRefusal) {
    log_command_run(correlation_id, params, "rejected", refusal.code());
    if refusal.is_validation_rejection() {
        log_validation_rejected(correlation_id, "command.run", refusal.code());
    }
}

fn log_command_run(correlation_id: &str, params: &RunParams, outcome: &str, reason: &str) {
    tracing::info!(
        event = "command.run",
        correlation_id,
        method = %params.method,
        location = %params.location.as_deref().filter(|id| !id.is_empty()).unwrap_or("local"),
        outcome,
        reason,
        "command run event"
    );
}

/// The runnable-method set that gates this run. A named location is gated on its
/// stored [`Location::runnable_methods`] (its offered set, diagnostics and BGP
/// alike) and must be online (FR-026); an unnamed or empty location falls back to
/// the built-in local node's diagnostic [`LOCAL_OFFERED`].
fn run_destination(state: &AppState, location: Option<&str>) -> Result<RunDestination, RunRefusal> {
    let location = location.filter(|id| !id.is_empty());
    let Some(id) = location else {
        return Ok(RunDestination::Local {
            offered: LOCAL_OFFERED
                .iter()
                .map(|method| RunnableMethod::Diagnostic(*method))
                .collect(),
        });
    };
    let location = state
        .store
        .get_location(id)
        .map_err(|_| RunRefusal::LocationUnavailable)?
        .ok_or(RunRefusal::LocationUnavailable)?;
    let agents = state
        .store
        .list_agents(id)
        .map_err(|_| RunRefusal::LocationUnavailable)?;
    let status = derive_location_status(&location, &agents, unix_now());
    if status != LocationStatus::Online {
        return Err(RunRefusal::LocationUnavailable);
    }
    let offered = location.runnable_methods();
    match location.kind {
        NodeKind::Local => Ok(RunDestination::Local { offered }),
        NodeKind::Remote => {
            let agent =
                online_agent(&state.tunnel_hub, &agents).ok_or(RunRefusal::LocationUnavailable)?;
            Ok(RunDestination::Remote {
                agent_id: agent.id.clone(),
                offered,
            })
        }
    }
}

fn online_agent<'a>(hub: &crate::TunnelHub, agents: &'a [Agent]) -> Option<&'a Agent> {
    let now = unix_now();
    agents
        .iter()
        .find(|agent| is_online(agent.last_seen, now, agent.revoked) && hub.is_connected(&agent.id))
}

async fn evaluate(
    state: &AppState,
    runtime: &RunRuntime,
    ctx: &ClientContext,
    params: &RunParams,
    destination: RunDestination,
) -> Result<RunOutput, RunRefusal> {
    // Rate limit first, at the boundary, keyed on the trusted-proxy identity.
    if let Some(client) = ctx.ip {
        if !runtime.admission.allow(client) {
            return Err(RunRefusal::RateLimited);
        }
    }

    match resolve_method(&params.method, destination.offered())? {
        ResolvedRun::Diagnostic(method) => {
            let target = diagnostic_target(&params.target, state.run.resolver.as_ref()).await?;
            match destination {
                RunDestination::Local { .. } => {
                    start_local(runtime, method.command(&target), Some(target.ip()))
                }
                RunDestination::Remote { agent_id, .. } => {
                    submit_remote(state, &agent_id, method_wire(method), &target.arg()).await
                }
            }
        }
        ResolvedRun::Bgp(family) => {
            // BGP validates a prefix (NOT a target): family-locked IP/CIDR grammar,
            // deliberately without the SSRF public-range filter, rejected here
            // before any daemon command runs (AC36/FR-072).
            let prefix =
                bgp_arg(&params.target, family).map_err(|error| bgp_target_refusal(&error))?;
            match destination {
                RunDestination::Local { .. } => {
                    // The built-in local node probes for its routing daemon at run
                    // time; absent → a clear "not available" refusal (AC41), no hang.
                    let daemon = state
                        .run
                        .daemon_probe
                        .detect()
                        .ok_or(RunRefusal::BgpUnavailable)?;
                    // BGP inspects the local RIB and never connects, so no pinned IP.
                    start_local(runtime, daemon.command(&prefix), None)
                }
                RunDestination::Remote { agent_id, .. } => {
                    // The agent re-validates the prefix with the same grammar and
                    // probes for its own daemon (mirrors the SSRF re-validation).
                    submit_remote(state, &agent_id, family.wire(), prefix.arg()).await
                }
            }
        }
    }
}

/// Start a run on the built-in local node through the one audited exec engine,
/// mapping its refusals to clear messages (AC40/AC41). `pinned_ip` is the validated
/// address for a diagnostic, or `None` for BGP (which never opens a connection).
fn start_local(
    runtime: &RunRuntime,
    command: CommandTemplate,
    pinned_ip: Option<IpAddr>,
) -> Result<RunOutput, RunRefusal> {
    let admission = runtime.admission.acquire().ok_or(RunRefusal::NodeBusy)?;
    match runtime.engine.try_start(command, pinned_ip) {
        Ok(handle) => Ok(RunOutput::Local(handle, admission)),
        Err(StartError::Busy) => Err(RunRefusal::NodeBusy),
        Err(StartError::Rejected(_)) => Err(RunRefusal::InvalidTarget(
            "that target is not a public address we can run diagnostics against".to_string(),
        )),
    }
}

/// Relay a run to a connected agent over the tunnel; the wire message is unchanged
/// (`Command { method, target }`) — BGP rides it with the method name `bgp`/`bgp6`
/// and the canonical prefix in the target field, and the agent re-validates it.
async fn submit_remote(
    state: &AppState,
    agent_id: &str,
    method: &str,
    target: &str,
) -> Result<RunOutput, RunRefusal> {
    let events = state
        .tunnel_hub
        .submit(
            agent_id,
            TunnelMessage::Command {
                run_id: random_id(),
                method: method.to_string(),
                target: target.to_string(),
            },
        )
        .await
        .map_err(|error| match error {
            SubmitError::Busy => RunRefusal::NodeBusy,
            SubmitError::NotConnected => RunRefusal::RemoteUnavailable,
        })?;
    Ok(RunOutput::Remote(events))
}

/// Validate a diagnostic target to a pinned public IP, mapping a rejection to a
/// clear message. Generic over the resolver so it is unit-tested with a stub.
async fn diagnostic_target<R: HostResolver>(
    target: &str,
    resolver: &R,
) -> Result<ValidatedTarget, RunRefusal> {
    validate_target(target, resolver)
        .await
        .map_err(|error| RunRefusal::InvalidTarget(target_message(&error)))
}

fn bgp_target_refusal(error: &BgpArgError) -> RunRefusal {
    RunRefusal::InvalidTarget(match error {
        BgpArgError::Malformed => {
            "that BGP prefix is not a valid IP address or CIDR range".to_string()
        }
        BgpArgError::WrongFamily => {
            "that BGP prefix is the wrong address family for this method".to_string()
        }
    })
}

/// Resolve the requested method name against the location's offered set into either
/// a diagnostic method or a BGP family (AC13 — a method the location does not offer
/// is refused, and no run is prepared).
fn resolve_method(name: &str, offered: &[RunnableMethod]) -> Result<ResolvedRun, RunRefusal> {
    if let Some(method) = method_from_wire(name) {
        if offered.contains(&RunnableMethod::Diagnostic(method)) {
            return Ok(ResolvedRun::Diagnostic(method));
        }
    }
    if let Some(family) = PrefixFamily::from_wire(name) {
        if offered.contains(&RunnableMethod::Bgp(family)) {
            return Ok(ResolvedRun::Bgp(family));
        }
    }
    Err(RunRefusal::MethodNotOffered)
}

fn method_from_wire(name: &str) -> Option<Method> {
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

fn method_wire(method: Method) -> &'static str {
    match method {
        Method::Ping => "ping",
        Method::Ping6 => "ping6",
        Method::Mtr => "mtr",
        Method::Mtr6 => "mtr6",
        Method::Traceroute => "traceroute",
        Method::Traceroute6 => "traceroute6",
    }
}

fn target_message(error: &TargetError) -> String {
    match error {
        TargetError::Malformed => "that target is not a valid IP address or hostname".to_string(),
        TargetError::Rejected(_) => {
            "that target is not a public address we can run diagnostics against".to_string()
        }
        TargetError::Unresolvable => "that hostname could not be resolved".to_string(),
    }
}

/// A same-origin check on `Origin` (falling back to `Referer`). Public runs are
/// unauthenticated but still trigger node work, so absence is refused: a browser
/// request must prove same-origin by sending one of these headers.
fn same_origin(headers: &HeaderMap) -> bool {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(normalize_authority);
    let claimed = headers
        .get(header::ORIGIN)
        .or_else(|| headers.get(header::REFERER))
        .and_then(|v| v.to_str().ok());
    let expected_scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("http");

    match (host, claimed) {
        (Some(host), Some(claimed)) => match origin_tuple(claimed) {
            Some((scheme, authority)) => {
                scheme.eq_ignore_ascii_case(expected_scheme) && authority == host
            }
            None => false,
        },
        (_, None) => false,
        (None, Some(_)) => false,
    }
}

fn origin_tuple(url: &str) -> Option<(&str, String)> {
    let (scheme, after_scheme) = url.split_once("://")?;
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    if authority.is_empty() {
        return None;
    }
    Some((scheme, normalize_authority(authority)))
}

fn normalize_authority(authority: &str) -> String {
    authority.trim_end_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::http::HeaderValue;
    use shared::protocol::TunnelMessage;

    use crate::auth::ClientContext;
    use crate::enroll::EnrollConfig;
    use crate::store::{unix_now, Agent, Location, NodeKind, OfferedMethod, Store};
    use crate::{LoginLimiter, TransportConfig, TunnelHub};

    struct StubResolver {
        addrs: Vec<IpAddr>,
    }
    impl HostResolver for StubResolver {
        async fn resolve(
            &self,
            _host: &str,
        ) -> Result<Vec<IpAddr>, shared::validate::ResolveError> {
            Ok(self.addrs.clone())
        }
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    fn temp_path(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "lg-run-api-{tag}-{}-{n}-{nanos}",
            std::process::id()
        ))
    }

    fn test_state() -> AppState {
        let files = temp_path("files");
        std::fs::create_dir_all(&files).expect("create files root");
        AppState {
            store: Store::open(temp_path("db")).expect("open test store"),
            transport: TransportConfig::new([]),
            login_limiter: Arc::new(LoginLimiter::default()),
            setup_token: None,
            run: RunService::for_test(8, Duration::from_secs(30), 100),
            files_root: Arc::from(files.as_path()),
            enroll: EnrollConfig::for_test("https://central.test:8443", b"identity".to_vec()),
            tunnel_hub: TunnelHub::new(),
        }
    }

    fn online_remote(state: &AppState) {
        state
            .store
            .put_location(&Location {
                id: "remote-1".to_string(),
                name: "Remote".to_string(),
                geo_label: "DE".to_string(),
                map_query: None,
                facility: None,
                facility_url: None,
                kind: NodeKind::Remote,
                data_plane_origin: None,
                offered_methods: vec![OfferedMethod::Ping, OfferedMethod::Traceroute],
                status: LocationStatus::Offline,
                created_at: 0,
            })
            .unwrap();
        state
            .store
            .put_agent(&Agent {
                id: "agent-1".to_string(),
                location_id: "remote-1".to_string(),
                credential_hash: "$argon2id$stub".to_string(),
                enrolled_at: 0,
                last_seen: Some(unix_now()),
                revoked: false,
            })
            .unwrap();
    }

    async fn response_body(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    // Slice 10 / AC7: a visitor run against an online remote location is relayed
    // to the connected agent and streamed back as the same SSE console events the
    // local path uses.
    #[tokio::test]
    async fn remote_run_relays_over_the_tunnel_and_streams_sse() {
        let state = test_state();
        online_remote(&state);
        let mut jobs = state.tunnel_hub.register_for_test("agent-1");
        let driver = tokio::spawn(async move {
            let job = jobs.recv().await.expect("remote job submitted");
            match job.command {
                TunnelMessage::Command { method, target, .. } => {
                    assert_eq!(method, "ping");
                    assert_eq!(target, "8.8.8.8");
                }
                other => panic!("expected command, got {other:?}"),
            }
            job.events
                .send(crate::RelayEvent::Line("remote output".to_string()))
                .await
                .unwrap();
            job.events
                .send(crate::RelayEvent::Terminal { error: None })
                .await
                .unwrap();
        });

        let response = run_stream(
            State(state),
            ClientContext {
                ip: Some("198.51.100.9".parse().unwrap()),
                secure: false,
            },
            headers(&[
                ("host", "lg.test"),
                ("x-forwarded-proto", "https"),
                ("origin", "https://lg.test"),
            ]),
            Query(RunParams {
                method: "ping".to_string(),
                target: "8.8.8.8".to_string(),
                location: Some("remote-1".to_string()),
            }),
        )
        .await;

        let body = response_body(response).await;
        driver.await.unwrap();
        assert!(body.contains("event: line"), "{body}");
        assert!(body.contains("remote output"), "{body}");
        assert!(body.contains("event: done"), "{body}");
    }

    // Slice 10 / AC41: a remote terminal failure is delivered as a clear SSE
    // run-error followed by done, so the visitor console closes instead of hanging.
    #[tokio::test]
    async fn remote_terminal_error_streams_run_error_and_done() {
        let state = test_state();
        online_remote(&state);
        let mut jobs = state.tunnel_hub.register_for_test("agent-1");
        let driver = tokio::spawn(async move {
            let job = jobs.recv().await.expect("remote job submitted");
            job.events
                .send(crate::RelayEvent::Terminal {
                    error: Some("the remote node dropped the connection".to_string()),
                })
                .await
                .unwrap();
        });

        let response = run_stream(
            State(state),
            ClientContext {
                ip: Some("198.51.100.9".parse().unwrap()),
                secure: false,
            },
            headers(&[
                ("host", "lg.test"),
                ("x-forwarded-proto", "https"),
                ("origin", "https://lg.test"),
            ]),
            Query(RunParams {
                method: "traceroute".to_string(),
                target: "8.8.8.8".to_string(),
                location: Some("remote-1".to_string()),
            }),
        )
        .await;

        let body = response_body(response).await;
        driver.await.unwrap();
        assert!(
            body.contains("event: run-error")
                && body.contains("remote node dropped")
                && body.contains("event: done"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn remote_second_run_gets_node_busy_instead_of_queueing() {
        let state = test_state();
        online_remote(&state);
        let _jobs = state.tunnel_hub.register_for_test("agent-1");
        let first = run_stream(
            State(state.clone()),
            ClientContext {
                ip: Some("198.51.100.9".parse().unwrap()),
                secure: false,
            },
            headers(&[
                ("host", "lg.test"),
                ("x-forwarded-proto", "https"),
                ("origin", "https://lg.test"),
            ]),
            Query(RunParams {
                method: "ping".to_string(),
                target: "8.8.8.8".to_string(),
                location: Some("remote-1".to_string()),
            }),
        )
        .await;

        let second = run_stream(
            State(state),
            ClientContext {
                ip: Some("198.51.100.10".parse().unwrap()),
                secure: false,
            },
            headers(&[
                ("host", "lg.test"),
                ("x-forwarded-proto", "https"),
                ("origin", "https://lg.test"),
            ]),
            Query(RunParams {
                method: "ping".to_string(),
                target: "8.8.4.4".to_string(),
                location: Some("remote-1".to_string()),
            }),
        )
        .await;

        let body = tokio::time::timeout(Duration::from_millis(200), response_body(second))
            .await
            .expect("busy refusal must be immediate, not queued behind the first run");
        assert!(body.contains("busy"), "{body}");
        drop(first);
    }

    #[tokio::test]
    async fn remote_run_chooses_a_fresh_connected_agent() {
        let state = test_state();
        state
            .store
            .put_location(&Location {
                id: "remote-1".to_string(),
                name: "Remote".to_string(),
                geo_label: "DE".to_string(),
                map_query: None,
                facility: None,
                facility_url: None,
                kind: NodeKind::Remote,
                data_plane_origin: None,
                offered_methods: vec![OfferedMethod::Ping],
                status: LocationStatus::Offline,
                created_at: 0,
            })
            .unwrap();
        for id in ["agent-a", "agent-b"] {
            state
                .store
                .put_agent(&Agent {
                    id: id.to_string(),
                    location_id: "remote-1".to_string(),
                    credential_hash: "$argon2id$stub".to_string(),
                    enrolled_at: 0,
                    last_seen: Some(unix_now()),
                    revoked: false,
                })
                .unwrap();
        }
        let mut connected_jobs = state.tunnel_hub.register_for_test("agent-b");
        let driver = tokio::spawn(async move {
            let job = connected_jobs
                .recv()
                .await
                .expect("run must route to the connected agent");
            match job.command {
                TunnelMessage::Command { target, .. } => assert_eq!(target, "8.8.8.8"),
                other => panic!("expected command, got {other:?}"),
            }
            job.events
                .send(crate::RelayEvent::Terminal { error: None })
                .await
                .unwrap();
        });

        let response = run_stream(
            State(state),
            ClientContext {
                ip: Some("198.51.100.9".parse().unwrap()),
                secure: false,
            },
            headers(&[
                ("host", "lg.test"),
                ("x-forwarded-proto", "https"),
                ("origin", "https://lg.test"),
            ]),
            Query(RunParams {
                method: "ping".to_string(),
                target: "8.8.8.8".to_string(),
                location: Some("remote-1".to_string()),
            }),
        )
        .await;

        let body = response_body(response).await;
        driver.await.unwrap();
        assert!(body.contains("event: done"), "{body}");
        assert!(
            !body.contains("not connected"),
            "a disconnected fresh agent must not make a connected peer fail: {body}"
        );
    }

    fn diagnostics(methods: &[Method]) -> Vec<RunnableMethod> {
        methods
            .iter()
            .map(|m| RunnableMethod::Diagnostic(*m))
            .collect()
    }

    // AC13 — a method the location does not offer is rejected; a run request for
    // it never reaches execution. BGP resolves only when its family is offered.
    #[test]
    fn resolve_method_gates_diagnostics_and_bgp_on_the_offered_set() {
        let offered = diagnostics(&[Method::Ping, Method::Mtr]);
        assert!(matches!(
            resolve_method("mtr", &offered),
            Ok(ResolvedRun::Diagnostic(Method::Mtr))
        ));
        assert!(matches!(
            resolve_method("traceroute", &offered),
            Err(RunRefusal::MethodNotOffered)
        ));
        assert!(matches!(
            resolve_method("telnet", &offered),
            Err(RunRefusal::MethodNotOffered)
        ));
        // BGP is refused on a diagnostics-only location.
        assert!(matches!(
            resolve_method("bgp", &offered),
            Err(RunRefusal::MethodNotOffered)
        ));

        let with_bgp = vec![RunnableMethod::Bgp(PrefixFamily::V4)];
        assert!(matches!(
            resolve_method("bgp", &with_bgp),
            Ok(ResolvedRun::Bgp(PrefixFamily::V4))
        ));
        // bgp6 is a distinct offering — offering bgp does not offer bgp6.
        assert!(matches!(
            resolve_method("bgp6", &with_bgp),
            Err(RunRefusal::MethodNotOffered)
        ));
    }

    #[tokio::test]
    async fn diagnostic_target_accepts_a_public_ip_literal() {
        let resolver = StubResolver { addrs: vec![] };
        let target = diagnostic_target("8.8.8.8", &resolver)
            .await
            .expect("public ip literal is a valid target");
        assert_eq!(target.arg(), "8.8.8.8");
    }

    // AC41 — a rejected (non-public) target surfaces a clear, non-technical error
    // and no run is prepared.
    #[tokio::test]
    async fn diagnostic_target_rejects_a_private_target_with_a_clear_message() {
        let resolver = StubResolver { addrs: vec![] };
        match diagnostic_target("10.0.0.1", &resolver).await {
            Err(RunRefusal::InvalidTarget(message)) => {
                assert!(message.contains("public"));
                assert!(!message.contains("Private") && !message.contains("Reject"));
            }
            _ => panic!("a private target must be refused with a clear message"),
        }
    }

    // AC41 — an unresolvable hostname surfaces a clear error, not a hang or trace.
    #[tokio::test]
    async fn diagnostic_target_reports_an_unresolvable_hostname() {
        let resolver = StubResolver { addrs: vec![] };
        assert!(matches!(
            diagnostic_target("nope.invalid", &resolver).await,
            Err(RunRefusal::InvalidTarget(message)) if message.contains("resolved")
        ));
    }

    #[test]
    fn same_origin_allows_a_matching_host() {
        assert!(same_origin(&headers(&[
            ("host", "lg.example.com"),
            ("x-forwarded-proto", "https"),
            ("origin", "https://lg.example.com"),
        ])));
    }

    #[test]
    fn same_origin_refuses_a_missing_origin() {
        assert!(!same_origin(&headers(&[("host", "lg.example.com")])));
    }

    #[test]
    fn same_origin_refuses_a_scheme_or_port_mismatch() {
        assert!(!same_origin(&headers(&[
            ("host", "lg.example.com:443"),
            ("x-forwarded-proto", "https"),
            ("origin", "http://lg.example.com:443"),
        ])));
        assert!(!same_origin(&headers(&[
            ("host", "lg.example.com:8443"),
            ("x-forwarded-proto", "https"),
            ("origin", "https://lg.example.com:443"),
        ])));
    }

    #[test]
    fn same_origin_refuses_a_cross_site_origin() {
        assert!(!same_origin(&headers(&[
            ("host", "lg.example.com"),
            ("x-forwarded-proto", "https"),
            ("origin", "https://evil.test"),
        ])));
    }

    #[test]
    fn same_origin_uses_referer_when_no_origin() {
        assert!(!same_origin(&headers(&[
            ("host", "lg.example.com"),
            ("referer", "https://evil.test/attack"),
        ])));
    }

    // ----- Slice 11: BGP on local + remote ------------------------------------

    struct StubProbe(Option<shared::template::BgpDaemon>);
    impl DaemonProbe for StubProbe {
        fn detect(&self) -> Option<shared::template::BgpDaemon> {
            self.0
        }
    }

    fn bgp_local_state(daemon: Option<shared::template::BgpDaemon>) -> AppState {
        let mut state = test_state();
        state.run = state
            .run
            .clone()
            .with_daemon_probe(Arc::new(StubProbe(daemon)));
        state
            .store
            .put_location(&Location {
                id: "local-1".to_string(),
                name: "Local".to_string(),
                geo_label: "DE".to_string(),
                map_query: None,
                facility: None,
                facility_url: None,
                kind: NodeKind::Local,
                data_plane_origin: None,
                offered_methods: vec![OfferedMethod::Ping, OfferedMethod::Bgp, OfferedMethod::Bgp6],
                status: LocationStatus::Online,
                created_at: 0,
            })
            .unwrap();
        state
    }

    async fn run_bgp_local(state: AppState, method: &str, target: &str) -> String {
        let response = run_stream(
            State(state),
            ClientContext {
                ip: Some("198.51.100.9".parse().unwrap()),
                secure: false,
            },
            headers(&[
                ("host", "lg.test"),
                ("x-forwarded-proto", "https"),
                ("origin", "https://lg.test"),
            ]),
            Query(RunParams {
                method: method.to_string(),
                target: target.to_string(),
                location: Some("local-1".to_string()),
            }),
        )
        .await;
        response_body(response).await
    }

    // AC41 / daemon-gate: BGP on a node with no supported routing daemon is
    // unavailable, surfaced as a clear error, not a hang or a spawned process.
    #[tokio::test]
    async fn bgp_local_without_a_daemon_is_unavailable() {
        let body = run_bgp_local(bgp_local_state(None), "bgp", "8.8.8.8").await;
        assert!(
            body.contains("routing daemon"),
            "a node with no daemon must refuse BGP with a clear message: {body}"
        );
        assert!(
            !body.contains("diagnostic tool"),
            "no process should have been spawned: {body}"
        );
    }

    // AC15/AC37 local: with a daemon detected, a valid prefix is dispatched to the
    // read-only exec path (it reaches execution and terminates — no daemon-absent
    // refusal, no hang). The fixed read-only template is asserted in shared::template.
    #[tokio::test]
    async fn bgp_local_with_a_daemon_dispatches_the_read_only_query() {
        let body = run_bgp_local(
            bgp_local_state(Some(shared::template::BgpDaemon::Bird)),
            "bgp",
            "8.8.8.8",
        )
        .await;
        assert!(
            !body.contains("routing daemon"),
            "a detected daemon must not report BGP unavailable: {body}"
        );
        assert!(
            body.contains("event: done"),
            "the run must reach the exec engine and terminate, not hang: {body}"
        );
    }

    // AC36/T4 at the run boundary: an injected BGP prefix is rejected BEFORE any
    // daemon command runs, even though a daemon is present — no process spawned.
    #[tokio::test]
    async fn bgp_local_rejects_an_injected_prefix_before_the_daemon() {
        let body = run_bgp_local(
            bgp_local_state(Some(shared::template::BgpDaemon::Bird)),
            "bgp",
            "8.8.8.8; configure terminal",
        )
        .await;
        assert!(
            body.contains("not a valid IP address or CIDR"),
            "an injected prefix must surface a clear rejection: {body}"
        );
        assert!(
            !body.contains("diagnostic tool") && !body.contains("event: line"),
            "an injected prefix must be rejected before any daemon command runs: {body}"
        );
    }

    // AC36 family lock at the run boundary: a v6 literal for the v4 `bgp` method is
    // refused with a clear message, before any daemon command.
    #[tokio::test]
    async fn bgp_local_rejects_a_wrong_family_prefix() {
        let body = run_bgp_local(
            bgp_local_state(Some(shared::template::BgpDaemon::Frr)),
            "bgp",
            "2001:db8::/32",
        )
        .await;
        assert!(
            body.contains("wrong address family"),
            "a v6 prefix on the v4 BGP method must be refused: {body}"
        );
    }

    // AC32 remote: a BGP run against an online remote is relayed to the agent as the
    // unchanged `Command` wire message — method `bgp`, the canonical prefix in the
    // target field — and the agent's route output streams back over SSE. A private
    // CIDR is relayed verbatim (BGP applies no SSRF public-range filter).
    #[tokio::test]
    async fn bgp_on_remote_relays_the_prefix_and_streams_route_output() {
        let state = test_state();
        state
            .store
            .put_location(&Location {
                id: "remote-1".to_string(),
                name: "Remote".to_string(),
                geo_label: "DE".to_string(),
                map_query: None,
                facility: None,
                facility_url: None,
                kind: NodeKind::Remote,
                data_plane_origin: None,
                offered_methods: vec![OfferedMethod::Bgp],
                status: LocationStatus::Offline,
                created_at: 0,
            })
            .unwrap();
        state
            .store
            .put_agent(&Agent {
                id: "agent-1".to_string(),
                location_id: "remote-1".to_string(),
                credential_hash: "$argon2id$stub".to_string(),
                enrolled_at: 0,
                last_seen: Some(unix_now()),
                revoked: false,
            })
            .unwrap();
        let mut jobs = state.tunnel_hub.register_for_test("agent-1");
        let driver = tokio::spawn(async move {
            let job = jobs.recv().await.expect("remote BGP job submitted");
            match job.command {
                TunnelMessage::Command { method, target, .. } => {
                    assert_eq!(method, "bgp", "BGP relays with the bgp wire method");
                    assert_eq!(
                        target, "10.0.0.0/8",
                        "the canonical prefix rides the target field, private CIDR and all"
                    );
                }
                other => panic!("expected a BGP command, got {other:?}"),
            }
            job.events
                .send(crate::RelayEvent::Line("10.0.0.0/8 via BIRD".to_string()))
                .await
                .unwrap();
            job.events
                .send(crate::RelayEvent::Terminal { error: None })
                .await
                .unwrap();
        });

        let response = run_stream(
            State(state),
            ClientContext {
                ip: Some("198.51.100.9".parse().unwrap()),
                secure: false,
            },
            headers(&[
                ("host", "lg.test"),
                ("x-forwarded-proto", "https"),
                ("origin", "https://lg.test"),
            ]),
            Query(RunParams {
                method: "bgp".to_string(),
                target: "10.0.0.0/8".to_string(),
                location: Some("remote-1".to_string()),
            }),
        )
        .await;

        let body = response_body(response).await;
        driver.await.unwrap();
        assert!(
            body.contains("event: line") && body.contains("10.0.0.0/8 via BIRD"),
            "{body}"
        );
        assert!(body.contains("event: done"), "{body}");
    }

    // AC25: an exec-limit setting takes effect on the run path — the concurrency
    // cap the run engine enforces is the one the admin set in GlobalSettings.
    #[test]
    fn from_settings_applies_the_concurrency_cap() {
        let settings = GlobalSettings {
            exec_max_concurrent: 3,
            ..GlobalSettings::default()
        };
        let run = RunService::from_settings(&settings);
        assert_eq!(
            run.available_permits(),
            3,
            "the concurrency-cap setting is the cap the run path enforces"
        );
    }

    #[test]
    fn run_admission_blocks_past_the_rate_max() {
        let admission = RunAdmission::new(
            1,
            RateLimit {
                max: 3,
                window: Duration::from_secs(60),
            },
        );
        let client: IpAddr = "203.0.113.9".parse().unwrap();
        for _ in 0..3 {
            assert!(admission.allow(client));
        }
        assert!(!admission.allow(client), "the fourth request is blocked");
    }

    #[test]
    fn settings_update_keeps_existing_rate_windows() {
        let state = test_state();
        let run = RunService::for_test(1, Duration::from_secs(30), 1);
        let client: IpAddr = "203.0.113.9".parse().unwrap();
        assert!(run.snapshot().admission.allow(client));

        let settings = GlobalSettings {
            exec_max_concurrent: 1,
            exec_rate_max: 2,
            exec_rate_window_secs: 60,
            ..GlobalSettings::default()
        };
        run.save_settings(&state.store, &settings).unwrap();

        assert!(run.snapshot().admission.allow(client));
        assert!(
            !run.snapshot().admission.allow(client),
            "the pre-update request remains in the same rate window"
        );
    }

    #[test]
    fn held_run_stays_in_the_shared_admission_domain_after_settings_update() {
        let run = RunService::for_test(1, Duration::from_secs(30), 10);
        let held = run
            .snapshot()
            .admission
            .acquire()
            .expect("first run is admitted");
        let settings = GlobalSettings {
            exec_max_concurrent: 2,
            ..GlobalSettings::default()
        };

        run.save_settings_with(&settings, || Ok(())).unwrap();
        assert_eq!(run.available_permits(), 1);

        let next = run
            .snapshot()
            .admission
            .acquire()
            .expect("one additional run is admitted under the new limit");
        assert!(run.snapshot().admission.acquire().is_none());
        drop(held);
        assert!(run.snapshot().admission.acquire().is_some());
        drop(next);
    }

    // AC25 (q-2026-07-10-008): concurrent settings saves are mutually exclusive
    // through the production `settings_update` lock, publication happens inside
    // that critical section, and publication order is last-write-wins. Every
    // proof is positive — a shared in-critical witness any overlap trips, and a
    // publication-window witness set only by a received entry signal — never an
    // assertion that a worker is blocked on the mutex (unobservable on
    // `std::sync::Mutex`). Every wait is timeout-bounded and worker 1's barrier
    // release is unconditional, so no build of this test can hang: a lock-bypass
    // mutant fails through the overlap witness, and a publish-outside-the-lock
    // mutant fails through the publication-window witness, both within the
    // grace window.
    #[test]
    fn concurrent_settings_updates_serialize_on_the_production_lock() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::mpsc;
        use std::thread;

        let grace = Duration::from_secs(1);
        let run = Arc::new(RunService::for_test(1, Duration::from_secs(30), 10));

        let in_critical = Arc::new(AtomicBool::new(false));
        let overlap = Arc::new(AtomicBool::new(false));
        let clock = Arc::new(AtomicUsize::new(0));
        let first_exit_tick = Arc::new(AtomicUsize::new(usize::MAX));
        let second_entry_tick = Arc::new(AtomicUsize::new(usize::MAX));
        let permits_at_second_entry = Arc::new(AtomicUsize::new(usize::MAX));

        // Worker 1 parks inside its persist closure while holding the
        // production settings lock.
        let (first_parked_tx, first_parked_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel::<()>();
        let first_worker = thread::spawn({
            let run = Arc::clone(&run);
            let in_critical = Arc::clone(&in_critical);
            let overlap = Arc::clone(&overlap);
            let clock = Arc::clone(&clock);
            let first_exit_tick = Arc::clone(&first_exit_tick);
            move || {
                let settings = GlobalSettings {
                    exec_max_concurrent: 2,
                    ..GlobalSettings::default()
                };
                run.save_settings_with(&settings, || {
                    if in_critical.swap(true, Ordering::SeqCst) {
                        overlap.store(true, Ordering::SeqCst);
                    }
                    let _ = first_parked_tx.send(());
                    // Bounded park: exits after 10x grace even if the release
                    // send is lost, so no build of this test can hang here.
                    let _ = release_first_rx.recv_timeout(grace * 10);
                    first_exit_tick.store(clock.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                    in_critical.store(false, Ordering::SeqCst);
                    Ok(())
                })
            }
        });
        first_parked_rx
            .recv_timeout(grace)
            .expect("first worker reaches its persist closure");

        // Same-lock witness, recorded now and asserted after the joins: the
        // critical section worker 1 is parked in holds the live
        // `settings_update` handle, not a duplicate lock.
        let lock_held_while_parked = run.settings_update.try_lock().is_err();

        // Worker 2 signals immediately before calling the production path; the
        // first statement inside `save_settings_with` is the lock attempt. Its
        // persist closure records the limits already live at its entry — in the
        // correct build worker 1's publication precedes its settings unlock, so
        // worker 2 deterministically sees worker 1's cap.
        let (pre_attempt_tx, pre_attempt_rx) = mpsc::channel();
        let (second_entered_tx, second_entered_rx) = mpsc::channel();
        let second_worker = thread::spawn({
            let run = Arc::clone(&run);
            let in_critical = Arc::clone(&in_critical);
            let overlap = Arc::clone(&overlap);
            let clock = Arc::clone(&clock);
            let second_entry_tick = Arc::clone(&second_entry_tick);
            let permits_at_second_entry = Arc::clone(&permits_at_second_entry);
            move || {
                let settings = GlobalSettings {
                    exec_max_concurrent: 3,
                    ..GlobalSettings::default()
                };
                let _ = pre_attempt_tx.send(());
                run.save_settings_with(&settings, || {
                    second_entry_tick.store(clock.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                    if in_critical.swap(true, Ordering::SeqCst) {
                        overlap.store(true, Ordering::SeqCst);
                    }
                    // The entry signal must precede the `available_permits()`
                    // call: main holds the admission mutex for the publication
                    // window and waits (bounded) on this signal, so swapping
                    // the two blocks worker 2 on the frozen mutex before it
                    // can signal — a mutant build then stalls the full grace
                    // window every run and the witness degrades silently.
                    let _ = second_entered_tx.send(());
                    permits_at_second_entry.store(run.available_permits(), Ordering::SeqCst);
                    in_critical.store(false, Ordering::SeqCst);
                    Ok(())
                })
            }
        });
        pre_attempt_rx
            .recv_timeout(grace)
            .expect("second worker signals before entering the settings path");

        // Publication-window witness: while main holds the production admission
        // state mutex, worker 1's `admission.update` cannot complete. In the
        // correct build worker 1 therefore still holds the settings lock for the
        // whole window (publication is inside its critical section), so worker 2
        // cannot enter and this wait times out. A publish-outside-the-lock
        // mutant releases the settings lock with publication still pending, so
        // worker 2 enters and its signal arrives within the grace window. The
        // flag is set only by a received signal; the guard drop and worker 1's
        // barrier release are unconditional, so no build can hang.
        let frozen_publication = run
            .runtime
            .admission
            .state
            .lock()
            .expect("run admission mutex");
        let _ = release_first_tx.send(());
        let second_entered_before_first_publication = second_entered_rx.recv_timeout(grace).is_ok();
        drop(frozen_publication);

        first_worker
            .join()
            .expect("first worker completes")
            .expect("first settings update persists");
        second_worker
            .join()
            .expect("second worker completes")
            .expect("second settings update persists");

        assert!(
            !overlap.load(Ordering::SeqCst),
            "persist closures overlapped: the production settings lock was bypassed"
        );
        assert!(
            !second_entered_before_first_publication,
            "second worker entered its critical section before the first worker's \
             publication was live: publication escaped the settings critical section"
        );
        assert_eq!(
            permits_at_second_entry.load(Ordering::SeqCst),
            2,
            "worker 1's limits were not live at worker 2's persist entry"
        );
        assert!(
            lock_held_while_parked,
            "the settings critical section is not guarded by the live settings_update lock"
        );
        assert!(
            second_entry_tick.load(Ordering::SeqCst) > first_exit_tick.load(Ordering::SeqCst),
            "second worker's persist entry is not ordered after the first worker's exit"
        );
        // Publication is last-write-wins: the second worker's limits are live.
        assert_eq!(run.available_permits(), 3);
    }

    #[test]
    fn failed_settings_persistence_does_not_publish_new_admission_limits() {
        let run = RunService::for_test(1, Duration::from_secs(30), 1);
        let settings = GlobalSettings {
            exec_max_concurrent: 2,
            exec_rate_max: 2,
            ..GlobalSettings::default()
        };

        let result = run.save_settings_with(&settings, || {
            Err(StoreError::Backend("persist failed".to_string()))
        });
        assert!(matches!(result, Err(StoreError::Backend(_))));
        assert_eq!(run.available_permits(), 1);

        let client: IpAddr = "203.0.113.10".parse().unwrap();
        assert!(run.snapshot().admission.allow(client));
        assert!(!run.snapshot().admission.allow(client));
    }
}
