//! redb-backed persistence: the single volume file holding the admin account,
//! setup state, sessions, global settings, and the location catalogue (locations
//! and their test IPs / iperf endpoints / test files, plus the agents and
//! enrollment tokens a location owns). All state the container needs to survive a
//! restart lives here. redb has no SQL, so relations and the location cascade are
//! hand-rolled Rust over serde-encoded rows keyed by id (the redb-hold decision).

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rand_core::{OsRng, RngCore};
use redb::{Database, ReadableDatabase, ReadableTable, Table, TableDefinition, TableHandle};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use shared::liveness::is_online;
use shared::template::Method;
use shared::validate::PrefixFamily;

pub(crate) const ADMIN: TableDefinition<&str, &[u8]> = TableDefinition::new("admin");
pub(crate) const SETUP: TableDefinition<&str, &[u8]> = TableDefinition::new("setup");
pub(crate) const SESSION: TableDefinition<&str, &[u8]> = TableDefinition::new("session");
const SESSION_COOKIE_KEY_TABLE_NAME: &str = "session_cookie_key";
pub(crate) const SESSION_COOKIE_KEY: TableDefinition<&str, &[u8]> =
    TableDefinition::new(SESSION_COOKIE_KEY_TABLE_NAME);
pub(crate) const SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");
pub(crate) const LOCATION: TableDefinition<&str, &[u8]> = TableDefinition::new("location");
pub(crate) const TEST_IP: TableDefinition<&str, &[u8]> = TableDefinition::new("test_ip");
pub(crate) const IPERF: TableDefinition<&str, &[u8]> = TableDefinition::new("iperf_endpoint");
pub(crate) const TEST_FILE: TableDefinition<&str, &[u8]> = TableDefinition::new("test_file");
pub(crate) const AGENT: TableDefinition<&str, &[u8]> = TableDefinition::new("agent");
pub(crate) const ENROLLMENT_TOKEN: TableDefinition<&str, &[u8]> =
    TableDefinition::new("enrollment_token");

const ADMIN_KEY: &str = "admin";
const SETUP_KEY: &str = "state";
const SETTINGS_KEY: &str = "global";
const SESSION_COOKIE_KEY_ID: &str = "signing";
const SESSION_COOKIE_KEY_LEN: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Admin {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupState {
    pub installed: bool,
    pub completed_at: u64,
}

/// Which appearance the public and admin UIs default to before a visitor picks
/// their own (FR-016). `System` follows `prefers-color-scheme`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Editable global settings (FR-016 + the Slice-4 exec limits, AC25). The exec
/// and rate fields default from the `LG_EXEC_*` env vars so a deploy keeps its
/// prior behaviour until an admin overrides it; once set here they drive the run
/// path (`RunService::from_settings`). Every field is `#[serde(default)]` so a
/// settings row written by an earlier slice still deserializes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalSettings {
    #[serde(default = "default_site_title")]
    pub site_title: String,
    #[serde(default)]
    pub logo_url: Option<String>,
    #[serde(default)]
    pub default_theme: Theme,
    #[serde(default)]
    pub terms_url: Option<String>,
    #[serde(default)]
    pub custom_block: Option<String>,
    /// Global concurrency cap the exec engine is built with (FR-075/AC40).
    #[serde(default = "default_exec_max_concurrent")]
    pub exec_max_concurrent: usize,
    #[serde(default = "default_exec_timeout_secs")]
    pub exec_timeout_secs: u64,
    #[serde(default = "default_exec_max_output_kib")]
    pub exec_max_output_kib: usize,
    /// Per-client exec rate limit (FR-035): `max` requests per `window` seconds.
    #[serde(default = "default_exec_rate_max")]
    pub exec_rate_max: u32,
    #[serde(default = "default_exec_rate_window_secs")]
    pub exec_rate_window_secs: u64,
}

fn env_or<T: std::str::FromStr>(key: &str, fallback: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(fallback)
}

fn default_site_title() -> String {
    "Looking Glass".to_string()
}
fn default_exec_max_concurrent() -> usize {
    env_or("LG_EXEC_MAX_CONCURRENT", 8)
}
fn default_exec_timeout_secs() -> u64 {
    env_or("LG_EXEC_TIMEOUT_SECS", 30)
}
fn default_exec_max_output_kib() -> usize {
    env_or("LG_EXEC_MAX_OUTPUT_KIB", 256)
}
fn default_exec_rate_max() -> u32 {
    env_or("LG_EXEC_RATE_MAX", 20)
}
fn default_exec_rate_window_secs() -> u64 {
    env_or("LG_EXEC_RATE_WINDOW_SECS", 60)
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            site_title: default_site_title(),
            logo_url: None,
            default_theme: Theme::default(),
            terms_url: None,
            custom_block: None,
            exec_max_concurrent: default_exec_max_concurrent(),
            exec_timeout_secs: default_exec_timeout_secs(),
            exec_max_output_kib: default_exec_max_output_kib(),
            exec_rate_max: default_exec_rate_max(),
            exec_rate_window_secs: default_exec_rate_window_secs(),
        }
    }
}

/// Whether a location runs on the central container's built-in node or a remote
/// enrolled agent (FR-011). Remote nodes are enrolled in Slice 7+.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Local,
    Remote,
}

/// Whether a location is currently reachable for runs. A local node is online by
/// definition; a remote node stays offline until its agent enrolls and heartbeats
/// (Slice 8b owns the transition — this slice only sets the initial value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocationStatus {
    Online,
    Offline,
}

/// A diagnostic method an admin can offer at a location (FR-015). Includes `bgp`/
/// `bgp6`, which an admin enables where a routing daemon is present; whether the
/// daemon is actually available is gated node-side at run time (FR-036).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfferedMethod {
    Ping,
    Ping6,
    Mtr,
    Mtr6,
    Traceroute,
    Traceroute6,
    Bgp,
    Bgp6,
}

/// A method a location offers that the run path can dispatch. Diagnostics take a
/// validated target and run an argv tool ([`Method`]); BGP takes a grammar-validated
/// prefix and shells to the node's routing daemon, so it is a distinct variant
/// rather than a [`Method`] — the run path branches on which (decisions.md
/// "Slice 11 checkpoint": BGP is prefix-based, not a target method).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnableMethod {
    Diagnostic(Method),
    Bgp(PrefixFamily),
}

