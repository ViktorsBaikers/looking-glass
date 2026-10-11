//! A redb-backed [`SessionStore`] so tower-sessions state survives a container
//! restart (the memory store would drop every session on reboot, failing FR-006
//! persistence). Records are JSON-encoded into the store's `session` table keyed
//! by the opaque session id.

use std::sync::Arc;

use async_trait::async_trait;
use redb::{Database, ReadableDatabase, ReadableTable};
use time::OffsetDateTime;
use tower_sessions::cookie::{Key, SameSite};
use tower_sessions::service::SignedCookie;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, ExpiredDeletion};
use tower_sessions::{Expiry, SessionManagerLayer, SessionStore};

use crate::store::{Store, SESSION};

pub const COOKIE_NAME: &str = "lg.sid";
const IDLE_TIMEOUT_SECS: i64 = 30 * 60;

#[derive(Clone, Debug)]
pub struct RedbSessionStore {
    db: Arc<Database>,
    cookie_key: Key,
}

impl RedbSessionStore {
    pub fn new(store: &Store) -> Self {
        Self {
            db: store.database(),
            cookie_key: Key::from(store.session_cookie_key()),
        }
    }

    /// Delete every session record belonging to one administrator, except `keep`
    /// — the mechanism behind "removal ends that peer's sessions immediately"
    /// and "a password change signs out the caller's OTHER sessions" (spec #1).
    /// The record data has no secondary index on the admin id, so this scans and
    /// deletes inside one write transaction.
    pub async fn delete_for_admin(
        &self,
        admin_id: &str,
        keep: Option<Id>,
    ) -> session_store::Result<()> {
        let admin_id = admin_id.to_owned();
        write_off_runtime(&self.db, move |db| {
            let txn = db.begin_write().map_err(backend)?;
            {
                let mut table = txn.open_table(SESSION).map_err(backend)?;
                let doomed: Vec<String> = table
                    .iter()
                    .map_err(backend)?
                    .filter_map(|entry| {
                        let (key, value) = entry.ok()?;
                        let record = decode(value.value()).ok()?;
                        let mine = record
                            .data
                            .get(crate::auth::SESSION_ADMIN_KEY)
                            .and_then(|value| value.as_str())
                            == Some(admin_id.as_str());
                        (mine && Some(record.id) != keep).then(|| key.value().to_string())
                    })
                    .collect();
                for key in doomed {
                    table.remove(key.as_str()).map_err(backend)?;
                }
            }
            txn.commit().map_err(backend)
        })
        .await
    }
}

/// Runs a redb write on the blocking pool: `begin_write` waits out any other
/// writer and `commit` fsyncs, and neither may park an async worker.
async fn write_off_runtime<R: Send + 'static>(
    db: &Arc<Database>,
    job: impl FnOnce(&Database) -> session_store::Result<R> + Send + 'static,
) -> session_store::Result<R> {
    let db = Arc::clone(db);
    tokio::task::spawn_blocking(move || job(&db))
        .await
        .map_err(backend)?
}

fn backend<E: std::fmt::Display>(e: E) -> session_store::Error {
    session_store::Error::Backend(e.to_string())
}

fn encode(record: &Record) -> Result<Vec<u8>, session_store::Error> {
    serde_json::to_vec(record).map_err(|e| session_store::Error::Encode(e.to_string()))
}

fn decode(bytes: &[u8]) -> Result<Record, session_store::Error> {
    serde_json::from_slice(bytes).map_err(|e| session_store::Error::Decode(e.to_string()))
}

