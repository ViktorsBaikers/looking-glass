//! redb-backed persistence: the single volume file holding the administrator
//! setup state, sessions, global settings, and the location catalogue (locations
//! and their test IPs / iperf endpoints / test files, plus the agents and
//! enrollment tokens a location owns). All state the container needs to survive a
//! restart lives here. redb has no SQL, so relations and the location cascade are
//! hand-rolled Rust over serde-encoded rows keyed by id (the redb-hold decision).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rand_core::{OsRng, RngCore};
use redb::{
    Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, Table, TableDefinition,
    TableHandle, WriteTransaction,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use shared::liveness::is_online;
use shared::protocol::CertificateStatus;
use shared::template::Method;
use shared::validate::PrefixFamily;

pub(crate) const ADMINISTRATOR: TableDefinition<&str, &[u8]> =
    TableDefinition::new("administrator");
/// The pre-Administrators single-row admin table (spec #1 legacy migration).
/// Read once at open, migrated into [`ADMINISTRATOR`], then deleted.
const LEGACY_ADMIN_TABLE_NAME: &str = "admin";
const LEGACY_ADMIN: TableDefinition<&str, &[u8]> = TableDefinition::new(LEGACY_ADMIN_TABLE_NAME);
pub(crate) const SETUP: TableDefinition<&str, &[u8]> = TableDefinition::new("setup");
pub(crate) const SESSION: TableDefinition<&str, &[u8]> = TableDefinition::new("session");
const SESSION_COOKIE_KEY_TABLE_NAME: &str = "session_cookie_key";
pub(crate) const SESSION_COOKIE_KEY: TableDefinition<&str, &[u8]> =
    TableDefinition::new(SESSION_COOKIE_KEY_TABLE_NAME);
pub(crate) const SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");
pub(crate) const LOCATION: TableDefinition<&str, &[u8]> = TableDefinition::new("location");
/// One row (`ORDER_KEY`) holding the admin-chosen location order as a list of ids.
/// Ids absent from it (older volumes, fresh creates) sort after it by `created_at`.
pub(crate) const LOCATION_ORDER: TableDefinition<&str, &[u8]> =
    TableDefinition::new("location_order");
pub(crate) const TEST_IP: TableDefinition<&str, &[u8]> = TableDefinition::new("test_ip");
pub(crate) const IPERF: TableDefinition<&str, &[u8]> = TableDefinition::new("iperf_endpoint");
pub(crate) const TEST_FILE: TableDefinition<&str, &[u8]> = TableDefinition::new("test_file");
pub(crate) const AGENT: TableDefinition<&str, &[u8]> = TableDefinition::new("agent");
pub(crate) const ENROLLMENT_TOKEN: TableDefinition<&str, &[u8]> =
    TableDefinition::new("enrollment_token");
/// A remote location's data-plane certificate status, keyed by location id, as
/// its agent last reported it over the tunnel.
pub(crate) const CERTIFICATE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("certificate_status");

/// A [`CERTIFICATE`] row: the status and the data-plane origin its location had
/// when it was recorded. A row written before the origin was kept reads as no
/// origin.
#[derive(Serialize, Deserialize)]
struct CertificateRecord {
    #[serde(default)]
    origin: Option<String>,
    #[serde(flatten)]
    status: CertificateStatus,
}

const LEGACY_ADMIN_KEY: &str = "admin";
const SETUP_KEY: &str = "state";
const SETTINGS_KEY: &str = "global";
const ORDER_KEY: &str = "ids";
const SESSION_COOKIE_KEY_ID: &str = "signing";
const SESSION_COOKIE_KEY_LEN: usize = 64;

/// Whether an administrator can sign in (`Active`) or still owes a password set
/// through their activation link (`Pending`, ADR-0001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdministratorStatus {
    Active,
    Pending,
}

/// One equal-peer administrator (ADR-0001). `password_hash` is `None` while
/// pending — the password is set at activation, never by the creating peer. The
/// activation token itself is never stored: only its SHA-256 hash and absolute
/// expiry, and regenerating replaces both, which is what invalidates the old link.
/// `session_generation` is the credential epoch: every password rotation bumps
/// it inside the same write transaction, and the session extractor refuses any
/// session stamped with an older value — so a session record resurrected by an
/// overlapping request cannot survive the rotation. The serde default keeps
/// rows written before the field (and the sessions minted from them) valid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Administrator {
    pub id: String,
    pub username: String,
    pub password_hash: Option<String>,
    pub status: AdministratorStatus,
    pub created_at: u64,
    pub activation_token_hash: Option<String>,
    pub activation_expires_at: Option<u64>,
    #[serde(default)]
    pub session_generation: u64,
}

/// The legacy single-admin row shape, decoded only by the open-time migration.
#[derive(Deserialize)]
struct LegacyAdmin {
    id: String,
    username: String,
    password_hash: String,
    created_at: u64,
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

/// Upper bounds of the exec settings (each lower bound is 1). The `LG_EXEC_*`
/// clamps below and the admin settings validation (`SettingsInput` in
/// `admin_api`) both read these, so the two ranges cannot drift apart.
pub(crate) const EXEC_MAX_CONCURRENT_MAX: usize = 1024;
pub(crate) const EXEC_TIMEOUT_SECS_MAX: u64 = 3600;
pub(crate) const EXEC_MAX_OUTPUT_KIB_MAX: usize = 1_048_576;
pub(crate) const EXEC_RATE_MAX_MAX: u32 = 100_000;
pub(crate) const EXEC_RATE_WINDOW_SECS_MAX: u64 = 86_400;

/// Read an `LG_EXEC_*` default, clamped to the range the admin settings API
/// accepts (`SettingsInput` in `admin_api`) so an env value can neither run live
/// out of bounds nor make every later settings save fail validation.
fn env_or<T: std::str::FromStr + Ord + Copy + std::fmt::Debug>(
    key: &str,
    fallback: T,
    range: std::ops::RangeInclusive<T>,
) -> T {
    let Some(value) = std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse::<T>().ok())
    else {
        return fallback;
    };
    let clamped = value.clamp(*range.start(), *range.end());
    if clamped != value {
        tracing::warn!("{key}={value:?} is outside {range:?}; using {clamped:?}");
    }
    clamped
}