impl OfferedMethod {
    /// The [`RunnableMethod`] this offering maps to. Every offered method is
    /// runnable as of Slice 11 (BGP now shells to a routing daemon); the run path
    /// still gates BGP on daemon presence at execution time.
    pub fn runnable(self) -> RunnableMethod {
        match self {
            OfferedMethod::Ping => RunnableMethod::Diagnostic(Method::Ping),
            OfferedMethod::Ping6 => RunnableMethod::Diagnostic(Method::Ping6),
            OfferedMethod::Mtr => RunnableMethod::Diagnostic(Method::Mtr),
            OfferedMethod::Mtr6 => RunnableMethod::Diagnostic(Method::Mtr6),
            OfferedMethod::Traceroute => RunnableMethod::Diagnostic(Method::Traceroute),
            OfferedMethod::Traceroute6 => RunnableMethod::Diagnostic(Method::Traceroute6),
            OfferedMethod::Bgp => RunnableMethod::Bgp(PrefixFamily::V4),
            OfferedMethod::Bgp6 => RunnableMethod::Bgp(PrefixFamily::V6),
        }
    }
}

/// Address family of a test IP (FR-012).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    V4,
    V6,
}

/// A diagnostic location (FR-010/011). `offered_methods` gates what is runnable
/// there; `status` gates public selectability (FR-026).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub id: String,
    pub name: String,
    pub geo_label: String,
    pub map_query: Option<String>,
    pub facility: Option<String>,
    pub facility_url: Option<String>,
    pub kind: NodeKind,
    #[serde(default)]
    pub data_plane_origin: Option<String>,
    pub offered_methods: Vec<OfferedMethod>,
    pub status: LocationStatus,
    pub created_at: u64,
}

impl Location {
    /// The offered methods that can be dispatched at this location — the runnable
    /// set the run path enforces (FR-015). A method not offered here is absent; a
    /// BGP offering maps to its family and is gated on daemon presence at run time.
    pub fn runnable_methods(&self) -> Vec<RunnableMethod> {
        self.offered_methods.iter().map(|m| m.runnable()).collect()
    }
}

/// Derive a location's live status at time `now` from its agents (compute-on-read,
/// decisions.md "Slice 8b liveness design"). A local node runs on the container's
/// built-in node and is online by definition; a remote node is online iff any of its
/// agents is [`is_online`] — a recent heartbeat AND not revoked. The persisted
/// `location.status` is never consulted here: online is derived, not stored, so it is
/// restart-safe and a revoked agent can never resurrect the location.
pub fn derive_location_status(location: &Location, agents: &[Agent], now: u64) -> LocationStatus {
    match location.kind {
        NodeKind::Local => LocationStatus::Online,
        NodeKind::Remote => {
            if agents
                .iter()
                .any(|agent| is_online(agent.last_seen, now, agent.revoked))
            {
                LocationStatus::Online
            } else {
                LocationStatus::Offline
            }
        }
    }
}

/// The most recent heartbeat across a remote location's agents, for the admin
/// last-seen column. `None` for a local node (no agent) or a remote whose agents have
/// never beaten.
pub fn latest_last_seen(location: &Location, agents: &[Agent]) -> Option<u64> {
    match location.kind {
        NodeKind::Local => None,
        NodeKind::Remote => agents
            .iter()
            .filter(|agent| !agent.revoked)
            .filter_map(|agent| agent.last_seen)
            .max(),
    }
}

/// A test IP address a location advertises for visitors to target (FR-012).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestIp {
    pub id: String,
    pub location_id: String,
    pub family: Family,
    pub address: String,
    pub label: Option<String>,
}

/// An iperf endpoint a location advertises, with the copy-paste command strings
/// shown to visitors (FR-013).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IperfEndpoint {
    pub id: String,
    pub location_id: String,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub cmd_incoming: String,
    pub cmd_outgoing: String,
}

/// A downloadable test file a location advertises for speed testing (FR-014).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFile {
    pub id: String,
    pub location_id: String,
    pub label: String,
    pub declared_size: String,
    pub source_ref: String,
}

/// An enrolled remote agent (FR-024). `credential_hash` is the Argon2id hash of the
/// long-lived per-agent credential — the cleartext is returned to the agent exactly
/// once at enrollment and never stored. `revoked` and `last_seen` are consumed by
/// the revoke (Slice 9) and liveness (Slice 8b) slices; this slice only issues.
/// Carries a top-level `location_id` so a location delete cascades to it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub location_id: String,
    pub credential_hash: String,
    pub enrolled_at: u64,
    #[serde(default)]
    pub last_seen: Option<u64>,
    #[serde(default)]
    pub revoked: bool,
}

/// A single-use, time-limited enrollment token (FR-023). Only the SHA-256 hash of
/// the token is stored — the raw token lives in the operator's install command and
/// nowhere at rest. `used_at` enforces single-use; `expires_at` enforces the TTL.
/// Carries a top-level `location_id` so a location delete cascades to it — the
/// invariant that closes the "revoked location's token still valid" hole.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentToken {
    pub id: String,
    pub location_id: String,
    pub token_hash: String,
    pub expires_at: u64,
    #[serde(default)]
    pub used_at: Option<u64>,
}

/// The location-id field every child row carries — deserialized on its own so the
/// cascade can match children by parent without knowing each full child shape.
#[derive(Deserialize)]
struct ChildRef {
    location_id: String,
}

