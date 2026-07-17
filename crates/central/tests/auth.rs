use std::collections::HashMap;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use central::RedbSessionStore;
use redb::{Database, ReadableDatabase, TableDefinition};
use time::{Duration, OffsetDateTime};
use tower_sessions::cookie::{Cookie, CookieJar, Key};
use tower_sessions::session::{Id, Record};
use tower_sessions::SessionStore;
use tracing_subscriber::fmt::MakeWriter;

mod common;
use common::{
    assert_status, body_string, cleartext_request, secure_request, send, session_cookie,
    temp_files_dir, test_state, test_state_at, SETUP_TOKEN,
};

const PASSWORD: &str = "Sup3r-Secret-Passphrase!";
const SESSION_COOKIE_KEY: TableDefinition<&str, &[u8]> = TableDefinition::new("session_cookie_key");
const SESSION_COOKIE_KEY_ID: &str = "signing";
const TEST_SESSION_COOKIE_KEY: [u8; 64] = [7; 64];

fn persist_session_cookie_key(path: &Path, key: &[u8]) {
    let db = Database::create(path).expect("create session-key test database");
    let txn = db.begin_write().expect("start session-key write");
    {
        let mut table = txn
            .open_table(SESSION_COOKIE_KEY)
            .expect("open session-key table");
        table
            .insert(SESSION_COOKIE_KEY_ID, key)
            .expect("write session-key test material");
    }
    txn.commit().expect("commit session-key test material");
}

fn delete_session_cookie_key(path: &Path) {
    let db = Database::create(path).expect("open session-key test database");
    let txn = db.begin_write().expect("start session-key delete");
    {
        let mut table = txn
            .open_table(SESSION_COOKIE_KEY)
            .expect("open session-key table");
        table
            .remove(SESSION_COOKIE_KEY_ID)
            .expect("delete session-key test material");
    }
    txn.commit().expect("commit session-key deletion");
}

fn signed_test_state() -> central::AppState {
    let path = common::temp_db_path();
    persist_session_cookie_key(&path, &TEST_SESSION_COOKIE_KEY);
    test_state_at(path)
}

fn signed_session_cookie(id: Id) -> String {
    let key = Key::from(&TEST_SESSION_COOKIE_KEY);
    let mut jar = CookieJar::new();
    jar.signed_mut(&key)
        .add(Cookie::new(central::SESSION_COOKIE_NAME, id.to_string()));
    jar.get(central::SESSION_COOKIE_NAME)
        .expect("signed session cookie")
        .to_string()
}