fn default_site_title() -> String {
    "Looking Glass".to_string()
}
fn default_exec_max_concurrent() -> usize {
    env_or("LG_EXEC_MAX_CONCURRENT", 8, 1..=EXEC_MAX_CONCURRENT_MAX)
}
fn default_exec_timeout_secs() -> u64 {
    env_or("LG_EXEC_TIMEOUT_SECS", 30, 1..=EXEC_TIMEOUT_SECS_MAX)
}
fn default_exec_max_output_kib() -> usize {
    env_or("LG_EXEC_MAX_OUTPUT_KIB", 256, 1..=EXEC_MAX_OUTPUT_KIB_MAX)
}
fn default_exec_rate_max() -> u32 {
    env_or("LG_EXEC_RATE_MAX", 20, 1..=EXEC_RATE_MAX_MAX)
}
fn default_exec_rate_window_secs() -> u64 {
    env_or(
        "LG_EXEC_RATE_WINDOW_SECS",
        60,
        1..=EXEC_RATE_WINDOW_SECS_MAX,
    )
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
    /// Optional ASN (spec #1): 1–4294967295, absent for rows saved before the
    /// redesign (`#[serde(default)]` keeps old volumes readable).
    #[serde(default)]
    pub asn: Option<u32>,
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
    UsernameTaken,
    Backend(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::AlreadyInstalled => f.write_str("setup already completed"),
            StoreError::UsernameTaken => f.write_str("username already taken"),
            StoreError::Backend(msg) => write!(f, "store backend error: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

fn backend<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Backend(e.to_string())
}

/// Why [`Store::remove_administrator`] refused — the spec #1 guard rails,
/// checked inside the delete transaction itself.
#[derive(Debug)]
pub enum RemoveAdministratorError {
    NotFound,
    CannotRemoveSelf,
    LastActive,
    Backend(StoreError),
}

impl From<StoreError> for RemoveAdministratorError {
    fn from(error: StoreError) -> Self {
        Self::Backend(error)
    }
}

/// Open (creating if absent) the store file owner-only: it holds the
/// session-cookie signing key and live session ids. A missing data dir is created
/// 0700; an existing one (e.g. an operator's volume mount) keeps its mode. A file an
/// older release left loose is tightened to 0600, failing closed like the
/// setup-token file. redb keeps its lock on this file, with no sidecar files.
fn open_owner_only(path: &Path) -> std::io::Result<std::fs::File> {
    let mut dir = std::fs::DirBuilder::new();
    let mut file = std::fs::OpenOptions::new();
    // The same flags redb's `Database::create` opens with.
    file.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        dir.mode(0o700);
        file.mode(0o600);
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        dir.recursive(true).create(parent)?;
    }
    let file = file.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
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
        let file = open_owner_only(path.as_ref()).map_err(backend)?;
        let db = Arc::new(Database::builder().create_file(file).map_err(backend)?);
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
        off_workers(|| {
            let mut txn = db.begin_write().map_err(backend)?;
            txn.open_table(ADMINISTRATOR).map_err(backend)?;
            txn.open_table(SETUP).map_err(backend)?;
            txn.open_table(SESSION).map_err(backend)?;
            txn.open_table(LOCATION).map_err(backend)?;
            txn.open_table(LOCATION_ORDER).map_err(backend)?;
            txn.open_table(TEST_IP).map_err(backend)?;
            txn.open_table(IPERF).map_err(backend)?;
            txn.open_table(TEST_FILE).map_err(backend)?;
            txn.open_table(AGENT).map_err(backend)?;
            txn.open_table(ENROLLMENT_TOKEN).map_err(backend)?;
            txn.open_table(CERTIFICATE).map_err(backend)?;
            Self::migrate_legacy_single_admin(&mut txn)?;
            {
                let mut settings = txn.open_table(SETTINGS).map_err(backend)?;
                if settings.get(SETTINGS_KEY).map_err(backend)?.is_none() {
                    let encoded =
                        serde_json::to_vec(&GlobalSettings::default()).map_err(backend)?;
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
                                StoreError::Backend(
                                    "invalid session cookie signing key".to_string(),
                                )
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
                                "OS randomness unavailable for session cookie signing key"
                                    .to_string(),
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
        })
    }

    /// Legacy single-admin volume migration (spec #1): a pre-Administrators volume
    /// holds one row keyed `"admin"` in an `admin` table. When that table exists its
    /// row becomes an equivalent active [`Administrator`] (id, username, password
    /// hash, and created-at preserved) and the legacy table is dropped, so a second
    /// open finds nothing to migrate. Runs inside the bootstrap write transaction —
    /// no reader ever observes a half-migrated volume.
    fn migrate_legacy_single_admin(txn: &mut WriteTransaction) -> Result<(), StoreError> {
        let legacy_present = txn
            .list_tables()
            .map_err(backend)?
            .any(|table| table.name() == LEGACY_ADMIN_TABLE_NAME);
        if !legacy_present {
            return Ok(());
        }
        let legacy: Option<LegacyAdmin> = {
            let table = txn.open_table(LEGACY_ADMIN).map_err(backend)?;
            let row = table.get(LEGACY_ADMIN_KEY).map_err(backend)?;
            match row {
                Some(guard) => Some(serde_json::from_slice(guard.value()).map_err(backend)?),
                None => None,
            }
        };
        if let Some(legacy) = legacy {
            let mut admins = txn.open_table(ADMINISTRATOR).map_err(backend)?;
            if admins.get(legacy.id.as_str()).map_err(backend)?.is_none() {
                let admin = Administrator {
                    id: legacy.id,
                    username: legacy.username,
                    password_hash: Some(legacy.password_hash),
                    status: AdministratorStatus::Active,
                    created_at: legacy.created_at,
                    activation_token_hash: None,
                    activation_expires_at: None,
                    session_generation: 0,
                };
                let encoded = serde_json::to_vec(&admin).map_err(backend)?;
                admins
                    .insert(admin.id.as_str(), encoded.as_slice())
                    .map_err(backend)?;
            }
        }
        txn.delete_table(LEGACY_ADMIN).map_err(backend)?;
        Ok(())
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

    pub fn list_administrators(&self) -> Result<Vec<Administrator>, StoreError> {
        let mut admins: Vec<Administrator> = self.read_all(ADMINISTRATOR)?;
        admins.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(admins)
    }

    pub fn get_administrator(&self, id: &str) -> Result<Option<Administrator>, StoreError> {
        self.read_record(ADMINISTRATOR, id)
    }

    pub fn put_administrator(&self, admin: &Administrator) -> Result<(), StoreError> {
        self.write_record(ADMINISTRATOR, &admin.id, admin)
    }

    /// The (at most one) sign-in-capable administrator with this exact username —
    /// the login lookup. Pending peers never match: they have no password yet.
    pub fn find_active_administrator_by_username(
        &self,
        username: &str,
    ) -> Result<Option<Administrator>, StoreError> {
        Ok(self
            .read_all::<Administrator>(ADMINISTRATOR)?
            .into_iter()
            .find(|admin| {
                admin.status == AdministratorStatus::Active && admin.username == username
            }))
    }

    /// The (at most one) pending administrator whose current activation token
    /// hashes to `token_hash` — a full scan, the accepted redb-hold cost. Expiry
    /// is judged by the caller against the returned row.
    pub fn find_pending_by_activation_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<Administrator>, StoreError> {
        Ok(self
            .read_all::<Administrator>(ADMINISTRATOR)?
            .into_iter()
            .find(|admin| {
                admin.status == AdministratorStatus::Pending
                    && admin.activation_token_hash.as_deref() == Some(token_hash)
            }))
    }

    pub fn settings(&self) -> Result<GlobalSettings, StoreError> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(SETTINGS).map_err(backend)?;
        match table.get(SETTINGS_KEY).map_err(backend)? {
            Some(guard) => Ok(serde_json::from_slice(guard.value()).map_err(backend)?),
            None => Ok(GlobalSettings::default()),
        }
    }

    /// Create the first administrator and mark setup complete in a single write
    /// transaction. redb serializes writers, so a concurrent second call sees the
    /// existing administrator and returns `AlreadyInstalled` — the closed-installer
    /// guarantee is atomic here.
    pub fn create_first_administrator(
        &self,
        id: String,
        username: String,
        password_hash: String,
    ) -> Result<Administrator, StoreError> {
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            let admin = {
                let mut admins = txn.open_table(ADMINISTRATOR).map_err(backend)?;
                if admins.len().map_err(backend)? > 0 {
                    return Err(StoreError::AlreadyInstalled);
                }
                let admin = Administrator {
                    id,
                    username,
                    password_hash: Some(password_hash),
                    status: AdministratorStatus::Active,
                    created_at: unix_now(),
                    activation_token_hash: None,
                    activation_expires_at: None,
                    session_generation: 0,
                };
                let encoded = serde_json::to_vec(&admin).map_err(backend)?;
                admins
                    .insert(admin.id.as_str(), encoded.as_slice())
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
        })
    }

    /// Insert a new pending administrator, refusing a username already taken
    /// case-insensitively (spec #1). The uniqueness scan and the insert share one
    /// write transaction, so two racing creates cannot both claim a name.
    pub fn create_pending_administrator(
        &self,
        admin: Administrator,
    ) -> Result<Administrator, StoreError> {
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            {
                let mut admins = txn.open_table(ADMINISTRATOR).map_err(backend)?;
                for entry in admins.iter().map_err(backend)? {
                    let (_key, value) = entry.map_err(backend)?;
                    let existing: Administrator =
                        serde_json::from_slice(value.value()).map_err(backend)?;
                    if existing.username.eq_ignore_ascii_case(&admin.username) {
                        return Err(StoreError::UsernameTaken);
                    }
                }
                let encoded = serde_json::to_vec(&admin).map_err(backend)?;
                admins
                    .insert(admin.id.as_str(), encoded.as_slice())
                    .map_err(backend)?;
            }
            txn.commit().map_err(backend)?;
            Ok(admin)
        })
    }

    /// Replace a pending administrator's activation token+expiry in one write
    /// transaction — replacing the stored hash is what invalidates the previous
    /// link. `Ok(None)` when the row is absent or no longer pending, so a
    /// regeneration racing an activation can never hand out a fresh link for an
    /// account that already has a password.
    pub fn regenerate_activation(
        &self,
        id: &str,
        token_hash: String,
        expires_at: u64,
    ) -> Result<Option<Administrator>, StoreError> {
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            let updated = {
                let mut admins = txn.open_table(ADMINISTRATOR).map_err(backend)?;
                let current: Option<Administrator> = match admins.get(id).map_err(backend)? {
                    Some(guard) => Some(serde_json::from_slice(guard.value()).map_err(backend)?),
                    None => None,
                };
                match current.filter(|admin| admin.status == AdministratorStatus::Pending) {
                    Some(mut admin) => {
                        admin.activation_token_hash = Some(token_hash);
                        admin.activation_expires_at = Some(expires_at);
                        let encoded = serde_json::to_vec(&admin).map_err(backend)?;
                        admins.insert(id, encoded.as_slice()).map_err(backend)?;
                        Some(admin)
                    }
                    None => None,
                }
            };
            txn.commit().map_err(backend)?;
            Ok(updated)
        })
    }

    /// Atomically consume an activation link: in one write transaction, re-read
    /// the administrator and activate only if still pending, the stored hash
    /// still matches, and the expiry has not passed. Returns `true` when this
    /// call is the one that activated — two racing activations can never both
    /// succeed on one single-use token (the enrollment-token pattern).
    pub fn activate_administrator(
        &self,
        id: &str,
        token_hash: &str,
        password_hash: String,
        now: u64,
    ) -> Result<bool, StoreError> {
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            let activated = {
                let mut admins = txn.open_table(ADMINISTRATOR).map_err(backend)?;
                let current: Option<Administrator> = match admins.get(id).map_err(backend)? {
                    Some(guard) => Some(serde_json::from_slice(guard.value()).map_err(backend)?),
                    None => None,
                };
                let eligible = current.filter(|admin| {
                    admin.status == AdministratorStatus::Pending
                        && admin.activation_token_hash.as_deref() == Some(token_hash)
                        && admin
                            .activation_expires_at
                            .is_some_and(|expiry| now <= expiry)
                });
                match eligible {
                    Some(mut admin) => {
                        admin.password_hash = Some(password_hash);
                        admin.status = AdministratorStatus::Active;
                        admin.activation_token_hash = None;
                        admin.activation_expires_at = None;
                        let encoded = serde_json::to_vec(&admin).map_err(backend)?;
                        admins.insert(id, encoded.as_slice()).map_err(backend)?;
                        true
                    }
                    None => false,
                }
            };
            txn.commit().map_err(backend)?;
            Ok(activated)
        })
    }

    /// Remove one administrator under the spec #1 guard rails — enforced inside
    /// the same redb write transaction as the delete, so two peers concurrently
    /// removing each other can never both pass the last-active check (redb
    /// serializes writers; the loser re-reads the winner's committed table).
    pub fn remove_administrator(
        &self,
        caller_id: &str,
        target_id: &str,
    ) -> Result<(), RemoveAdministratorError> {
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            {
                let mut admins = txn.open_table(ADMINISTRATOR).map_err(backend)?;
                let target: Administrator = match admins.get(target_id).map_err(backend)? {
                    Some(guard) => serde_json::from_slice(guard.value()).map_err(backend)?,
                    None => return Err(RemoveAdministratorError::NotFound),
                };
                if target.id == caller_id {
                    return Err(RemoveAdministratorError::CannotRemoveSelf);
                }
                if target.status == AdministratorStatus::Active {
                    let mut active = 0usize;
                    for entry in admins.iter().map_err(backend)? {
                        let (_key, value) = entry.map_err(backend)?;
                        let admin: Administrator =
                            serde_json::from_slice(value.value()).map_err(backend)?;
                        if admin.status == AdministratorStatus::Active {
                            active += 1;
                        }
                    }
                    if active <= 1 {
                        return Err(RemoveAdministratorError::LastActive);
                    }
                }
                admins.remove(target_id).map_err(backend)?;
            }
            txn.commit().map_err(backend)?;
            Ok(())
        })
    }

    /// Conditional password rotation: one write transaction that requires the
    /// row to still exist, still be active, and still carry the exact hash the
    /// handler verified against — so a removal (or a second rotation) racing
    /// the Argon2 work can never be undone by this write, and the row is never
    /// re-created. On success stores `new_hash`, bumps `session_generation`
    /// (revoking every session stamped with an older value), and returns the
    /// new generation; `Ok(None)` when any precondition broke.
    pub fn rotate_password(
        &self,
        id: &str,
        expected_hash: &str,
        new_hash: String,
    ) -> Result<Option<u64>, StoreError> {
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            let generation = {
                let mut admins = txn.open_table(ADMINISTRATOR).map_err(backend)?;
                let current: Option<Administrator> = match admins.get(id).map_err(backend)? {
                    Some(guard) => Some(serde_json::from_slice(guard.value()).map_err(backend)?),
                    None => None,
                };
                let eligible = current.filter(|admin| {
                    admin.status == AdministratorStatus::Active
                        && admin.password_hash.as_deref() == Some(expected_hash)
                });
                match eligible {
                    Some(mut admin) => {
                        admin.password_hash = Some(new_hash);
                        admin.session_generation += 1;
                        let encoded = serde_json::to_vec(&admin).map_err(backend)?;
                        admins.insert(id, encoded.as_slice()).map_err(backend)?;
                        Some(admin.session_generation)
                    }
                    None => None,
                }
            };
            txn.commit().map_err(backend)?;
            Ok(generation)
        })
    }

    pub(crate) fn persist_settings(&self, settings: &GlobalSettings) -> Result<(), StoreError> {
        self.write_record(SETTINGS, SETTINGS_KEY, settings)
    }

    /// Every location in the admin-chosen order (the public tab order).
    pub fn list_locations(&self) -> Result<Vec<Location>, StoreError> {
        let mut locations: Vec<Location> = self.read_all(LOCATION)?;
        let order: Vec<String> = self
            .read_record(LOCATION_ORDER, ORDER_KEY)?
            .unwrap_or_default();
        let rank: HashMap<&str, usize> = order
            .iter()
            .enumerate()
            .map(|(index, id)| (id.as_str(), index))
            .collect();
        locations.sort_by_key(|location| {
            (
                rank.get(location.id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX),
                location.created_at,
            )
        });
        Ok(locations)
    }

    /// Persist a new location order. `ids` must name every current location
    /// exactly once; otherwise (a stale client, a concurrent create or delete)
    /// nothing is written and `false` comes back.
    pub fn reorder_locations(&self, ids: &[String]) -> Result<bool, StoreError> {
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            {
                let locations = txn.open_table(LOCATION).map_err(backend)?;
                let mut current = HashSet::new();
                for entry in locations.iter().map_err(backend)? {
                    current.insert(entry.map_err(backend)?.0.value().to_string());
                }
                let requested: HashSet<&str> = ids.iter().map(String::as_str).collect();
                if requested.len() != ids.len()
                    || requested.len() != current.len()
                    || !current.iter().all(|id| requested.contains(id.as_str()))
                {
                    return Ok(false);
                }
                let encoded = serde_json::to_vec(ids).map_err(backend)?;
                txn.open_table(LOCATION_ORDER)
                    .map_err(backend)?
                    .insert(ORDER_KEY, encoded.as_slice())
                    .map_err(backend)?;
            }
            txn.commit().map_err(backend)?;
            Ok(true)
        })
    }

    pub fn get_location(&self, id: &str) -> Result<Option<Location>, StoreError> {
        self.read_record(LOCATION, id)
    }

    /// Insert a location by id (a create: fresh id + `unix_now`).
    pub fn put_location(&self, location: &Location) -> Result<(), StoreError> {
        self.write_record(LOCATION, &location.id, location)
    }

    /// Replace a location only while it still exists, so an edit racing a
    /// delete never resurrects it. `None` when it is gone; nothing is written.
    /// A local location has no agent, so saving one as local revokes its agents
    /// and drops its enrollment tokens in the same transaction; the revoked ids
    /// come back so the caller can kick live tunnels.
    pub fn update_location(&self, location: &Location) -> Result<Option<Vec<String>>, StoreError> {
        let encoded = serde_json::to_vec(location).map_err(backend)?;
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            {
                let mut locations = txn.open_table(LOCATION).map_err(backend)?;
                if locations
                    .get(location.id.as_str())
                    .map_err(backend)?
                    .is_none()
                {
                    return Ok(None);
                }
                locations
                    .insert(location.id.as_str(), encoded.as_slice())
                    .map_err(backend)?;
            }
            let revoked = if location.kind == NodeKind::Local {
                revoke_agents_and_tokens(&txn, &location.id)?
            } else {
                Vec::new()
            };
            txn.commit().map_err(backend)?;
            Ok(Some(revoked))
        })
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
        off_workers(|| {
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
                txn.open_table(CERTIFICATE)
                    .map_err(backend)?
                    .remove(id)
                    .map_err(backend)?;
                (existed, agent_ids)
            };
            txn.commit().map_err(backend)?;
            Ok(DeletedLocation { existed, agent_ids })
        })
    }

    pub fn list_test_ips(&self, location_id: &str) -> Result<Vec<TestIp>, StoreError> {
        self.read_children(TEST_IP, location_id)
    }
    pub fn get_test_ip(&self, id: &str) -> Result<Option<TestIp>, StoreError> {
        self.read_record(TEST_IP, id)
    }
    /// Upsert only while the parent location exists; `false` writes nothing.
    pub fn put_test_ip(&self, test_ip: &TestIp) -> Result<bool, StoreError> {
        self.write_if_location(TEST_IP, &test_ip.id, test_ip, &test_ip.location_id, false)
    }
    /// Replace only while both the row and its location exist, so an edit
    /// racing a delete never brings the row back. `false` writes nothing.
    pub fn update_test_ip(&self, test_ip: &TestIp) -> Result<bool, StoreError> {
        self.write_if_location(TEST_IP, &test_ip.id, test_ip, &test_ip.location_id, true)
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
    /// Upsert only while the parent location exists; `false` writes nothing.
    pub fn put_iperf(&self, endpoint: &IperfEndpoint) -> Result<bool, StoreError> {
        self.write_if_location(IPERF, &endpoint.id, endpoint, &endpoint.location_id, false)
    }
    /// Replace only while both the row and its location exist, so an edit
    /// racing a delete never brings the row back. `false` writes nothing.
    pub fn update_iperf(&self, endpoint: &IperfEndpoint) -> Result<bool, StoreError> {
        self.write_if_location(IPERF, &endpoint.id, endpoint, &endpoint.location_id, true)
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
    /// Upsert only while the parent location exists; `false` writes nothing.
    pub fn put_test_file(&self, file: &TestFile) -> Result<bool, StoreError> {
        self.write_if_location(TEST_FILE, &file.id, file, &file.location_id, false)
    }
    /// Replace only while both the row and its location exist, so an edit
    /// racing a delete never brings the row back. `false` writes nothing.
    pub fn update_test_file(&self, file: &TestFile) -> Result<bool, StoreError> {
        self.write_if_location(TEST_FILE, &file.id, file, &file.location_id, true)
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

    /// Store a freshly minted token in one write transaction that also drops
    /// every other token for its location (Regenerate supersedes the old
    /// install command) and every expired or used token of any location.
    pub fn mint_enrollment_token(
        &self,
        token: &EnrollmentToken,
        now: u64,
    ) -> Result<(), StoreError> {
        let encoded = serde_json::to_vec(token).map_err(backend)?;
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            {
                let mut table = txn.open_table(ENROLLMENT_TOKEN).map_err(backend)?;
                let mut stale = Vec::new();
                for entry in table.iter().map_err(backend)? {
                    let (key, value) = entry.map_err(backend)?;
                    let old: EnrollmentToken =
                        serde_json::from_slice(value.value()).map_err(backend)?;
                    if old.location_id == token.location_id
                        || old.used_at.is_some()
                        || now > old.expires_at
                    {
                        stale.push(key.value().to_string());
                    }
                }
                for id in &stale {
                    table.remove(id.as_str()).map_err(backend)?;
                }
                table
                    .insert(token.id.as_str(), encoded.as_slice())
                    .map_err(backend)?;
            }
            txn.commit().map_err(backend)
        })
    }

    /// Redeem a token in one write transaction: it must still exist unused and
    /// its location must still exist; then the token is marked used and `agent`
    /// is written. Returns `false` and writes nothing otherwise, so a racing
    /// redemption, Regenerate, Revoke or location delete can never leave a
    /// second or orphaned agent (single-use, TOCTOU-free).
    pub fn redeem_enrollment_token(
        &self,
        token_id: &str,
        used_at: u64,
        agent: &Agent,
    ) -> Result<bool, StoreError> {
        let encoded_agent = serde_json::to_vec(agent).map_err(backend)?;
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            {
                let mut tokens = txn.open_table(ENROLLMENT_TOKEN).map_err(backend)?;
                let current: Option<EnrollmentToken> = match tokens
                    .get(token_id)
                    .map_err(backend)?
                {
                    Some(guard) => Some(serde_json::from_slice(guard.value()).map_err(backend)?),
                    None => None,
                };
                let Some(mut token) = current.filter(|token| {
                    token.used_at.is_none() && token.location_id == agent.location_id
                }) else {
                    return Ok(false);
                };
                // Only a remote location has an agent.
                let locations = txn.open_table(LOCATION).map_err(backend)?;
                let remote = match locations.get(agent.location_id.as_str()).map_err(backend)? {
                    Some(guard) => {
                        serde_json::from_slice::<Location>(guard.value())
                            .map_err(backend)?
                            .kind
                            == NodeKind::Remote
                    }
                    None => false,
                };
                if !remote {
                    return Ok(false);
                }
                token.used_at = Some(used_at);
                let encoded = serde_json::to_vec(&token).map_err(backend)?;
                tokens
                    .insert(token_id, encoded.as_slice())
                    .map_err(backend)?;
                txn.open_table(AGENT)
                    .map_err(backend)?
                    .insert(agent.id.as_str(), encoded_agent.as_slice())
                    .map_err(backend)?;
            }
            txn.commit().map_err(backend)?;
            Ok(true)
        })
    }

    /// Record `location_id`'s data-plane certificate status for `origin`, only
    /// while the location exists and still has that origin, checked in the same
    /// write transaction: a report racing a delete never leaves an orphan row,
    /// and one racing an origin edit is dropped. `false` writes nothing.
    pub fn put_certificate_status(
        &self,
        location_id: &str,
        origin: &str,
        status: &CertificateStatus,
    ) -> Result<bool, StoreError> {
        let encoded = serde_json::to_vec(&CertificateRecord {
            origin: Some(origin.to_string()),
            status: status.clone(),
        })
        .map_err(backend)?;
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            {
                let locations = txn.open_table(LOCATION).map_err(backend)?;
                let Some(location) = locations.get(location_id).map_err(backend)? else {
                    return Ok(false);
                };
                let location: Location =
                    serde_json::from_slice(location.value()).map_err(backend)?;
                if location.data_plane_origin.as_deref() != Some(origin) {
                    return Ok(false);
                }
            }
            txn.open_table(CERTIFICATE)
                .map_err(backend)?
                .insert(location_id, encoded.as_slice())
                .map_err(backend)?;
            txn.commit().map_err(backend)?;
            Ok(true)
        })
    }

    /// The certificate status recorded for the location's current origin; one
    /// recorded for an origin since changed or cleared is not current.
    pub fn get_certificate_status(
        &self,
        location_id: &str,
    ) -> Result<Option<CertificateStatus>, StoreError> {
        let Some(location) = self.get_location(location_id)? else {
            return Ok(None);
        };
        let record: Option<CertificateRecord> = self.read_record(CERTIFICATE, location_id)?;
        Ok(record
            .filter(|record| record.origin == location.data_plane_origin)
            .map(|record| record.status))
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
    /// clearing `last_seen` so the admin state returns to not-enrolled, and drop
    /// the location's outstanding enrollment tokens so no earlier install command
    /// re-enrolls it. Returns the revoked agent ids so the caller can kick live
    /// tunnels for the same agents.
    pub fn revoke_agents_for_location(&self, location_id: &str) -> Result<Vec<String>, StoreError> {
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            let revoked = revoke_agents_and_tokens(&txn, location_id)?;
            txn.commit().map_err(backend)?;
            Ok(revoked)
        })
    }

    /// Record an agent's proof-of-life (a received tunnel heartbeat), advancing its
    /// `last_seen` to `ts`. See [`Self::touch_agents_last_seen`] for the rules.
    pub fn touch_agent_last_seen(&self, id: &str, ts: u64) -> Result<(), StoreError> {
        self.touch_agents_last_seen(&[(id.to_string(), ts)])
    }

    /// Record proof-of-life for many agents in one transaction: every commit
    /// fsyncs, so the tunnel coalesces a burst of heartbeats into one write here.
    /// A read-modify-write per agent so it preserves `revoked` — recording
    /// liveness can never un-revoke an agent, and `is_online` still derives a
    /// revoked agent offline. Unknown/absent agents are skipped. `last_seen` only
    /// moves forward, so a stale timestamp never rolls a fresher one back.
    pub fn touch_agents_last_seen(&self, beats: &[(String, u64)]) -> Result<(), StoreError> {
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            {
                let mut table = txn.open_table(AGENT).map_err(backend)?;
                for (id, ts) in beats {
                    let current: Option<Agent> = match table.get(id.as_str()).map_err(backend)? {
                        Some(guard) => {
                            Some(serde_json::from_slice(guard.value()).map_err(backend)?)
                        }
                        None => None,
                    };
                    if let Some(mut agent) = current
                        .filter(|agent| !agent.revoked)
                        .filter(|agent| agent.last_seen.is_none_or(|seen| *ts > seen))
                    {
                        agent.last_seen = Some(*ts);
                        let encoded = serde_json::to_vec(&agent).map_err(backend)?;
                        table
                            .insert(id.as_str(), encoded.as_slice())
                            .map_err(backend)?;
                    }
                }
            }
            txn.commit().map_err(backend)?;
            Ok(())
        })
    }

    fn write_record<T: Serialize>(
        &self,
        def: TableDefinition<&str, &[u8]>,
        key: &str,
        value: &T,
    ) -> Result<(), StoreError> {
        let encoded = serde_json::to_vec(value).map_err(backend)?;
        let write = || {
            let txn = self.db.begin_write().map_err(backend)?;
            {
                let mut table = txn.open_table(def).map_err(backend)?;
                table.insert(key, encoded.as_slice()).map_err(backend)?;
            }
            txn.commit().map_err(backend)
        };
        off_workers(write)
    }

    /// Upsert `value` only if location `location_id` exists, checked inside the
    /// same write transaction, so a concurrent location delete is never undone
    /// (no resurrected location, no orphaned child). With `must_exist`, `key`
    /// must also still be in `def`, so a concurrent row delete is never undone
    /// either. `false` writes nothing.
    fn write_if_location<T: Serialize>(
        &self,
        def: TableDefinition<&str, &[u8]>,
        key: &str,
        value: &T,
        location_id: &str,
        must_exist: bool,
    ) -> Result<bool, StoreError> {
        let encoded = serde_json::to_vec(value).map_err(backend)?;
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            {
                let locations = txn.open_table(LOCATION).map_err(backend)?;
                if locations.get(location_id).map_err(backend)?.is_none() {
                    return Ok(false);
                }
            }
            {
                let mut table = txn.open_table(def).map_err(backend)?;
                if must_exist && table.get(key).map_err(backend)?.is_none() {
                    return Ok(false);
                }
                table.insert(key, encoded.as_slice()).map_err(backend)?;
            }
            txn.commit().map_err(backend)?;
            Ok(true)
        })
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
        off_workers(|| {
            let txn = self.db.begin_write().map_err(backend)?;
            let existed = {
                let mut table = txn.open_table(def).map_err(backend)?;
                let removed = table.remove(key).map_err(backend)?.is_some();
                removed
            };
            txn.commit().map_err(backend)?;
            Ok(existed)
        })
    }
}