#[derive(Debug)]
pub enum StoreError {
    AlreadyInstalled,
    Backend(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::AlreadyInstalled => f.write_str("setup already completed"),
            StoreError::Backend(msg) => write!(f, "store backend error: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

fn backend<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Backend(e.to_string())
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A handle to the single redb volume file, cheap to clone (shared `Arc`).
#[derive(Clone)]
pub struct Store {
    db: Arc<Database>,
    session_cookie_key: Arc<[u8; SESSION_COOKIE_KEY_LEN]>,
}

pub struct DeletedLocation {
    pub existed: bool,
    pub agent_ids: Vec<String>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Store")
    }
}

impl Store {
    /// Open (creating if absent) the volume file and bootstrap every table so a
    /// fresh deploy starts with the four tables present and default settings seeded.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db = Arc::new(Database::create(path).map_err(backend)?);
        let session_cookie_key = Self::bootstrap(&db)?;
        Ok(Self {
            db,
            session_cookie_key: Arc::new(session_cookie_key),
        })
    }

    pub(crate) fn database(&self) -> Arc<Database> {
        Arc::clone(&self.db)
    }

    pub(crate) fn session_cookie_key(&self) -> &[u8; SESSION_COOKIE_KEY_LEN] {
        &self.session_cookie_key
    }

    fn bootstrap(db: &Database) -> Result<[u8; SESSION_COOKIE_KEY_LEN], StoreError> {
        let txn = db.begin_write().map_err(backend)?;
        txn.open_table(ADMIN).map_err(backend)?;
        txn.open_table(SETUP).map_err(backend)?;
        txn.open_table(SESSION).map_err(backend)?;
        txn.open_table(LOCATION).map_err(backend)?;
        txn.open_table(TEST_IP).map_err(backend)?;
        txn.open_table(IPERF).map_err(backend)?;
        txn.open_table(TEST_FILE).map_err(backend)?;
        txn.open_table(AGENT).map_err(backend)?;
        txn.open_table(ENROLLMENT_TOKEN).map_err(backend)?;
        {
            let mut settings = txn.open_table(SETTINGS).map_err(backend)?;
            if settings.get(SETTINGS_KEY).map_err(backend)?.is_none() {
                let encoded = serde_json::to_vec(&GlobalSettings::default()).map_err(backend)?;
                settings
                    .insert(SETTINGS_KEY, encoded.as_slice())
                    .map_err(backend)?;
            }
        }
        // ponytail: a missing table is indistinguishable from a pre-Slice-19 volume,
        // so the absent-table path creates a key; an existing empty table fails closed.
        let session_cookie_key_table_exists = txn
            .list_tables()
            .map_err(backend)?
            .any(|table| table.name() == SESSION_COOKIE_KEY_TABLE_NAME);
        let session_cookie_key = {
            let mut table = txn.open_table(SESSION_COOKIE_KEY).map_err(backend)?;
            let existing_key = match table.get(SESSION_COOKIE_KEY_ID).map_err(backend)? {
                Some(value) => {
                    let key: &[u8; SESSION_COOKIE_KEY_LEN] =
                        value.value().try_into().map_err(|_| {
                            StoreError::Backend("invalid session cookie signing key".to_string())
                        })?;
                    Some(*key)
                }
                None => None,
            };
            match existing_key {
                Some(key) => key,
                None if session_cookie_key_table_exists => {
                    return Err(StoreError::Backend(
                        "missing session cookie signing key".to_string(),
                    ));
                }
                None => {
                    let mut key = [0; SESSION_COOKIE_KEY_LEN];
                    OsRng.try_fill_bytes(&mut key).map_err(|_| {
                        StoreError::Backend(
                            "OS randomness unavailable for session cookie signing key".to_string(),
                        )
                    })?;
                    table
                        .insert(SESSION_COOKIE_KEY_ID, key.as_slice())
                        .map_err(backend)?;
                    key
                }
            }
        };
        txn.commit().map_err(backend)?;
        Ok(session_cookie_key)
    }

    pub fn is_installed(&self) -> Result<bool, StoreError> {
        Ok(self.setup_state()?.map(|s| s.installed).unwrap_or(false))
    }

    fn setup_state(&self) -> Result<Option<SetupState>, StoreError> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(SETUP).map_err(backend)?;
        match table.get(SETUP_KEY).map_err(backend)? {
            Some(guard) => Ok(Some(
                serde_json::from_slice(guard.value()).map_err(backend)?,
            )),
            None => Ok(None),
        }
    }