fn setup_json(username: &str, password: &str) -> String {
    format!(r#"{{"username":"{username}","password":"{password}"}}"#)
}

fn setup_body(token: &str, username: &str, password: &str) -> String {
    format!(r#"{{"setup_token":"{token}","username":"{username}","password":"{password}"}}"#)
}

async fn install_admin(app: axum::Router, username: &str, password: &str) {
    let response = send(
        app,
        secure_request(
            "POST",
            "/api/setup",
            &setup_body(SETUP_TOKEN, username, password),
        ),
    )
    .await;
    assert_status(&response, StatusCode::CREATED);
}

// AC1 — the installer gates every non-setup route until an admin exists.
#[tokio::test]
async fn protected_route_is_refused_before_setup() {
    let app = central::build(test_state());
    let response = send(
        app,
        Request::builder()
            .uri("/api/admin/me")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&response, StatusCode::FORBIDDEN);
    assert!(body_string(response).await.contains("setup_required"));
}

// AC1 — public APIs are also refused until the first admin completes setup.
#[tokio::test]
async fn public_routes_are_refused_before_setup() {
    for uri in [
        "/api/locations",
        "/api/run/stream?method=ping&target=1.1.1.1",
        "/api/locations/loc/files/file/download",
    ] {
        let response = send(
            central::build(test_state()),
            Request::builder().uri(uri).body(Body::empty()).unwrap(),
        )
        .await;
        assert_status(&response, StatusCode::FORBIDDEN);
        assert!(body_string(response).await.contains("setup_required"));
    }
}

// AC1 — the setup-status probe the SPA reads is reachable pre-setup.
#[tokio::test]
async fn setup_status_reports_not_installed_on_a_fresh_deploy() {
    let app = central::build(test_state());
    let response = send(
        app,
        Request::builder()
            .uri("/api/setup/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&response, StatusCode::OK);
    assert!(body_string(response).await.contains("\"installed\":false"));
}

// AC2 — completing setup creates the admin and marks setup complete.
#[tokio::test]
async fn setup_creates_admin_and_marks_complete() {
    let state = test_state();
    let app = central::build(state.clone());
    install_admin(app, "alice", PASSWORD).await;

    assert!(state.store.is_installed().unwrap());
    assert_eq!(state.store.admin().unwrap().unwrap().username, "alice");

    let status = send(
        central::build(state),
        Request::builder()
            .uri("/api/setup/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(body_string(status).await.contains("\"installed\":true"));
}

// AC3 — once an admin exists the installer is closed; no second admin.
#[tokio::test]
async fn second_setup_is_refused_and_creates_no_second_admin() {
    let state = test_state();
    install_admin(central::build(state.clone()), "alice", PASSWORD).await;

    let response = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            "/api/setup",
            &setup_body(SETUP_TOKEN, "mallory", PASSWORD),
        ),
    )
    .await;
    assert_status(&response, StatusCode::CONFLICT);
    assert!(body_string(response).await.contains("already_installed"));
    assert_eq!(state.store.admin().unwrap().unwrap().username, "alice");
}

// AC4 — an admin route without a valid session is refused.
#[tokio::test]
async fn admin_route_without_session_is_unauthorized() {
    let state = test_state();
    install_admin(central::build(state.clone()), "alice", PASSWORD).await;

    let response = send(
        central::build(state),
        Request::builder()
            .uri("/api/admin/me")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&response, StatusCode::UNAUTHORIZED);
}

// FR-004 — a bare legacy session id cannot authorize a route, even if its
// matching server-side record remains present.
#[tokio::test]
async fn legacy_raw_session_id_is_refused_even_when_a_record_exists() {
    let state = test_state();
    install_admin(central::build(state.clone()), "alice", PASSWORD).await;

    let admin_id = state.store.admin().unwrap().unwrap().id;
    let raw_id = Id::default();
    let mut data = HashMap::new();
    data.insert("admin_id".to_string(), serde_json::json!(admin_id));
    data.insert(
        "auth_at".to_string(),
        serde_json::json!(OffsetDateTime::now_utc().unix_timestamp() as u64),
    );
    RedbSessionStore::new(&state.store)
        .save(&Record {
            id: raw_id,
            data,
            expiry_date: OffsetDateTime::now_utc() + Duration::hours(1),
        })
        .await
        .unwrap();

    let response = send(
        central::build(state),
        Request::builder()
            .uri("/api/admin/me")
            .header(
                "cookie",
                format!("{}={raw_id}", central::SESSION_COOKIE_NAME),
            )
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&response, StatusCode::UNAUTHORIZED);
}

// FR-004 — malformed durable signing material must fail startup rather than
// being replaced or falling back to plaintext cookies.
#[test]
fn malformed_session_cookie_key_refuses_store_open() {
    let path = common::temp_db_path();
    persist_session_cookie_key(&path, &[0; 63]);

    assert!(
        central::Store::open(path).is_err(),
        "invalid persisted signing material must prevent startup"
    );
}

// FR-004 — an existing key table without its signing row is corruption, not a
// request to silently replace the signer and invalidate active sessions.
#[test]
fn missing_session_cookie_key_row_refuses_store_open() {
    let path = common::temp_db_path();
    persist_session_cookie_key(&path, &TEST_SESSION_COOKIE_KEY);
    delete_session_cookie_key(&path);

    assert!(
        central::Store::open(path).is_err(),
        "an existing key table without its signing row must prevent startup"
    );
}

// A persistent volume from before Slice 19 has the old session table but no
// signing-key table; it receives the one-time key migration at startup.
#[test]
fn missing_session_cookie_key_table_migrates_existing_volume() {
    let path = common::temp_db_path();
    {
        let db = Database::create(&path).expect("create pre-migration database");
        let txn = db.begin_write().expect("start pre-migration write");
        txn.open_table(TableDefinition::<&str, &[u8]>::new("session"))
            .expect("create legacy session table");
        txn.commit().expect("commit pre-migration volume");
    }

    drop(central::Store::open(&path).expect("migrate pre-Slice-19 volume"));

    let db = Database::create(path).expect("reopen migrated database");
    let txn = db.begin_read().expect("read migrated database");
    let table = txn
        .open_table(SESSION_COOKIE_KEY)
        .expect("open migrated key table");
    let key = table
        .get(SESSION_COOKIE_KEY_ID)
        .expect("read migrated key")
        .expect("migrated signing row");
    assert_eq!(key.value().len(), TEST_SESSION_COOKIE_KEY.len());
}

// AC4 (expiry half / FR-006) — a session past the absolute cap is refused even
// though its cookie is still live.
#[tokio::test]
async fn session_past_absolute_cap_is_refused() {
    let state = signed_test_state();
    install_admin(central::build(state.clone()), "alice", PASSWORD).await;

    let session_store = RedbSessionStore::new(&state.store);
    let id = Id::default();
    let mut data = HashMap::new();
    data.insert(
        "admin_id".to_string(),
        serde_json::json!(state.store.admin().unwrap().unwrap().id),
    );
    data.insert("auth_at".to_string(), serde_json::json!(0u64));
    let record = Record {
        id,
        data,
        expiry_date: OffsetDateTime::now_utc() + Duration::hours(1),
    };
    session_store.save(&record).await.unwrap();

    let response = send(
        central::build(state),
        Request::builder()
            .uri("/api/admin/me")
            .header("cookie", signed_session_cookie(id))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&response, StatusCode::UNAUTHORIZED);
}

// FR-007 — logout ends the session immediately; the same cookie is then refused.
#[tokio::test]
async fn logout_ends_the_session_immediately() {
    let state = test_state();
    install_admin(central::build(state.clone()), "alice", PASSWORD).await;

    let login = send(
        central::build(state.clone()),
        secure_request("POST", "/api/auth/login", &setup_json("alice", PASSWORD)),
    )
    .await;
    assert_status(&login, StatusCode::NO_CONTENT);
    let cookie = session_cookie(&login).expect("login sets a session cookie");

    let authed = send(
        central::build(state.clone()),
        Request::builder()
            .uri("/api/admin/me")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&authed, StatusCode::OK);

    let logout = send(
        central::build(state.clone()),
        Request::builder()
            .method("POST")
            .uri("/api/auth/logout")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&logout, StatusCode::NO_CONTENT);

    let after = send(
        central::build(state),
        Request::builder()
            .uri("/api/admin/me")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&after, StatusCode::UNAUTHORIZED);
}

// AC5 — wrong username and wrong password fail with the same generic message.
#[tokio::test]
async fn wrong_credentials_do_not_enumerate() {
    let state = test_state();
    install_admin(central::build(state.clone()), "alice", PASSWORD).await;

    let wrong_password = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            "/api/auth/login",
            &setup_json("alice", "wrong-password-xx"),
        ),
    )
    .await;
    assert_status(&wrong_password, StatusCode::UNAUTHORIZED);
    let password_body = body_string(wrong_password).await;

    let wrong_username = send(
        central::build(state),
        secure_request("POST", "/api/auth/login", &setup_json("nobody", PASSWORD)),
    )
    .await;
    assert_status(&wrong_username, StatusCode::UNAUTHORIZED);
    let username_body = body_string(wrong_username).await;

    assert_eq!(
        password_body, username_body,
        "the failure must not reveal which field was wrong"
    );
    assert!(password_body.contains("Invalid username or password"));
}