/// Run a blocking redb write. `begin_write` waits out any other writer and
/// `commit` fsyncs. The callers are sync, so hand this async worker's queue to
/// another thread for the duration instead of `spawn_blocking`. A
/// current-thread runtime cannot hand off (it panics), so it runs inline.
fn off_workers<R>(write: impl FnOnce() -> R) -> R {
    #[cfg(test)]
    tests::before_write();
    let multi_thread = tokio::runtime::Handle::try_current().is_ok_and(|runtime| {
        runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
    });
    if multi_thread {
        tokio::task::block_in_place(write)
    } else {
        write()
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
/// Mark every agent of `location_id` revoked (clearing `last_seen`) and drop the
/// location's enrollment tokens inside `txn`. Returns the revoked agent ids.
fn revoke_agents_and_tokens(
    txn: &WriteTransaction,
    location_id: &str,
) -> Result<Vec<String>, StoreError> {
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
    purge_children(
        &mut txn.open_table(ENROLLMENT_TOKEN).map_err(backend)?,
        location_id,
    )?;
    Ok(revoked)
}

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
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    type Hook = Box<dyn Fn()>;

    thread_local! {
        /// Runs before each write transaction this thread starts, so a test
        /// can see the committed state between two writes of one operation.
        /// Thread-local, so parallel tests never see each other's writes.
        static BEFORE_WRITE: std::cell::RefCell<Option<Hook>> = const { std::cell::RefCell::new(None) };
    }

    pub(super) fn before_write() {
        // Taken while it runs, so the hook's own writes do not re-enter it.
        let hook = BEFORE_WRITE.take();
        if let Some(hook) = &hook {
            hook();
        }
        BEFORE_WRITE.set(hook);
    }

    /// Install this thread's [`before_write`] hook from another module's tests.
    pub(crate) fn set_before_write(hook: impl Fn() + 'static) {
        BEFORE_WRITE.set(Some(Box::new(hook)));
    }

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
            asn: None,
            offered_methods: offered,
            status: LocationStatus::Online,
            created_at: 0,
        }
    }

    // An LG_EXEC_* value outside the admin API's bounds is clamped at
    // load, so it can neither run live nor make every later settings save 422.
    #[test]
    fn exec_env_values_are_clamped_to_the_admin_bounds() {
        std::env::set_var("LG_TEST_T004_HIGH", "7200");
        std::env::set_var("LG_TEST_T004_ZERO", "0");
        std::env::set_var("LG_TEST_T004_OK", "45");
        assert_eq!(env_or("LG_TEST_T004_HIGH", 30u64, 1..=3600), 3600);
        assert_eq!(env_or("LG_TEST_T004_ZERO", 8usize, 1..=1024), 1);
        assert_eq!(env_or("LG_TEST_T004_OK", 30u64, 1..=3600), 45);
        assert_eq!(env_or("LG_TEST_T004_UNSET", 30u64, 1..=3600), 30);
    }

    #[test]
    fn reorder_sets_list_order_and_rejects_stale_sets() {
        let store = temp_store();
        for (id, created_at) in [("a", 1), ("b", 2), ("c", 3)] {
            let mut row = location(id, NodeKind::Local, vec![]);
            row.created_at = created_at;
            store.put_location(&row).unwrap();
        }
        let ids = |store: &Store| -> Vec<String> {
            store
                .list_locations()
                .unwrap()
                .into_iter()
                .map(|l| l.id)
                .collect()
        };
        assert_eq!(ids(&store), ["a", "b", "c"]); // no order yet: created_at

        let order = ["c", "a", "b"].map(String::from);
        assert!(store.reorder_locations(&order).unwrap());
        assert_eq!(ids(&store), order);

        // Missing, duplicated or unknown ids write nothing.
        assert!(!store
            .reorder_locations(&["a", "b"].map(String::from))
            .unwrap());
        assert!(!store
            .reorder_locations(&["a", "a", "b"].map(String::from))
            .unwrap());
        assert!(!store
            .reorder_locations(&["a", "b", "x"].map(String::from))
            .unwrap());
        assert_eq!(ids(&store), order);

        // A location created after the last reorder goes last.
        let mut late = location("d", NodeKind::Local, vec![]);
        late.created_at = 0;
        store.put_location(&late).unwrap();
        assert_eq!(ids(&store), ["c", "a", "b", "d"]);
    }

    fn seed_full_location(store: &Store, id: &str) {
        store
            .put_location(&Location {
                data_plane_origin: Some(format!("https://{id}.example.test")),
                ..location(id, NodeKind::Remote, vec![OfferedMethod::Ping])
            })
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

    // A remote's certificate status is stored only while its location
    // exists, and a location delete removes it with the other child rows.
    #[test]
    fn certificate_status_lives_and_dies_with_its_location() {
        let store = temp_store();
        let status = CertificateStatus {
            issued_at: Some(1),
            expires_at: Some(2),
            last_error: None,
        };
        assert!(
            !store
                .put_certificate_status("missing", "https://missing.example.test", &status)
                .unwrap(),
            "no row for a location that does not exist"
        );
        seed_full_location(&store, "target");
        seed_full_location(&store, "keep");
        assert!(store
            .put_certificate_status("target", "https://target.example.test", &status)
            .unwrap());
        assert!(store
            .put_certificate_status("keep", "https://keep.example.test", &status)
            .unwrap());

        assert!(store.delete_location("target").unwrap());
        assert_eq!(store.get_certificate_status("target").unwrap(), None);
        assert_eq!(store.get_certificate_status("keep").unwrap(), Some(status));
        assert_eq!(store.get_certificate_status("missing").unwrap(), None);
    }

    // F-207: a certificate status belongs to the data-plane origin its
    // location had when it was reported. Once the admin changes or clears the
    // origin, the old origin's certificate is no longer shown as current; a
    // report for the new origin is.
    #[test]
    fn certificate_status_is_shown_only_for_the_origin_it_was_reported_for() {
        let store = temp_store();
        let mut remote = location("loc", NodeKind::Remote, vec![]);
        remote.data_plane_origin = Some("https://old.example.test".to_string());
        store.put_location(&remote).unwrap();
        let status = CertificateStatus {
            issued_at: Some(1),
            expires_at: Some(2),
            last_error: None,
        };
        assert!(store
            .put_certificate_status("loc", "https://old.example.test", &status)
            .unwrap());
        assert_eq!(
            store.get_certificate_status("loc").unwrap(),
            Some(status.clone())
        );

        remote.data_plane_origin = Some("https://new.example.test".to_string());
        store.update_location(&remote).unwrap();
        assert_eq!(
            store.get_certificate_status("loc").unwrap(),
            None,
            "the previous origin's certificate must not show as current"
        );
        assert!(store
            .put_certificate_status("loc", "https://new.example.test", &status)
            .unwrap());
        assert_eq!(
            store.get_certificate_status("loc").unwrap(),
            Some(status.clone())
        );

        remote.data_plane_origin = None;
        store.update_location(&remote).unwrap();
        assert_eq!(
            store.get_certificate_status("loc").unwrap(),
            None,
            "a cleared origin has no certificate"
        );

        // A row written before the origin was kept still reads, as no origin.
        let txn = store.db.begin_write().unwrap();
        txn.open_table(CERTIFICATE)
            .unwrap()
            .insert(
                "loc",
                br#"{"issued_at":1,"expires_at":2,"last_error":null}"#.as_slice(),
            )
            .unwrap();
        txn.commit().unwrap();
        assert_eq!(store.get_certificate_status("loc").unwrap(), Some(status));
    }

    // F-244: a certificate report is checked against the location's origin in
    // the same write transaction that stores it, so a report checked against
    // the old origin and written after an admin changed or cleared it is
    // dropped, not stamped with the new origin (or with none).
    #[test]
    fn a_certificate_report_for_a_replaced_origin_is_not_stored() {
        let store = temp_store();
        let mut remote = location("loc", NodeKind::Remote, vec![]);
        let status = CertificateStatus {
            issued_at: Some(1),
            expires_at: Some(2),
            last_error: None,
        };
        for next in [Some("https://new.example.test".to_string()), None] {
            remote.data_plane_origin = next;
            store.put_location(&remote).unwrap();
            assert!(
                !store
                    .put_certificate_status("loc", "https://old.example.test", &status)
                    .unwrap(),
                "a report for the replaced origin must not be written"
            );
            assert_eq!(store.get_certificate_status("loc").unwrap(), None);
        }
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

    // FR-023 single-use: the first redeem wins, marks the token used and writes
    // its agent; a second redeem of the same token returns false and writes no
    // agent. The store makes this atomic so two racing enrollments cannot both
    // redeem one token.
    #[test]
    fn a_token_can_be_consumed_exactly_once() {
        let store = temp_store();
        store
            .put_location(&location("loc", NodeKind::Remote, vec![]))
            .unwrap();
        store
            .put_enrollment_token(&EnrollmentToken {
                id: "t".to_string(),
                location_id: "loc".to_string(),
                token_hash: "h".to_string(),
                expires_at: 9999,
                used_at: None,
            })
            .unwrap();
        assert!(store
            .redeem_enrollment_token("t", 100, &agent("a1", "loc", None, false))
            .unwrap());
        assert!(
            !store
                .redeem_enrollment_token("t", 200, &agent("a2", "loc", None, false))
                .unwrap(),
            "a second redeem of the same token must fail"
        );
        assert_eq!(
            store.find_token_by_hash("h").unwrap().unwrap().used_at,
            Some(100)
        );
        let agents = store.list_agents("loc").unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "a1");
    }

    // F-182: only a remote location has agents, so a token never enrolls one
    // for a local location, even when the token row is still there.
    #[test]
    fn a_token_for_a_local_location_redeems_nothing() {
        let store = temp_store();
        store
            .put_location(&location("loc", NodeKind::Local, vec![]))
            .unwrap();
        store
            .put_enrollment_token(&EnrollmentToken {
                id: "t".to_string(),
                location_id: "loc".to_string(),
                token_hash: "h".to_string(),
                expires_at: 9999,
                used_at: None,
            })
            .unwrap();
        assert!(!store
            .redeem_enrollment_token("t", 100, &agent("a1", "loc", None, false))
            .unwrap());
        assert!(store.all_agents().unwrap().is_empty());
    }

    // Redemption is one transaction. No write boundary ever sees the
    // token consumed without its agent, as a consume-then-write split would.
    #[test]
    fn a_redemption_never_commits_the_token_without_its_agent() {
        let store = temp_store();
        store
            .put_location(&location("loc", NodeKind::Remote, vec![]))
            .unwrap();
        store
            .put_enrollment_token(&EnrollmentToken {
                id: "t".to_string(),
                location_id: "loc".to_string(),
                token_hash: "h".to_string(),
                expires_at: 9999,
                used_at: None,
            })
            .unwrap();
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let (observer, log) = (store.clone(), seen.clone());
        BEFORE_WRITE.set(Some(Box::new(move || {
            let token = observer.find_token_by_hash("h").unwrap().unwrap();
            let agent = observer.get_agent("a1").unwrap();
            log.borrow_mut()
                .push((token.used_at.is_some(), agent.is_some()));
        })));

        assert!(store
            .redeem_enrollment_token("t", 100, &agent("a1", "loc", None, false))
            .unwrap());

        let seen = seen.borrow();
        assert!(!seen.is_empty(), "the redemption started a write");
        assert!(
            seen.iter().all(|(used, agent)| used == agent),
            "token consumed without its agent: {seen:?}"
        );
        assert_eq!(store.list_agents("loc").unwrap().len(), 1);
    }

    // Redemption checks the token inside its own write transaction. A
    // second redemption that commits just before that write wins, and the first
    // writes nothing, so one token never yields two agents.
    #[test]
    fn a_redemption_racing_another_for_the_same_token_leaves_one_agent() {
        let store = temp_store();
        store
            .put_location(&location("loc", NodeKind::Remote, vec![]))
            .unwrap();
        store
            .put_enrollment_token(&EnrollmentToken {
                id: "t".to_string(),
                location_id: "loc".to_string(),
                token_hash: "h".to_string(),
                expires_at: 9999,
                used_at: None,
            })
            .unwrap();
        let fired = std::rc::Rc::new(std::cell::Cell::new(false));
        let (racer, once) = (store.clone(), fired.clone());
        BEFORE_WRITE.set(Some(Box::new(move || {
            if !once.replace(true) {
                assert!(racer
                    .redeem_enrollment_token("t", 50, &agent("a0", "loc", None, false))
                    .unwrap());
            }
        })));

        let won = store
            .redeem_enrollment_token("t", 100, &agent("a1", "loc", None, false))
            .unwrap();
        BEFORE_WRITE.set(None);

        assert!(fired.get(), "the racing redemption ran");
        assert!(!won, "the token was already redeemed");
        let agents = store.all_agents().unwrap();
        assert_eq!(agents.len(), 1, "one token, one agent");
        assert_eq!(agents[0].id, "a0");
    }

    // SEC-001: two active peers removing each other at the same time
    // leave one active. Both removals are held behind a write lock, so each reads
    // before either commits; only a count taken inside the delete's own write
    // transaction refuses the second removal.
    #[test]
    fn concurrent_cross_removal_keeps_one_active_administrator() {
        let store = temp_store();
        for id in ["a", "b"] {
            store
                .put_administrator(&Administrator {
                    id: id.to_string(),
                    username: format!("user-{id}"),
                    password_hash: Some("hash".to_string()),
                    status: AdministratorStatus::Active,
                    created_at: 0,
                    activation_token_hash: None,
                    activation_expires_at: None,
                    session_generation: 0,
                })
                .unwrap();
        }
        let held = store.db.begin_write().unwrap();
        let removals: Vec<_> = [("a", "b"), ("b", "a")]
            .into_iter()
            .map(|(caller, target)| {
                let store = store.clone();
                std::thread::spawn(move || store.remove_administrator(caller, target))
            })
            .collect();
        std::thread::sleep(std::time::Duration::from_millis(200));
        drop(held);

        let results: Vec<_> = removals.into_iter().map(|r| r.join().unwrap()).collect();
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            1,
            "{results:?}"
        );
        assert!(
            results
                .iter()
                .any(|r| matches!(r, Err(RemoveAdministratorError::LastActive))),
            "{results:?}"
        );
        let active = store
            .list_administrators()
            .unwrap()
            .into_iter()
            .filter(|a| a.status == AdministratorStatus::Active)
            .count();
        assert_eq!(active, 1, "exactly one active administrator remains");
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
        // Fresh install seeds defaults: the hardcoded fallbacks with the env
        // unset, the LG_EXEC_* values when set. Checked in a child process so the
        // env is exactly what this test says, whatever the shell exports.
        seeded_exec_defaults_under(&[], (8, 20));
        seeded_exec_defaults_under(
            &[("LG_EXEC_MAX_CONCURRENT", "3"), ("LG_EXEC_RATE_MAX", "5")],
            (3, 5),
        );

        let store = temp_store();
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

    /// Child half of `settings_persist_and_default_from_env_fallback`: a fresh
    /// store's seeded exec defaults under the env its parent set.
    #[test]
    #[ignore = "child half of settings_persist_and_default_from_env_fallback"]
    fn seeded_exec_defaults_child() {
        let want = |key: &str| std::env::var(key).unwrap().parse::<u64>().unwrap();
        let seeded = temp_store().settings().unwrap();
        assert_eq!(
            seeded.exec_max_concurrent as u64,
            want("LG_TEST_WANT_MAX_CONCURRENT")
        );
        assert_eq!(
            u64::from(seeded.exec_rate_max),
            want("LG_TEST_WANT_RATE_MAX")
        );
    }

    fn seeded_exec_defaults_under(env: &[(&str, &str)], (max_concurrent, rate_max): (usize, u32)) {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "store::tests::seeded_exec_defaults_child",
                "--exact",
                "--ignored",
            ])
            .env_remove("LG_EXEC_MAX_CONCURRENT")
            .env_remove("LG_EXEC_RATE_MAX")
            .env("LG_TEST_WANT_MAX_CONCURRENT", max_concurrent.to_string())
            .env("LG_TEST_WANT_RATE_MAX", rate_max.to_string())
            .envs(env.iter().copied());
        let output = child.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("test result: ok. 1 passed"),
            "seeded exec defaults under {env:?}: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
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
        // The seeded default itself (8 unless LG_EXEC_MAX_CONCURRENT is set) is
        // pinned by settings_persist_and_default_from_env_fallback.
        assert_eq!(read.exec_max_concurrent, default_exec_max_concurrent());
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
    fn a_stale_touch_never_rolls_last_seen_back() {
        // Liveness writes run concurrently on the blocking pool and may commit out
        // of order; the older one landing second must not regress last_seen.
        let store = temp_store();
        store.put_agent(&agent("a1", "loc", None, false)).unwrap();
        store.touch_agent_last_seen("a1", 2000).unwrap();
        store.touch_agent_last_seen("a1", 1999).unwrap();
        assert_eq!(
            store.get_agent("a1").unwrap().unwrap().last_seen,
            Some(2000)
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

    const SETTLE: std::time::Duration = std::time::Duration::from_millis(300);
    const PROBE_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

    /// Holds redb's write lock from an OS thread while `load` runs, then reports
    /// whether `probe` finished within [`PROBE_BOUND`] and each load's output.
    /// Waits with std sleeps only: a runtime whose workers are all parked cannot
    /// fire tokio timers.
    async fn probe_answers_while_writes_wait<T: Send + 'static, P: Send + 'static>(
        db: Arc<Database>,
        load: impl FnOnce() -> Vec<tokio::task::JoinHandle<T>>,
        probe: impl FnOnce() -> tokio::task::JoinHandle<P>,
    ) -> (bool, Vec<T>, P) {
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let txn = db.begin_write().unwrap();
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(txn);
        });
        held_rx.recv().unwrap();

        let load = load();
        std::thread::sleep(SETTLE);
        let probe = probe();
        let deadline = std::time::Instant::now() + PROBE_BOUND;
        while !probe.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let answered = probe.is_finished();

        release_tx.send(()).unwrap();
        holder.join().unwrap();
        let mut outputs = Vec::new();
        for task in load {
            outputs.push(task.await.unwrap());
        }
        (answered, outputs, probe.await.unwrap())
    }

    fn app_state() -> crate::AppState {
        let dir = std::env::temp_dir().join(format!(
            "lg-store-http-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        crate::AppState {
            store: Store::open(dir.join("db.redb")).unwrap(),
            transport: crate::TransportConfig::new([std::net::Ipv4Addr::LOCALHOST.into()]),
            login_limiter: Arc::new(crate::LoginLimiter::default()),
            setup_token: None,
            run: crate::RunService::for_test(8, std::time::Duration::from_secs(30), 100),
            files_root: Arc::from(dir.as_path()),
            enroll: crate::EnrollConfig::for_test("https://central.test:8443", b"id".to_vec()),
            tunnel_hub: crate::TunnelHub::new(),
        }
    }

    fn request(
        method: &str,
        uri: &str,
        cookie: Option<&str>,
        body: &str,
    ) -> axum::http::Request<axum::body::Body> {
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-forwarded-proto", "https")
            .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                40000,
            ))));
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    // Admin deletes and a reorder waiting on redb's write lock must not
    // hold the async workers, or /health stalls behind them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn health_answers_while_admin_deletes_and_a_reorder_wait_on_the_write_lock() {
        use axum::http::StatusCode;
        use tower::ServiceExt;

        let state = app_state();
        let password = "correct-horse-battery-staple";
        let hash = crate::auth::hash_password(password).unwrap();
        let store = state.store.clone();
        store
            .create_first_administrator("alice-id".into(), "alice".into(), hash)
            .unwrap();
        for id in ["a", "b", "c"] {
            store
                .put_location(&location(id, NodeKind::Local, vec![OfferedMethod::Ping]))
                .unwrap();
        }
        let app = crate::build(state);
        let credentials = format!(r#"{{"username":"alice","password":"{password}"}}"#);
        let login = app
            .clone()
            .oneshot(request("POST", "/api/auth/login", None, &credentials))
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::NO_CONTENT);
        let cookie = login.headers()["set-cookie"].to_str().unwrap();
        let cookie = cookie.split(';').next().unwrap().to_string();

        let (answered, statuses, health) = probe_answers_while_writes_wait(
            store.database(),
            || {
                [
                    ("DELETE", "/api/admin/locations/a", ""),
                    ("DELETE", "/api/admin/locations/b", ""),
                    (
                        "PUT",
                        "/api/admin/locations/order",
                        r#"{"ids":["a","b","c"]}"#,
                    ),
                ]
                .into_iter()
                .map(|(method, uri, body)| {
                    let request = request(method, uri, Some(&cookie), body);
                    tokio::spawn(app.clone().oneshot(request))
                })
                .collect()
            },
            || tokio::spawn(app.clone().oneshot(request("GET", "/health", None, ""))),
        )
        .await;
        assert!(answered, "/health must answer within {PROBE_BOUND:?}");
        assert_eq!(health.unwrap().status(), StatusCode::OK);
        let statuses: Vec<_> = statuses.into_iter().map(|r| r.unwrap().status()).collect();
        assert_eq!(statuses[..2], [StatusCode::NO_CONTENT; 2]);
        // The reorder lists a and b, so it wins only if it commits first.
        assert!(
            matches!(statuses[2], StatusCode::NO_CONTENT | StatusCode::CONFLICT),
            "{statuses:?}"
        );
    }

    // Structure check: every write transaction in this file runs through
    // `off_workers`. Each path runs twice while the write lock is held; two
    // inline waits pin both workers and the probe task never runs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_write_transaction_pins_an_async_worker_while_it_waits() {
        type Write = fn(&Store);
        let writes: [(&str, Write); 18] = [
            ("bootstrap", |s| drop(Store::bootstrap(&s.db))),
            ("create_first_administrator", |s| {
                drop(s.create_first_administrator("x".into(), "x".into(), "h".into()))
            }),
            ("create_pending_administrator", |s| {
                drop(s.create_pending_administrator(Administrator {
                    id: "p".into(),
                    username: "p".into(),
                    password_hash: None,
                    status: AdministratorStatus::Pending,
                    created_at: 0,
                    activation_token_hash: None,
                    activation_expires_at: None,
                    session_generation: 0,
                }))
            }),
            ("regenerate_activation", |s| {
                drop(s.regenerate_activation("p", "t".into(), 1))
            }),
            ("activate_administrator", |s| {
                drop(s.activate_administrator("p", "t", "h".into(), 0))
            }),
            ("remove_administrator", |s| {
                drop(s.remove_administrator("x", "p"))
            }),
            ("rotate_password", |s| {
                drop(s.rotate_password("x", "h", "n".into()))
            }),
            ("reorder_locations", |s| drop(s.reorder_locations(&[]))),
            ("delete_location_with_agents", |s| {
                drop(s.delete_location_with_agents("loc"))
            }),
            ("mint_enrollment_token", |s| {
                let token = EnrollmentToken {
                    id: "t".into(),
                    location_id: "loc".into(),
                    token_hash: "h".into(),
                    expires_at: 1,
                    used_at: None,
                };
                drop(s.mint_enrollment_token(&token, 0))
            }),
            ("redeem_enrollment_token", |s| {
                drop(s.redeem_enrollment_token("t", 0, &agent("a", "loc", None, false)))
            }),
            ("revoke_agents_for_location", |s| {
                drop(s.revoke_agents_for_location("loc"))
            }),
            ("touch_agents_last_seen", |s| {
                drop(s.touch_agents_last_seen(&[("a".into(), 1)]))
            }),
            ("write_record", |s| {
                drop(s.put_location(&location("loc", NodeKind::Local, vec![])))
            }),
            ("update_location", |s| {
                drop(s.update_location(&location("loc", NodeKind::Local, vec![])))
            }),
            ("write_if_location", |s| {
                drop(s.put_test_ip(&TestIp {
                    id: "ip".into(),
                    location_id: "loc".into(),
                    family: Family::V4,
                    address: "203.0.113.5".into(),
                    label: None,
                }))
            }),
            ("put_certificate_status", |s| {
                drop(s.put_certificate_status("loc", "o", &CertificateStatus::default()))
            }),
            ("remove_record", |s| drop(s.delete_test_ip("ip"))),
        ];
        let production = include_str!("store.rs")
            .split("\n#[cfg(test)]\npub(crate) mod tests")
            .next()
            .unwrap();
        assert_eq!(
            production.matches(".begin_write()").count(),
            writes.len(),
            "every write transaction needs an entry here"
        );

        let store = temp_store();
        let mut pinned = Vec::new();
        for (name, write) in writes {
            let (answered, _, ()) = probe_answers_while_writes_wait(
                store.database(),
                || {
                    (0..2)
                        .map(|_| {
                            let store = store.clone();
                            tokio::spawn(async move { write(&store) })
                        })
                        .collect()
                },
                || tokio::spawn(async {}),
            )
            .await;
            if !answered {
                pinned.push(name);
            }
        }
        assert!(
            pinned.is_empty(),
            "these writes pin the async workers: {pinned:?}"
        );
    }
}