    pub fn admin(&self) -> Result<Option<Admin>, StoreError> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(ADMIN).map_err(backend)?;
        match table.get(ADMIN_KEY).map_err(backend)? {
            Some(guard) => Ok(Some(
                serde_json::from_slice(guard.value()).map_err(backend)?,
            )),
            None => Ok(None),
        }
    }

    pub fn settings(&self) -> Result<GlobalSettings, StoreError> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(SETTINGS).map_err(backend)?;
        match table.get(SETTINGS_KEY).map_err(backend)? {
            Some(guard) => Ok(serde_json::from_slice(guard.value()).map_err(backend)?),
            None => Ok(GlobalSettings::default()),
        }
    }

    /// Create the one admin account and mark setup complete in a single write
    /// transaction. redb serializes writers, so a concurrent second call sees the
    /// existing admin and returns `AlreadyInstalled` — the single-admin invariant
    /// and the closed-installer guarantee are both atomic here.
    pub fn create_admin(
        &self,
        id: String,
        username: String,
        password_hash: String,
    ) -> Result<Admin, StoreError> {
        let txn = self.db.begin_write().map_err(backend)?;
        let admin = {
            let mut admins = txn.open_table(ADMIN).map_err(backend)?;
            if admins.get(ADMIN_KEY).map_err(backend)?.is_some() {
                return Err(StoreError::AlreadyInstalled);
            }
            let admin = Admin {
                id,
                username,
                password_hash,
                created_at: unix_now(),
            };
            let encoded = serde_json::to_vec(&admin).map_err(backend)?;
            admins
                .insert(ADMIN_KEY, encoded.as_slice())
                .map_err(backend)?;
            admin
        };
        {
            let mut setup = txn.open_table(SETUP).map_err(backend)?;
            let state = SetupState {
                installed: true,
                completed_at: unix_now(),
            };
            let encoded = serde_json::to_vec(&state).map_err(backend)?;
            setup
                .insert(SETUP_KEY, encoded.as_slice())
                .map_err(backend)?;
        }
        txn.commit().map_err(backend)?;
        Ok(admin)
    }

    pub(crate) fn persist_settings(&self, settings: &GlobalSettings) -> Result<(), StoreError> {
        self.write_record(SETTINGS, SETTINGS_KEY, settings)
    }

    pub fn list_locations(&self) -> Result<Vec<Location>, StoreError> {
        let mut locations: Vec<Location> = self.read_all(LOCATION)?;
        locations.sort_by_key(|location| location.created_at);
        Ok(locations)
    }

    pub fn get_location(&self, id: &str) -> Result<Option<Location>, StoreError> {
        self.read_record(LOCATION, id)
    }

    /// Upsert a location by id. Callers use a fresh id + `unix_now` for a create
    /// and the existing row's id for an edit, so this is the single write path.
    pub fn put_location(&self, location: &Location) -> Result<(), StoreError> {
        self.write_record(LOCATION, &location.id, location)
    }

    /// Delete a location and **every** child row that belongs to it, in one write
    /// transaction (risk #6). Enumerates each child table by name so no child type
    /// is silently missed; the whole delete commits or none of it does — no
    /// orphaned test IPs, iperf endpoints, files, agents, or tokens. Returns
    /// whether the location existed.
    pub fn delete_location(&self, id: &str) -> Result<bool, StoreError> {
        Ok(self.delete_location_with_agents(id)?.existed)
    }

    /// Delete a location and return the agent ids removed by the cascade so the
    /// caller can kick any live tunnels for those agents.
    pub fn delete_location_with_agents(&self, id: &str) -> Result<DeletedLocation, StoreError> {
        let txn = self.db.begin_write().map_err(backend)?;
        let (existed, agent_ids) = {
            let mut locations = txn.open_table(LOCATION).map_err(backend)?;
            let existed = locations.remove(id).map_err(backend)?.is_some();
            purge_children(&mut txn.open_table(TEST_IP).map_err(backend)?, id)?;
            purge_children(&mut txn.open_table(IPERF).map_err(backend)?, id)?;
            purge_children(&mut txn.open_table(TEST_FILE).map_err(backend)?, id)?;
            let mut agents = txn.open_table(AGENT).map_err(backend)?;
            let agent_ids = child_ids(&mut agents, id)?;
            purge_children(&mut agents, id)?;
            purge_children(&mut txn.open_table(ENROLLMENT_TOKEN).map_err(backend)?, id)?;
            (existed, agent_ids)
        };
        txn.commit().map_err(backend)?;
        Ok(DeletedLocation { existed, agent_ids })
    }

    pub fn list_test_ips(&self, location_id: &str) -> Result<Vec<TestIp>, StoreError> {
        self.read_children(TEST_IP, location_id)
    }
    pub fn get_test_ip(&self, id: &str) -> Result<Option<TestIp>, StoreError> {
        self.read_record(TEST_IP, id)
    }
    pub fn put_test_ip(&self, test_ip: &TestIp) -> Result<(), StoreError> {
        self.write_record(TEST_IP, &test_ip.id, test_ip)
    }
    pub fn delete_test_ip(&self, id: &str) -> Result<bool, StoreError> {
        self.remove_record(TEST_IP, id)
    }

    pub fn list_iperf(&self, location_id: &str) -> Result<Vec<IperfEndpoint>, StoreError> {
        self.read_children(IPERF, location_id)
    }
    pub fn get_iperf(&self, id: &str) -> Result<Option<IperfEndpoint>, StoreError> {
        self.read_record(IPERF, id)
    }
    pub fn put_iperf(&self, endpoint: &IperfEndpoint) -> Result<(), StoreError> {
        self.write_record(IPERF, &endpoint.id, endpoint)
    }
    pub fn delete_iperf(&self, id: &str) -> Result<bool, StoreError> {
        self.remove_record(IPERF, id)
    }

    pub fn list_test_files(&self, location_id: &str) -> Result<Vec<TestFile>, StoreError> {
        self.read_children(TEST_FILE, location_id)
    }
    pub fn get_test_file(&self, id: &str) -> Result<Option<TestFile>, StoreError> {
        self.read_record(TEST_FILE, id)
    }
    pub fn put_test_file(&self, file: &TestFile) -> Result<(), StoreError> {
        self.write_record(TEST_FILE, &file.id, file)
    }
    pub fn delete_test_file(&self, id: &str) -> Result<bool, StoreError> {
        self.remove_record(TEST_FILE, id)
    }

    // ----- Agents + enrollment tokens (Slice 7) ------------------------------

    /// Persist an enrollment token (its hash + TTL). Keyed by the token's own id;
    /// enrollment looks it up by `token_hash` via [`Self::find_token_by_hash`].
    pub fn put_enrollment_token(&self, token: &EnrollmentToken) -> Result<(), StoreError> {
        self.write_record(ENROLLMENT_TOKEN, &token.id, token)
    }

    /// Find the (at most one) unexpired-or-not token whose stored hash matches — a
    /// full scan, since redb has no secondary index (the accepted redb-hold cost).
    /// Expiry/single-use are judged by the caller against the returned row.
    pub fn find_token_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<EnrollmentToken>, StoreError> {
        Ok(self
            .read_all::<EnrollmentToken>(ENROLLMENT_TOKEN)?
            .into_iter()
            .find(|token| token.token_hash == token_hash))
    }

    /// Atomically consume a token: in one write transaction, re-read it and mark it
    /// used only if it was not already used. Returns `true` when this call is the one
    /// that consumed it, `false` if it was already used or absent — so two racing
    /// enrollments can never both succeed on one token (single-use, TOCTOU-free).
    pub fn consume_enrollment_token(&self, id: &str, used_at: u64) -> Result<bool, StoreError> {
        let txn = self.db.begin_write().map_err(backend)?;
        let consumed = {
            let mut table = txn.open_table(ENROLLMENT_TOKEN).map_err(backend)?;
            let current: Option<EnrollmentToken> = match table.get(id).map_err(backend)? {
                Some(guard) => Some(serde_json::from_slice(guard.value()).map_err(backend)?),
                None => None,
            };
            match current {
                Some(mut token) if token.used_at.is_none() => {
                    token.used_at = Some(used_at);
                    let encoded = serde_json::to_vec(&token).map_err(backend)?;
                    table.insert(id, encoded.as_slice()).map_err(backend)?;
                    true
                }
                _ => false,
            }
        };
        txn.commit().map_err(backend)?;
        Ok(consumed)
    }

    pub fn put_agent(&self, agent: &Agent) -> Result<(), StoreError> {
        self.write_record(AGENT, &agent.id, agent)
    }

    pub fn get_agent(&self, id: &str) -> Result<Option<Agent>, StoreError> {
        self.read_record(AGENT, id)
    }

    pub fn list_agents(&self, location_id: &str) -> Result<Vec<Agent>, StoreError> {
        self.read_children(AGENT, location_id)
    }

    /// Every enrolled agent, across all locations — the single scan the read
    /// boundary groups by `location_id` to derive each location's live status
    /// without an N+1 per-location query.
    pub fn all_agents(&self) -> Result<Vec<Agent>, StoreError> {
        self.read_all(AGENT)
    }

    /// Revoke every agent enrolled for `location_id` in one write transaction,
    /// clearing `last_seen` so the admin state returns to not-enrolled. Returns the
    /// revoked agent ids so the caller can kick live tunnels for the same agents.
    pub fn revoke_agents_for_location(&self, location_id: &str) -> Result<Vec<String>, StoreError> {
        let txn = self.db.begin_write().map_err(backend)?;
        let mut revoked = Vec::new();
        {
            let mut table = txn.open_table(AGENT).map_err(backend)?;
            let mut agents = Vec::new();
            for entry in table.iter().map_err(backend)? {
                let (_key, value) = entry.map_err(backend)?;
                let mut agent: Agent = serde_json::from_slice(value.value()).map_err(backend)?;
                if agent.location_id == location_id {
                    agent.revoked = true;
                    agent.last_seen = None;
                    agents.push(agent);
                }
            }
            agents.sort_by(|a, b| a.id.cmp(&b.id));
            for agent in agents {
                let encoded = serde_json::to_vec(&agent).map_err(backend)?;
                table
                    .insert(agent.id.as_str(), encoded.as_slice())
                    .map_err(backend)?;
                revoked.push(agent.id);
            }
        }
        txn.commit().map_err(backend)?;
        Ok(revoked)
    }

    /// Record an agent's proof-of-life (a received tunnel heartbeat), advancing its
    /// `last_seen` to `ts`. A read-modify-write in one transaction so it preserves
    /// `revoked` — recording liveness can never un-revoke an agent, and `is_online`
    /// still derives a revoked agent offline. A no-op for an unknown/absent agent.
    pub fn touch_agent_last_seen(&self, id: &str, ts: u64) -> Result<(), StoreError> {
        let txn = self.db.begin_write().map_err(backend)?;
        {
            let mut table = txn.open_table(AGENT).map_err(backend)?;
            let current: Option<Agent> = match table.get(id).map_err(backend)? {
                Some(guard) => Some(serde_json::from_slice(guard.value()).map_err(backend)?),
                None => None,
            };
            if let Some(mut agent) = current.filter(|agent| !agent.revoked) {
                agent.last_seen = Some(ts);
                let encoded = serde_json::to_vec(&agent).map_err(backend)?;
                table.insert(id, encoded.as_slice()).map_err(backend)?;
            }
        }
        txn.commit().map_err(backend)?;
        Ok(())
    }

    fn write_record<T: Serialize>(
        &self,
        def: TableDefinition<&str, &[u8]>,
        key: &str,
        value: &T,
    ) -> Result<(), StoreError> {
        let encoded = serde_json::to_vec(value).map_err(backend)?;
        let txn = self.db.begin_write().map_err(backend)?;
        {
            let mut table = txn.open_table(def).map_err(backend)?;
            table.insert(key, encoded.as_slice()).map_err(backend)?;
        }
        txn.commit().map_err(backend)?;
        Ok(())
    }

    fn read_record<T: DeserializeOwned>(
        &self,
        def: TableDefinition<&str, &[u8]>,
        key: &str,
    ) -> Result<Option<T>, StoreError> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(def).map_err(backend)?;
        match table.get(key).map_err(backend)? {
            Some(guard) => Ok(Some(
                serde_json::from_slice(guard.value()).map_err(backend)?,
            )),
            None => Ok(None),
        }
    }

    fn read_all<T: DeserializeOwned>(
        &self,
        def: TableDefinition<&str, &[u8]>,
    ) -> Result<Vec<T>, StoreError> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(def).map_err(backend)?;
        let mut out = Vec::new();
        for entry in table.iter().map_err(backend)? {
            let (_key, value) = entry.map_err(backend)?;
            out.push(serde_json::from_slice(value.value()).map_err(backend)?);
        }
        Ok(out)
    }

    /// Every row in `def` whose `location_id` matches — redb has no secondary
    /// index, so this filters a full scan in Rust (the accepted cost of the redb
    /// hold, over a small dataset).
    fn read_children<T: DeserializeOwned + HasLocation>(
        &self,
        def: TableDefinition<&str, &[u8]>,
        location_id: &str,
    ) -> Result<Vec<T>, StoreError> {
        Ok(self
            .read_all::<T>(def)?
            .into_iter()
            .filter(|child| child.location_id() == location_id)
            .collect())
    }

    fn remove_record(
        &self,
        def: TableDefinition<&str, &[u8]>,
        key: &str,
    ) -> Result<bool, StoreError> {
        let txn = self.db.begin_write().map_err(backend)?;
        let existed = {
            let mut table = txn.open_table(def).map_err(backend)?;
            let removed = table.remove(key).map_err(backend)?.is_some();
            removed
        };
        txn.commit().map_err(backend)?;
        Ok(existed)
    }
}