// AC34 — cleartext admin login is refused; no session is issued.
#[tokio::test]
async fn cleartext_login_is_refused() {
    let state = test_state();
    install_admin(central::build(state.clone()), "alice", PASSWORD).await;

    let response = send(
        central::build(state),
        cleartext_request("POST", "/api/auth/login", &setup_json("alice", PASSWORD)),
    )
    .await;
    assert_status(&response, StatusCode::FORBIDDEN);
    assert!(
        session_cookie(&response).is_none(),
        "no session on a refused cleartext login"
    );
    assert!(body_string(response).await.contains("insecure_transport"));
}

// FR-005 / AC39 (route half) — login attempts are bounded per client.
#[tokio::test]
async fn login_attempts_are_rate_limited() {
    let state = test_state();
    install_admin(central::build(state.clone()), "alice", PASSWORD).await;

    for _ in 0..5 {
        let response = send(
            central::build(state.clone()),
            secure_request(
                "POST",
                "/api/auth/login",
                &setup_json("alice", "wrong-password-xx"),
            ),
        )
        .await;
        assert_status(&response, StatusCode::UNAUTHORIZED);
    }
    let blocked = send(
        central::build(state),
        secure_request(
            "POST",
            "/api/auth/login",
            &setup_json("alice", "wrong-password-xx"),
        ),
    )
    .await;
    assert_status(&blocked, StatusCode::TOO_MANY_REQUESTS);
}