#[async_trait]
impl SessionStore for RedbSessionStore {
    /// Inserts under a fresh id, re-rolling on the (unlikely) collision.
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        let mut fresh = record.clone();
        *record = write_off_runtime(&self.db, move |db| {
            let txn = db.begin_write().map_err(backend)?;
            {
                let mut table = txn.open_table(SESSION).map_err(backend)?;
                while table
                    .get(fresh.id.to_string().as_str())
                    .map_err(backend)?
                    .is_some()
                {
                    fresh.id = Id::default();
                }
                table
                    .insert(fresh.id.to_string().as_str(), encode(&fresh)?.as_slice())
                    .map_err(backend)?;
            }
            txn.commit().map_err(backend)?;
            Ok(fresh)
        })
        .await?;
        Ok(())
    }

    /// Updates an existing record only. `with_always_save` re-saves every
    /// in-flight request's session, so a record deleted mid-request (logout,
    /// removal, a password change's purge) must not be written back.
    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let key = record.id.to_string();
        let value = encode(record)?;
        write_off_runtime(&self.db, move |db| {
            let txn = db.begin_write().map_err(backend)?;
            {
                let mut table = txn.open_table(SESSION).map_err(backend)?;
                if table.get(key.as_str()).map_err(backend)?.is_some() {
                    table
                        .insert(key.as_str(), value.as_slice())
                        .map_err(backend)?;
                }
            }
            txn.commit().map_err(backend)
        })
        .await
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        let key = session_id.to_string();
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(SESSION).map_err(backend)?;
        let Some(guard) = table.get(key.as_str()).map_err(backend)? else {
            return Ok(None);
        };
        let record = decode(guard.value())?;
        if record.expiry_date <= OffsetDateTime::now_utc() {
            return Ok(None);
        }
        Ok(Some(record))
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        let key = session_id.to_string();
        write_off_runtime(&self.db, move |db| {
            let txn = db.begin_write().map_err(backend)?;
            {
                let mut table = txn.open_table(SESSION).map_err(backend)?;
                table.remove(key.as_str()).map_err(backend)?;
            }
            txn.commit().map_err(backend)
        })
        .await
    }
}

#[async_trait]
impl ExpiredDeletion for RedbSessionStore {
    async fn delete_expired(&self) -> session_store::Result<()> {
        let now = OffsetDateTime::now_utc();
        write_off_runtime(&self.db, move |db| {
            let txn = db.begin_write().map_err(backend)?;
            {
                let mut table = txn.open_table(SESSION).map_err(backend)?;
                let expired: Vec<String> = table
                    .iter()
                    .map_err(backend)?
                    .filter_map(|entry| {
                        let (key, value) = entry.ok()?;
                        let record = decode(value.value()).ok()?;
                        (record.expiry_date <= now).then(|| key.value().to_string())
                    })
                    .collect();
                for key in expired {
                    table.remove(key.as_str()).map_err(backend)?;
                }
            }
            txn.commit().map_err(backend)
        })
        .await
    }
}

/// Deletes expired records now and then once per idle window, forever: `load`
/// only hides them, and `delete_for_admin` scans the whole table.
pub async fn purge_expired(store: RedbSessionStore) {
    let period = std::time::Duration::from_secs(IDLE_TIMEOUT_SECS as u64);
    let mut interval = tokio::time::interval(period);
    loop {
        interval.tick().await;
        if let Err(error) = store.delete_expired().await {
            tracing::warn!(%error, "expired session purge failed");
        }
    }
}

/// The session cookie is hardened per the Slice 2 checkpoint decision: opaque
/// high-entropy id, `HttpOnly` + `Secure` + `SameSite=Strict`, sliding idle
/// expiry. `with_always_save(true)` re-saves the session on every request so
/// activity refreshes the `OnInactivity` window (true sliding idle, FR-006). The
/// 12h absolute cap is enforced separately in `auth` via `auth_at`, since a
/// single tower-sessions `Expiry` cannot express both idle and absolute bounds.
pub fn session_layer(
    session_store: RedbSessionStore,
) -> SessionManagerLayer<RedbSessionStore, SignedCookie> {
    let cookie_key = session_store.cookie_key.clone();
    SessionManagerLayer::new(session_store)
        .with_signed(cookie_key)
        .with_name(COOKIE_NAME)
        .with_http_only(true)
        .with_secure(true)
        .with_same_site(SameSite::Strict)
        .with_always_save(true)
        .with_expiry(Expiry::OnInactivity(time::Duration::seconds(
            IDLE_TIMEOUT_SECS,
        )))
}