/// Read `location_id` from a child so `read_children` can filter generically.
trait HasLocation {
    fn location_id(&self) -> &str;
}
impl HasLocation for TestIp {
    fn location_id(&self) -> &str {
        &self.location_id
    }
}
impl HasLocation for IperfEndpoint {
    fn location_id(&self) -> &str {
        &self.location_id
    }
}
impl HasLocation for TestFile {
    fn location_id(&self) -> &str {
        &self.location_id
    }
}
impl HasLocation for Agent {
    fn location_id(&self) -> &str {
        &self.location_id
    }
}
impl HasLocation for EnrollmentToken {
    fn location_id(&self) -> &str {
        &self.location_id
    }
}

/// Remove every row in one child table that belongs to `location_id`. Ids are
/// collected first (the iterator borrows the table) then removed, so the whole
/// cascade runs inside its caller's single write transaction.
fn purge_children(table: &mut Table<&str, &[u8]>, location_id: &str) -> Result<(), StoreError> {
    let victims = child_ids(table, location_id)?;
    for id in &victims {
        table.remove(id.as_str()).map_err(backend)?;
    }
    Ok(())
}

fn child_ids(table: &mut Table<&str, &[u8]>, location_id: &str) -> Result<Vec<String>, StoreError> {
    let victims: Vec<String> = {
        let mut ids = Vec::new();
        for entry in table.iter().map_err(backend)? {
            let (key, value) = entry.map_err(backend)?;
            let child: ChildRef = serde_json::from_slice(value.value()).map_err(backend)?;
            if child.location_id == location_id {
                ids.push(key.value().to_string());
            }
        }
        ids
    };
    let mut victims = victims;
    victims.sort();
    Ok(victims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_store() -> Store {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lg-store-test-{}-{}-{}.redb",
            std::process::id(),
            n,
            nanos
        ));
        Store::open(path).expect("open temp store")
    }

    fn location(id: &str, kind: NodeKind, offered: Vec<OfferedMethod>) -> Location {
        Location {
            id: id.to_string(),
            name: format!("loc-{id}"),
            geo_label: "Test City".to_string(),
            map_query: None,
            facility: None,
            facility_url: None,
            kind,
            data_plane_origin: None,
            offered_methods: offered,
            status: LocationStatus::Online,
            created_at: 0,
        }
    }

    fn seed_full_location(store: &Store, id: &str) {
        store
            .put_location(&location(id, NodeKind::Remote, vec![OfferedMethod::Ping]))
            .unwrap();
        store
            .put_test_ip(&TestIp {
                id: format!("{id}-ip"),
                location_id: id.to_string(),
                family: Family::V4,
                address: "203.0.113.5".to_string(),
                label: None,
            })
            .unwrap();
        store
            .put_iperf(&IperfEndpoint {
                id: format!("{id}-iperf"),
                location_id: id.to_string(),
                label: "iperf".to_string(),
                host: "203.0.113.5".to_string(),
                port: 5201,
                cmd_incoming: "iperf3 -c host".to_string(),
                cmd_outgoing: "iperf3 -c host -R".to_string(),
            })
            .unwrap();
        store
            .put_test_file(&TestFile {
                id: format!("{id}-file"),
                location_id: id.to_string(),
                label: "1GB".to_string(),
                declared_size: "1 GB".to_string(),
                source_ref: "/files/1g.bin".to_string(),
            })
            .unwrap();
        // Real typed Agent + EnrollmentToken rows (Slice 7): the cascade must clear
        // these too, and typed rows — not JSON placeholders — are what proves it.
        store
            .put_agent(&Agent {
                id: format!("{id}-agent"),
                location_id: id.to_string(),
                credential_hash: "$argon2id$stub".to_string(),
                enrolled_at: 0,
                last_seen: None,
                revoked: false,
            })
            .unwrap();
        store
            .put_enrollment_token(&EnrollmentToken {
                id: format!("{id}-token"),
                location_id: id.to_string(),
                token_hash: format!("hash-{id}"),
                expires_at: 0,
                used_at: None,
            })
            .unwrap();
    }

    fn children_for(store: &Store, location_id: &str) -> usize {
        let ips = store.list_test_ips(location_id).unwrap().len();
        let iperf = store.list_iperf(location_id).unwrap().len();
        let files = store.list_test_files(location_id).unwrap().len();
        let agents = store
            .read_all::<ChildRef>(AGENT)
            .unwrap()
            .into_iter()
            .filter(|a| a.location_id == location_id)
            .count();
        let tokens = store
            .read_all::<ChildRef>(ENROLLMENT_TOKEN)
            .unwrap()
            .into_iter()
            .filter(|t| t.location_id == location_id)
            .count();
        ips + iperf + files + agents + tokens
    }

    // Risk #6 crux: deleting a location removes EVERY child row that belongs to
    // it — test IP, iperf endpoint, test file, agent, and enrollment token — and
    // leaves a sibling location's children untouched. No orphaned child survives.
    #[test]
    fn deleting_a_location_cascades_to_every_child_table() {
        let store = temp_store();
        seed_full_location(&store, "target");
        seed_full_location(&store, "keep");
        assert_eq!(
            children_for(&store, "target"),
            5,
            "one row per child table seeded"
        );
        assert_eq!(children_for(&store, "keep"), 5);

        assert!(store.delete_location("target").unwrap());

        assert_eq!(
            children_for(&store, "target"),
            0,
            "every child table must be empty for the deleted location — no orphans"
        );
        assert!(store.get_location("target").unwrap().is_none());
        assert_eq!(
            children_for(&store, "keep"),
            5,
            "a sibling location's children must be untouched"
        );
        assert!(store.get_location("keep").unwrap().is_some());
    }

    // Slice 7 binding invariant (drift.md): a location delete must remove its
    // typed Agent AND EnrollmentToken rows — proven by looking each specific row up
    // after the delete, not by a count of placeholders. This is the "revoked
    // location's token still valid" hole closed: a token whose location is gone must
    // itself be gone (or a reused-location token could re-enroll after revocation).
    #[test]
    fn deleting_a_location_removes_its_typed_agent_and_token_rows() {
        let store = temp_store();
        store
            .put_location(&location("loc", NodeKind::Remote, vec![]))
            .unwrap();
        store
            .put_agent(&Agent {
                id: "agent-1".to_string(),
                location_id: "loc".to_string(),
                credential_hash: "$argon2id$stub".to_string(),
                enrolled_at: 10,
                last_seen: Some(20),
                revoked: false,
            })
            .unwrap();
        store
            .put_enrollment_token(&EnrollmentToken {
                id: "token-1".to_string(),
                location_id: "loc".to_string(),
                token_hash: "abc123".to_string(),
                expires_at: 9999,
                used_at: None,
            })
            .unwrap();
        assert!(store.get_agent("agent-1").unwrap().is_some());
        assert!(store.find_token_by_hash("abc123").unwrap().is_some());

        assert!(store.delete_location("loc").unwrap());

        assert!(
            store.get_agent("agent-1").unwrap().is_none(),
            "the deleted location's agent row must be gone"
        );
        assert!(
            store.find_token_by_hash("abc123").unwrap().is_none(),
            "the deleted location's enrollment token must be gone — not left valid"
        );
    }

    #[test]
    fn deleting_a_location_returns_removed_agent_ids_for_tunnel_kick() {
        let store = temp_store();
        store
            .put_location(&location("loc", NodeKind::Remote, vec![]))
            .unwrap();
        store.put_agent(&agent("a1", "loc", None, false)).unwrap();
        store.put_agent(&agent("a2", "loc", None, false)).unwrap();
        store
            .put_agent(&agent("other", "other-loc", None, false))
            .unwrap();

        let deleted = store.delete_location_with_agents("loc").unwrap();

        assert!(deleted.existed);
        assert_eq!(deleted.agent_ids, vec!["a1".to_string(), "a2".to_string()]);
        assert!(store.get_agent("a1").unwrap().is_none());
        assert!(store.get_agent("other").unwrap().is_some());
    }

    // FR-023 single-use: the first consume wins and marks the token used; a second
    // consume of the same token returns false and issues nothing. The store makes
    // this atomic so two racing enrollments cannot both consume one token.
    #[test]
    fn a_token_can_be_consumed_exactly_once() {
        let store = temp_store();
        store
            .put_enrollment_token(&EnrollmentToken {
                id: "t".to_string(),
                location_id: "loc".to_string(),
                token_hash: "h".to_string(),
                expires_at: 9999,
                used_at: None,
            })
            .unwrap();
        assert!(store.consume_enrollment_token("t", 100).unwrap());
        assert!(
            !store.consume_enrollment_token("t", 200).unwrap(),
            "a second consume of the same token must fail"
        );
        assert_eq!(
            store.find_token_by_hash("h").unwrap().unwrap().used_at,
            Some(100)
        );
    }

    #[test]
    fn deleting_a_missing_location_reports_absent_and_touches_nothing() {
        let store = temp_store();
        seed_full_location(&store, "keep");
        assert!(!store.delete_location("ghost").unwrap());
        assert_eq!(children_for(&store, "keep"), 5);
    }

    // FR-015 admin side: a location's runnable set is exactly its offered methods,
    // mapped to what the run path dispatches — diagnostics to a [`Method`], BGP to
    // its family (Slice 11 makes BGP runnable); an unoffered method is absent.
    #[test]
    fn runnable_methods_maps_diagnostics_and_bgp_and_excludes_unoffered() {
        let loc = location(
            "m",
            NodeKind::Local,
            vec![
                OfferedMethod::Ping,
                OfferedMethod::Mtr,
                OfferedMethod::Bgp,
                OfferedMethod::Bgp6,
            ],
        );
        let runnable = loc.runnable_methods();
        assert!(runnable.contains(&RunnableMethod::Diagnostic(Method::Ping)));
        assert!(runnable.contains(&RunnableMethod::Diagnostic(Method::Mtr)));
        assert!(
            runnable.contains(&RunnableMethod::Bgp(PrefixFamily::V4)),
            "an offered BGP maps to its v4 family and is now runnable"
        );
        assert!(runnable.contains(&RunnableMethod::Bgp(PrefixFamily::V6)));
        assert!(
            !runnable.contains(&RunnableMethod::Diagnostic(Method::Traceroute)),
            "a method not offered here must not be runnable"
        );
        assert_eq!(runnable.len(), 4);
    }

    // AC25: an exec/rate setting persists and reads back, and env supplies the
    // fallback default when no admin has overridden it.
    #[test]
    fn settings_persist_and_default_from_env_fallback() {
        let store = temp_store();
        // Fresh install seeds defaults (env unset here → the hardcoded fallbacks).
        let seeded = store.settings().unwrap();
        assert_eq!(seeded.exec_max_concurrent, 8);
        assert_eq!(seeded.exec_rate_max, 20);

        let custom = GlobalSettings {
            site_title: "My Glass".to_string(),
            exec_max_concurrent: 2,
            exec_rate_max: 3,
            ..GlobalSettings::default()
        };
        store.persist_settings(&custom).unwrap();

        let read = store.settings().unwrap();
        assert_eq!(read.site_title, "My Glass");
        assert_eq!(read.exec_max_concurrent, 2);
        assert_eq!(read.exec_rate_max, 3);
    }

    // A settings row written by an earlier slice (only `site_title`) must still
    // deserialize, with the new fields filled from their defaults.
    #[test]
    fn legacy_settings_row_deserializes_with_defaults() {
        let store = temp_store();
        // Write a settings row shaped like an earlier slice's (only `site_title`).
        let txn = store.db.begin_write().unwrap();
        {
            let mut table = txn.open_table(SETTINGS).unwrap();
            table
                .insert(SETTINGS_KEY, br#"{"site_title":"Legacy"}"#.as_slice())
                .unwrap();
        }
        txn.commit().unwrap();
        let read = store.settings().unwrap();
        assert_eq!(read.site_title, "Legacy");
        assert_eq!(read.exec_max_concurrent, 8);
        assert_eq!(read.default_theme, Theme::System);
    }

    // ----- Slice 8b: heartbeat liveness (compute-on-read) --------------------

    fn agent(id: &str, location_id: &str, last_seen: Option<u64>, revoked: bool) -> Agent {
        Agent {
            id: id.to_string(),
            location_id: location_id.to_string(),
            credential_hash: "$argon2id$stub".to_string(),
            enrolled_at: 0,
            last_seen,
            revoked,
        }
    }

    // touch_agent_last_seen advances last_seen only for an active agent. A revoked
    // row stays not-enrolled even if a heartbeat races with revoke.
    #[test]
    fn touch_is_a_no_op_for_a_revoked_agent() {
        let store = temp_store();
        store.put_agent(&agent("a1", "loc", None, true)).unwrap();
        store.touch_agent_last_seen("a1", 1234).unwrap();
        let stored = store.get_agent("a1").unwrap().unwrap();
        assert_eq!(
            stored.last_seen, None,
            "a revoked agent must not get last_seen reintroduced"
        );
        assert!(stored.revoked, "a heartbeat must not un-revoke the agent");
    }

    #[test]
    fn touch_advances_last_seen_for_an_active_agent() {
        let store = temp_store();
        store.put_agent(&agent("a1", "loc", None, false)).unwrap();
        store.touch_agent_last_seen("a1", 1234).unwrap();
        assert_eq!(
            store.get_agent("a1").unwrap().unwrap().last_seen,
            Some(1234),
            "active agent liveness advances"
        );
    }

    #[test]
    fn touch_is_a_no_op_for_an_unknown_agent() {
        let store = temp_store();
        // No panic, no phantom row created.
        store.touch_agent_last_seen("ghost", 42).unwrap();
        assert!(store.get_agent("ghost").unwrap().is_none());
    }

    #[test]
    fn revoke_agents_for_location_marks_them_revoked_and_not_seen() {
        let store = temp_store();
        store
            .put_agent(&agent("a1", "loc", Some(1_000), false))
            .unwrap();
        store
            .put_agent(&agent("a2", "loc", Some(1_001), false))
            .unwrap();
        store
            .put_agent(&agent("other", "other-loc", Some(1_002), false))
            .unwrap();

        let revoked = store.revoke_agents_for_location("loc").unwrap();

        assert_eq!(revoked, vec!["a1".to_string(), "a2".to_string()]);
        for id in revoked {
            let stored = store.get_agent(&id).unwrap().unwrap();
            assert!(stored.revoked, "revoked agent {id} must refuse credentials");
            assert_eq!(
                stored.last_seen, None,
                "revoked agent {id} returns to not-enrolled in admin state"
            );
        }
        assert!(
            !store.get_agent("other").unwrap().unwrap().revoked,
            "a sibling location's agent is untouched"
        );
    }

    // The deterministic online→offline transition across the fixed 30s window, at
    // the read boundary: a remote with a fresh beat is online; the same agent 31s
    // later (a controllable clock, no sleep) is offline.
    #[test]
    fn remote_status_flips_offline_after_the_window() {
        let loc = location("loc", NodeKind::Remote, vec![OfferedMethod::Ping]);
        let agents = [agent("a1", "loc", Some(1_000), false)];
        assert_eq!(
            derive_location_status(&loc, &agents, 1_000),
            LocationStatus::Online,
            "a fresh heartbeat is online"
        );
        assert_eq!(
            derive_location_status(&loc, &agents, 1_000 + 31),
            LocationStatus::Offline,
            "31s of silence flips the location offline"
        );
    }

    #[test]
    fn a_local_node_is_always_online_regardless_of_agents() {
        let loc = location("loc", NodeKind::Local, vec![OfferedMethod::Ping]);
        // Even with a stale/empty agent set and any clock, local is online.
        assert_eq!(
            derive_location_status(&loc, &[], 9_999_999),
            LocationStatus::Online
        );
    }

    #[test]
    fn a_remote_with_no_agent_is_offline() {
        let loc = location("loc", NodeKind::Remote, vec![OfferedMethod::Ping]);
        assert_eq!(
            derive_location_status(&loc, &[], 1_000),
            LocationStatus::Offline
        );
    }

    // Resurrection-hole guard (drift.md): a revoked agent with a recent last_seen
    // must derive offline — persisted liveness never resurrects a revoked agent.
    #[test]
    fn a_revoked_agent_derives_offline_despite_a_recent_beat() {
        let loc = location("loc", NodeKind::Remote, vec![OfferedMethod::Ping]);
        let agents = [agent("a1", "loc", Some(1_000), true)];
        assert_eq!(
            derive_location_status(&loc, &agents, 1_000),
            LocationStatus::Offline,
            "a revoked agent is offline even one second after its beat"
        );
    }

    // AC26 (agent-state half): last_seen persists across a central restart, so the
    // location re-derives online from the reopened store — no in-memory liveness.
    #[test]
    fn liveness_survives_a_store_reopen() {
        let path = {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let mut path = std::env::temp_dir();
            path.push(format!(
                "lg-store-restart-{}-{}-{}.redb",
                std::process::id(),
                n,
                nanos
            ));
            path
        };
        {
            let store = Store::open(&path).unwrap();
            store
                .put_location(&location(
                    "loc",
                    NodeKind::Remote,
                    vec![OfferedMethod::Ping],
                ))
                .unwrap();
            store
                .put_agent(&agent("a1", "loc", Some(1_000), false))
                .unwrap();
        } // store dropped — the container "restarts".

        let reopened = Store::open(&path).unwrap();
        let loc = reopened.get_location("loc").unwrap().unwrap();
        let agents = reopened.list_agents("loc").unwrap();
        assert_eq!(agents[0].last_seen, Some(1_000), "last_seen persisted");
        // Within the window from the persisted timestamp → still online after restart.
        assert_eq!(
            derive_location_status(&loc, &agents, 1_000 + 5),
            LocationStatus::Online,
            "the reopened store re-derives online from the persisted last_seen"
        );
    }

    #[test]
    fn revoke_survives_a_store_reopen_and_derives_offline() {
        let path = {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let mut path = std::env::temp_dir();
            path.push(format!(
                "lg-store-revoked-restart-{}-{}-{}.redb",
                std::process::id(),
                n,
                nanos
            ));
            path
        };
        {
            let store = Store::open(&path).unwrap();
            store
                .put_location(&location(
                    "loc",
                    NodeKind::Remote,
                    vec![OfferedMethod::Ping],
                ))
                .unwrap();
            store
                .put_agent(&agent("a1", "loc", Some(1_000), false))
                .unwrap();
            assert_eq!(
                store.revoke_agents_for_location("loc").unwrap(),
                vec!["a1".to_string()]
            );
        }

        let reopened = Store::open(&path).unwrap();
        let loc = reopened.get_location("loc").unwrap().unwrap();
        let agents = reopened.list_agents("loc").unwrap();
        assert!(agents[0].revoked, "revocation persists across restart");
        assert_eq!(agents[0].last_seen, None, "revoked state is not-enrolled");
        assert_eq!(
            derive_location_status(&loc, &agents, 1_005),
            LocationStatus::Offline,
            "recent persisted liveness must not resurrect a revoked agent after restart"
        );
    }

    #[test]
    fn latest_last_seen_picks_the_freshest_agent_beat() {
        let loc = location("loc", NodeKind::Remote, vec![OfferedMethod::Ping]);
        let agents = [
            agent("a1", "loc", Some(500), false),
            agent("a2", "loc", Some(900), false),
        ];
        assert_eq!(latest_last_seen(&loc, &agents), Some(900));
        // A local node reports no last-seen (it has no agent).
        let local = location("loc", NodeKind::Local, vec![]);
        assert_eq!(latest_last_seen(&local, &agents), None);
    }
}