// (hardening) create-admin is refused without or with a wrong setup token, and
// succeeds exactly once with the correct token — closing the first-run hijack race.
#[tokio::test]
async fn setup_requires_the_correct_setup_token() {
    let state = test_state();

    let missing = send(
        central::build(state.clone()),
        secure_request("POST", "/api/setup", &setup_body("", "alice", PASSWORD)),
    )
    .await;
    assert_status(&missing, StatusCode::FORBIDDEN);
    assert!(body_string(missing).await.contains("invalid_setup_token"));

    let wrong = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            "/api/setup",
            &setup_body("not-the-token", "alice", PASSWORD),
        ),
    )
    .await;
    assert_status(&wrong, StatusCode::FORBIDDEN);
    assert!(
        state.store.admin().unwrap().is_none(),
        "a rejected setup must create no admin"
    );

    let accepted = send(
        central::build(state.clone()),
        secure_request(
            "POST",
            "/api/setup",
            &setup_body(SETUP_TOKEN, "alice", PASSWORD),
        ),
    )
    .await;
    assert_status(&accepted, StatusCode::CREATED);
    assert_eq!(state.store.admin().unwrap().unwrap().username, "alice");
}

// (hardening) login rotates the session id, so a fixed pre-auth id cannot be
// promoted into an authenticated session (session fixation).
#[tokio::test]
async fn login_rotates_the_session_id() {
    let state = test_state();
    install_admin(central::build(state.clone()), "alice", PASSWORD).await;

    let first_login = send(
        central::build(state.clone()),
        secure_request("POST", "/api/auth/login", &setup_json("alice", PASSWORD)),
    )
    .await;
    assert_status(&first_login, StatusCode::NO_CONTENT);
    let first_cookie = session_cookie(&first_login).expect("first login sets a session cookie");

    let first_access = send(
        central::build(state.clone()),
        Request::builder()
            .uri("/api/admin/me")
            .header("cookie", &first_cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&first_access, StatusCode::OK);

    let mut request = secure_request("POST", "/api/auth/login", &setup_json("alice", PASSWORD));
    request
        .headers_mut()
        .insert("cookie", first_cookie.parse().unwrap());
    let second_login = send(central::build(state.clone()), request).await;
    assert_status(&second_login, StatusCode::NO_CONTENT);

    let second_cookie = session_cookie(&second_login).expect("second login sets a session cookie");
    assert_ne!(
        second_cookie, first_cookie,
        "the session cookie must rotate across the login boundary"
    );

    let old_cookie = send(
        central::build(state.clone()),
        Request::builder()
            .uri("/api/admin/me")
            .header("cookie", first_cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&old_cookie, StatusCode::UNAUTHORIZED);

    let new_cookie = send(
        central::build(state),
        Request::builder()
            .uri("/api/admin/me")
            .header("cookie", second_cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&new_cookie, StatusCode::OK);
}

// (correctness / FR-006) an authenticated request refreshes the sliding idle
// window — the session is re-saved with an expiry no earlier than before.
#[tokio::test]
async fn authenticated_request_refreshes_the_idle_window() {
    let state = signed_test_state();
    install_admin(central::build(state.clone()), "alice", PASSWORD).await;

    let session_store = RedbSessionStore::new(&state.store);
    let id = Id::default();
    let mut data = HashMap::new();
    data.insert(
        "admin_id".to_string(),
        serde_json::json!(state.store.admin().unwrap().unwrap().id),
    );
    data.insert(
        "auth_at".to_string(),
        serde_json::json!(OffsetDateTime::now_utc().unix_timestamp() as u64),
    );
    session_store
        .save(&Record {
            id,
            data,
            expiry_date: OffsetDateTime::now_utc() + Duration::minutes(1),
        })
        .await
        .unwrap();
    let cookie = signed_session_cookie(id);
    let before = session_store.load(&id).await.unwrap().unwrap().expiry_date;

    let authed = send(
        central::build(state.clone()),
        Request::builder()
            .uri("/api/admin/me")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&authed, StatusCode::OK);
    assert!(
        session_cookie(&authed).is_some(),
        "an authenticated request must re-issue the cookie (sliding idle)"
    );

    let after = session_store.load(&id).await.unwrap().unwrap().expiry_date;
    assert!(after >= before, "activity must not shorten the idle window");
}

// FR-004 — altering one byte of a signed cookie must invalidate it before the
// matching session record can authorize an admin route.
#[tokio::test]
async fn altered_signed_cookie_is_refused() {
    let state = test_state();
    install_admin(central::build(state.clone()), "alice", PASSWORD).await;

    let login = send(
        central::build(state.clone()),
        secure_request("POST", "/api/auth/login", &setup_json("alice", PASSWORD)),
    )
    .await;
    assert_status(&login, StatusCode::NO_CONTENT);
    let mut cookie = session_cookie(&login).expect("login sets a session cookie");
    let last = cookie.pop().expect("non-empty session cookie");
    cookie.push(if last == 'a' { 'b' } else { 'a' });

    let response = send(
        central::build(state),
        Request::builder()
            .uri("/api/admin/me")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&response, StatusCode::UNAUTHORIZED);
}

// FR-004 — the durable key lets a real signed login cookie survive a store reopen.
#[tokio::test]
async fn signed_cookie_survives_a_store_reopen() {
    let path = common::temp_db_path();
    let cookie = {
        let state = test_state_at(path.clone());
        install_admin(central::build(state.clone()), "alice", PASSWORD).await;

        let login = send(
            central::build(state.clone()),
            secure_request("POST", "/api/auth/login", &setup_json("alice", PASSWORD)),
        )
        .await;
        assert_status(&login, StatusCode::NO_CONTENT);
        let set_cookie = login.headers().get("set-cookie").unwrap().to_str().unwrap();
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("Secure"));
        assert!(set_cookie.contains("SameSite=Strict"));
        let cookie = session_cookie(&login).expect("login sets a session cookie");

        let before_reopen = send(
            central::build(state),
            Request::builder()
                .uri("/api/admin/me")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_status(&before_reopen, StatusCode::OK);
        cookie
    };

    let after_reopen = send(
        central::build(test_state_at(path)),
        Request::builder()
            .uri("/api/admin/me")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_status(&after_reopen, StatusCode::OK);
}

// Startup is denied when redb itself cannot open the persistent volume.
#[test]
fn redb_open_failure_refuses_startup() {
    assert!(central::Store::open(temp_files_dir()).is_err());
}

// AC26 — admin, settings, and the tower-sessions store all survive a store reopen.
#[tokio::test]
async fn data_persists_across_a_store_reopen() {
    let path = common::temp_db_path();
    let session_id = Id::default();

    {
        let store = central::Store::open(&path).unwrap();
        store
            .create_admin(
                "alice-id".to_string(),
                "alice".to_string(),
                "$argon2id$v=19$stored-hash-placeholder".to_string(),
            )
            .unwrap();

        let session_store = RedbSessionStore::new(&store);
        let mut data = HashMap::new();
        data.insert("admin_id".to_string(), serde_json::json!("alice-id"));
        let record = Record {
            id: session_id,
            data,
            expiry_date: OffsetDateTime::now_utc() + Duration::hours(1),
        };
        session_store.save(&record).await.unwrap();
    }

    {
        let store = central::Store::open(&path).unwrap();
        assert!(store.is_installed().unwrap());
        assert_eq!(store.admin().unwrap().unwrap().username, "alice");
        assert_eq!(store.settings().unwrap().site_title, "Looking Glass");

        let session_store = RedbSessionStore::new(&store);
        assert!(
            session_store.load(&session_id).await.unwrap().is_some(),
            "the persisted session must survive a restart"
        );
    }
}

// A process-global capturing subscriber, set once, so log events from any thread
// (including handler polls off the test thread) are recorded deterministically.
static LOG_BUFFER: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();

fn log_capture() -> Arc<Mutex<Vec<u8>>> {
    LOG_BUFFER
        .get_or_init(|| {
            let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
            let subscriber = tracing_subscriber::fmt()
                .with_writer(BufferWriter(buffer.clone()))
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .finish();
            tracing::subscriber::set_global_default(subscriber)
                .expect("install global log-capture subscriber");
            buffer
        })
        .clone()
}

// AC6 — no plaintext password appears in any emitted log line.
#[tokio::test]
async fn password_never_appears_in_logs() {
    let buffer = log_capture();

    let state = test_state();
    install_admin(central::build(state.clone()), "alice", PASSWORD).await;
    let login = send(
        central::build(state),
        secure_request("POST", "/api/auth/login", &setup_json("alice", PASSWORD)),
    )
    .await;
    assert_status(&login, StatusCode::NO_CONTENT);

    let logs = String::from_utf8_lossy(&buffer.lock().unwrap()).to_string();
    assert!(
        logs.contains("admin authenticated"),
        "auth events must be logged (else the scan is vacuous)"
    );
    assert!(
        !logs.contains(PASSWORD),
        "the plaintext password must never appear in a log line"
    );
}

#[derive(Clone)]
struct BufferWriter(Arc<Mutex<Vec<u8>>>);

impl Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for BufferWriter {
    type Writer = BufferWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
