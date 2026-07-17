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
    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let key = record.id.to_string();
        let value = encode(record)?;
        let txn = self.db.begin_write().map_err(backend)?;
        {
            let mut table = txn.open_table(SESSION).map_err(backend)?;
            table
                .insert(key.as_str(), value.as_slice())
                .map_err(backend)?;
        }
        txn.commit().map_err(backend)?;
        Ok(())
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
        let txn = self.db.begin_write().map_err(backend)?;
        {
            let mut table = txn.open_table(SESSION).map_err(backend)?;
            table.remove(key.as_str()).map_err(backend)?;
        }
        txn.commit().map_err(backend)?;
        Ok(())
    }
}

#[async_trait]
impl ExpiredDeletion for RedbSessionStore {
    async fn delete_expired(&self) -> session_store::Result<()> {
        let now = OffsetDateTime::now_utc();
        let txn = self.db.begin_write().map_err(backend)?;
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
        txn.commit().map_err(backend)?;
        Ok(())
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
