//! Automatic HTTPS for the speed-test data plane: the agent
//! obtains and renews its own certificate from an ACME CA (Let's Encrypt
//! production by default, Subscriber Agreement accepted, no contact address)
//! through the TLS-ALPN-01 challenge answered on the data-plane listener itself,
//! so the operator configures nothing. An IP origin is ordered with Let's
//! Encrypt's six-day `shortlived` profile (the one that issues for addresses), a
//! DNS name with the default profile. The account key and the certificate are
//! cached owner-only in the agent's state directory and reused across restarts.
//! Renewal starts once a third of the lifetime remains; a failure is reported
//! and retried with a doubling backoff while the still-valid certificate keeps
//! serving.

use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use instant_acme::{
    Account, AccountBuilder, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier,
    LetsEncrypt, NewAccount, NewOrder, OrderStatus, RetryPolicy,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::{ClientHello, ParsedCertificate, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use serde::{Deserialize, Serialize};
use shared::protocol::CertificateStatus;
use tokio::sync::watch;

/// The TLS-ALPN-01 protocol name (RFC 8737).
const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";
const ACCOUNT_FILE: &str = "acme-account.json";
const CERTIFICATE_FILE: &str = "acme-certificate.json";
/// First retry after a failed issuance; each further failure doubles it.
const FIRST_RETRY: Duration = Duration::from_secs(60);
/// Well inside Let's Encrypt's failed-validation allowance (5 per hour).
const MAX_RETRY: Duration = Duration::from_secs(3600);
/// Retry after a new certificate the agent's clock makes due or invalid on
/// arrival: ordering again cannot help and burns the CA's duplicate-certificate
/// limit (Let's Encrypt: 5 a week), so skew costs at most two orders a day.
const CLOCK_RETRY: Duration = Duration::from_secs(12 * 3600);
/// How far a new certificate's notBefore may lie ahead of the agent's clock:
/// visitors check validity with their own clocks, so a slow agent clock is no
/// reason to discard it. Expired certificates are still refused.
const SLOW_CLOCK_TOLERANCE: Duration = Duration::from_secs(24 * 3600);
/// Deadline for one issuance, account to certificate: instant-acme bounds each
/// of its two polls at 30 s but not a single request the CA never answers.
const ACME_TIMEOUT: Duration = Duration::from_secs(300);

type Builder = Arc<dyn Fn() -> Result<AccountBuilder, instant_acme::Error> + Send + Sync>;

/// Where certificates come from. Production is [`Acme::lets_encrypt`]; tests
/// point `directory_url` and `builder` (the HTTP client) at an in-process CA.
pub struct Acme {
    pub directory_url: String,
    pub builder: Builder,
    pub state_dir: PathBuf,
    pub first_retry: Duration,
    pub timeout: Duration,
}

impl Acme {
    pub fn lets_encrypt(state_dir: PathBuf) -> Self {
        Self {
            directory_url: LetsEncrypt::Production.url().to_string(),
            builder: Arc::new(Account::builder),
            state_dir,
            first_retry: FIRST_RETRY,
            timeout: ACME_TIMEOUT,
        }
    }
}

/// The data-plane listener's certificates: the served one, swapped in place on
/// renewal (no restart), and during an issuance the TLS-ALPN-01 challenge
/// certificate, handed only to a client that asks for `acme-tls/1`.
#[derive(Debug, Default)]
pub struct CertResolver {
    served: RwLock<Option<Arc<CertifiedKey>>>,
    challenge: RwLock<Option<Arc<CertifiedKey>>>,
}

impl CertResolver {
    fn serve(&self, issued: Option<&Issued>) {
        *self.served.write().expect("served certificate lock") =
            issued.map(|issued| Arc::clone(&issued.key));
    }

    fn set_challenge(&self, key: Option<Arc<CertifiedKey>>) {
        *self.challenge.write().expect("challenge certificate lock") = key;
    }
}

impl ResolvesServerCert for CertResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let acme = hello
            .alpn()
            .is_some_and(|mut protocols| protocols.any(|protocol| protocol == ACME_TLS_ALPN));
        let slot = if acme { &self.challenge } else { &self.served };
        slot.read().ok()?.clone()
    }
}

/// The data plane's TLS settings: certificates from `resolver`, HTTP/1.1 for
/// browsers and `acme-tls/1` for the CA's validation.
pub fn server_config(resolver: Arc<CertResolver>) -> rustls::ServerConfig {
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("ring supports the default TLS versions")
    .with_no_client_auth()
    .with_cert_resolver(resolver);
    config.alpn_protocols = vec![b"http/1.1".to_vec(), ACME_TLS_ALPN.to_vec()];
    config
}

/// Keep a valid certificate for the assigned origin for the agent's life: serve
/// the cached one, order a new one when none is cached or a third of its
/// lifetime remains, publish the status, and follow origin changes.
pub async fn manage(
    acme: Acme,
    mut origin: watch::Receiver<Option<String>>,
    resolver: Arc<CertResolver>,
    status: watch::Sender<Option<CertificateStatus>>,
) {
    let mut failures = 0;
    loop {
        let host = origin.borrow_and_update().as_deref().and_then(origin_host);
        let wait = match host {
            Some(host) => {
                let still_assigned = || !origin.has_changed().unwrap_or(false);
                Some(
                    ensure(
                        &acme,
                        &host,
                        &resolver,
                        &status,
                        &mut failures,
                        still_assigned,
                    )
                    .await,
                )
            }
            None => None,
        };
        tokio::select! {
            _ = async {
                match wait {
                    Some(wait) => tokio::time::sleep(wait).await,
                    None => std::future::pending().await,
                }
            } => {}
            changed = origin.changed() => {
                if changed.is_err() {
                    return;
                }
                failures = 0;
            }
        }
    }
}

/// One pass for `host`: serve what is cached, renew if due, report, and say how
/// long until the next pass.
async fn ensure(
    acme: &Acme,
    host: &str,
    resolver: &CertResolver,
    status: &watch::Sender<Option<CertificateStatus>>,
    failures: &mut u32,
    still_assigned: impl Fn() -> bool,
) -> Duration {
    let mut current = read_json::<Stored>(&acme.state_dir.join(CERTIFICATE_FILE))
        .filter(|stored| stored.host == host)
        .and_then(Issued::parse);
    resolver.serve(current.as_ref());
    let mut last_error = None;
    let mut clock = false;
    if current
        .as_ref()
        .is_none_or(|issued| unix_now() >= issued.renew_at())
    {
        let fresh = tokio::time::timeout(acme.timeout, issue(acme, host, resolver))
            .await
            .unwrap_or_else(|_| Err(format!("the CA did not answer within {:?}", acme.timeout)));
        resolver.set_challenge(None);
        match fresh.and_then(|stored| {
            let encoded = serde_json::to_vec(&stored).map_err(|error| error.to_string())?;
            let issued = Issued::parse(stored).ok_or("the CA returned an unusable certificate")?;
            issued.check(host)?;
            if issued.not_before >= issued.not_after {
                return Err("the CA returned a certificate with no validity period".into());
            }
            let valid_from = issued
                .not_before
                .saturating_sub(SLOW_CLOCK_TOLERANCE.as_secs());
            if !(valid_from..=issued.not_after).contains(&unix_now()) {
                clock = true;
                return Err("the new certificate is not valid now: check the clock".into());
            }
            write_owner_only(&acme.state_dir.join(CERTIFICATE_FILE), &encoded)
                .map_err(|error| format!("could not store the certificate: {error}"))?;
            Ok(issued)
        }) {
            Ok(issued) => {
                resolver.serve(Some(&issued));
                if unix_now() >= issued.renew_at() {
                    clock = true;
                    *failures += 1;
                    last_error = Some(
                        "the new certificate is already due for renewal: check the clock".into(),
                    );
                } else {
                    *failures = 0;
                }
                current = Some(issued);
            }
            Err(error) => {
                tracing::warn!(%host, %error, "data-plane certificate issuance failed");
                *failures += 1;
                last_error = Some(error);
            }
        }
    }
    let next = CertificateStatus {
        issued_at: current.as_ref().map(|issued| issued.not_before),
        expires_at: current.as_ref().map(|issued| issued.not_after),
        last_error,
    };
    status.send_if_modified(|reported| {
        // A pass that outlived its origin reports nothing: the tunnel voided
        // the status when the origin changed, and the next pass reports anew.
        if !still_assigned() {
            return false;
        }
        let changed = reported.as_ref() != Some(&next);
        *reported = Some(next.clone());
        changed
    });
    match current {
        Some(issued) if next.last_error.is_none() => {
            Duration::from_secs(issued.renew_at().saturating_sub(unix_now()))
        }
        _ if clock => CLOCK_RETRY,
        _ => retry_after(acme.first_retry, *failures),
    }
}

/// Order a certificate for `host`, answering TLS-ALPN-01 through `resolver`.
async fn issue(acme: &Acme, host: &str, resolver: &CertResolver) -> Result<Stored, String> {
    let account = account(acme).await?;
    let identifiers = [match host.parse::<IpAddr>() {
        Ok(ip) => Identifier::Ip(ip),
        Err(_) => Identifier::Dns(host.to_string()),
    }];
    let mut new_order = NewOrder::new(&identifiers);
    if matches!(identifiers[0], Identifier::Ip(_)) {
        new_order = new_order.profile("shortlived");
    }
    let mut order = account.new_order(&new_order).await.map_err(acme_error)?;
    {
        let mut authorizations = order.authorizations();
        while let Some(authorization) = authorizations.next().await {
            let mut authorization = authorization.map_err(acme_error)?;
            if authorization.status == AuthorizationStatus::Valid {
                continue;
            }
            let mut challenge = authorization
                .challenge(ChallengeType::TlsAlpn01)
                .ok_or("the CA offered no tls-alpn-01 challenge")?;
            let digest = challenge.key_authorization().digest();
            resolver.set_challenge(Some(challenge_certificate(host, digest.as_ref())?));
            challenge.set_ready().await.map_err(acme_error)?;
        }
    }
    match order
        .poll_ready(&RetryPolicy::default())
        .await
        .map_err(acme_error)?
    {
        OrderStatus::Ready => {}
        status => return Err(format!("the certificate order ended {status:?}")),
    }
    let key_pem = order.finalize().await.map_err(acme_error)?;
    let chain_pem = order
        .poll_certificate(&RetryPolicy::default())
        .await
        .map_err(acme_error)?;
    Ok(Stored {
        host: host.to_string(),
        chain_pem,
        key_pem,
    })
}

/// The cached ACME account, or a new one (terms accepted, no contact address).
async fn account(acme: &Acme) -> Result<Account, String> {
    let path = acme.state_dir.join(ACCOUNT_FILE);
    let builder = (acme.builder)().map_err(acme_error)?;
    if let Some(credentials) = read_json::<AccountCredentials>(&path) {
        return builder
            .from_credentials(credentials)
            .await
            .map_err(acme_error);
    }
    let new_account = NewAccount {
        contact: &[],
        terms_of_service_agreed: true,
        only_return_existing: false,
    };
    let (account, credentials) = builder
        .create(&new_account, acme.directory_url.clone(), None)
        .await
        .map_err(acme_error)?;
    let encoded = serde_json::to_vec(&credentials).map_err(|error| error.to_string())?;
    write_owner_only(&path, &encoded)
        .map_err(|error| format!("could not store the ACME account: {error}"))?;
    Ok(account)
}

fn acme_error(error: instant_acme::Error) -> String {
    format!("ACME: {error}")
}

/// A self-signed certificate for `host` carrying the critical acmeIdentifier
/// extension over the key authorization digest (RFC 8737 section 3).
fn challenge_certificate(host: &str, digest: &[u8]) -> Result<Arc<CertifiedKey>, String> {
    let rcgen_error = |error: rcgen::Error| error.to_string();
    let mut params = rcgen::CertificateParams::new(vec![host.to_string()]).map_err(rcgen_error)?;
    params.custom_extensions = vec![rcgen::CustomExtension::new_acme_identifier(digest)];
    let key = rcgen::KeyPair::generate().map_err(rcgen_error)?;
    let cert = params.self_signed(&key).map_err(rcgen_error)?;
    certified_key(
        vec![cert.der().clone()],
        PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    )
}

fn certified_key(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<CertifiedKey>, String> {
    let signer =
        rustls::crypto::ring::sign::any_supported_type(&key).map_err(|error| error.to_string())?;
    Ok(Arc::new(CertifiedKey::new(chain, signer)))
}

/// The certificate as cached on disk.
#[derive(Serialize, Deserialize)]
struct Stored {
    host: String,
    chain_pem: String,
    key_pem: String,
}

/// A cached certificate ready to serve.
struct Issued {
    not_before: u64,
    not_after: u64,
    key: Arc<CertifiedKey>,
}

impl Issued {
    fn parse(stored: Stored) -> Option<Self> {
        let chain = CertificateDer::pem_slice_iter(stored.chain_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        let (not_before, not_after) = validity(chain.first()?)?;
        let key = PrivateKeyDer::from_pem_slice(stored.key_pem.as_bytes()).ok()?;
        Some(Self {
            not_before,
            not_after,
            key: certified_key(chain, key).ok()?,
        })
    }

    fn renew_at(&self) -> u64 {
        renew_at(self.not_before, self.not_after)
    }

    /// Whether this is the certificate ordered: its chain starts with a leaf on
    /// the order key (so the leaf leads), and that leaf names `host`.
    fn check(&self, host: &str) -> Result<(), String> {
        self.key
            .keys_match()
            .map_err(|_| "the CA's chain does not start with a certificate for the order key")?;
        let leaf = self
            .key
            .end_entity_cert()
            .and_then(ParsedCertificate::try_from)
            .map_err(|error| error.to_string())?;
        let name = ServerName::try_from(host).map_err(|error| error.to_string())?;
        rustls::client::verify_server_name(&leaf, &name)
            .map_err(|_| format!("the CA's certificate is not for {host}"))
    }
}

/// When a third of the lifetime remains.
fn renew_at(not_before: u64, not_after: u64) -> u64 {
    not_after - not_after.saturating_sub(not_before) / 3
}

/// `first`, doubled for each failure after the first, capped at [`MAX_RETRY`].
fn retry_after(first: Duration, failures: u32) -> Duration {
    first
        .saturating_mul(1u32 << failures.saturating_sub(1).min(20))
        .min(MAX_RETRY)
}

/// The host of an `https://host[:port]` origin, brackets dropped from an IPv6
/// literal.
pub fn origin_host(origin: &str) -> Option<String> {
    let authority = origin.strip_prefix("https://")?;
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// A certificate's notBefore and notAfter in unix seconds: a minimal DER walk of
/// Certificate > TBSCertificate > validity (RFC 5280 section 4.1).
fn validity(cert: &[u8]) -> Option<(u64, u64)> {
    let (_, cert, _) = der(cert)?;
    let (_, tbs, _) = der(cert)?;
    let (tag, _, after_version) = der(tbs)?;
    let mut rest = if tag == 0xa0 { after_version } else { tbs };
    // serialNumber, signature, issuer
    for _ in 0..3 {
        rest = der(rest)?.2;
    }
    let (_, validity, _) = der(rest)?;
    let (tag, not_before, rest) = der(validity)?;
    let not_before = der_time(tag, not_before)?;
    let (tag, not_after, _) = der(rest)?;
    Some((not_before, der_time(tag, not_after)?))
}

/// A UTCTime (0x17) or GeneralizedTime (0x18) in the RFC 5280 `...Z` form.
fn der_time(tag: u8, text: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(text).ok()?.strip_suffix('Z')?;
    let (year, text) = match tag {
        0x17 => {
            let year: i64 = text.get(..2)?.parse().ok()?;
            (
                if year < 50 { 2000 + year } else { 1900 + year },
                &text[2..],
            )
        }
        0x18 => (text.get(..4)?.parse().ok()?, &text[4..]),
        _ => return None,
    };
    if text.len() != 10 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let field = |at: usize| text[at..at + 2].parse::<i64>().ok();
    let (month, day) = (field(0)?, field(2)?);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + field(4)? * 3_600 + field(6)? * 60 + field(8)?).ok()
}

/// Split one DER TLV off the front: its tag, its content and what follows.
fn der(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, input) = input.split_first()?;
    let (&first, mut input) = input.split_first()?;
    let len = if first < 0x80 {
        usize::from(first)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || input.len() < count {
            return None;
        }
        let (bytes, tail) = input.split_at(count);
        input = tail;
        bytes
            .iter()
            .fold(0, |len, &byte| len << 8 | usize::from(byte))
    };
    if input.len() < len {
        return None;
    }
    let (content, rest) = input.split_at(len);
    Some((tag, content, rest))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Write `bytes` owner-only (0600) through a temp file and a rename, so a crash
/// leaves the previous file whole.
// ponytail: enroll.rs has a private twin (with fchown); share one when enroll.rs is next touched.
fn write_owner_only(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod fake {
    //! An in-process ACME CA (RFC 8555) for tests. It speaks just enough of the
    //! protocol for instant-acme, validates TLS-ALPN-01 for real by dialing the
    //! agent's data-plane listener with ALPN `acme-tls/1` (RFC 8737, with the
    //! RFC 8738 reverse-DNS SNI for an IP), and issues from a throwaway CA. JWS
    //! signatures are not verified: this tests the agent's client, not the CA.

    use super::*;
    use axum::body::Bytes;
    use axum::http::{Method, Request, Response, StatusCode};
    use instant_acme::{BodyWrapper, BytesResponse, HttpClient};
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::ServerName;
    use rustls::{DigitallySignedStruct, SignatureScheme};
    use serde_json::{json, Value};
    use std::future::Future;
    use std::net::SocketAddr;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::time::Instant;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const BASE: &str = "http://fake-acme.test";
    const DIRECTORY: &str = "http://fake-acme.test/directory";

    #[derive(Clone)]
    pub(crate) struct FakeCa(Arc<Inner>);

    struct Inner {
        issuer: Issuer<'static, KeyPair>,
        root: CertificateDer<'static>,
        root_pem: String,
        state: Mutex<State>,
    }

    #[derive(Default)]
    pub(crate) struct State {
        /// The agent listener a TLS-ALPN-01 validation dials.
        pub(crate) port: u16,
        /// Lifetime of each issued certificate, in seconds.
        pub(crate) lifetime: u64,
        /// Refuse this many new orders (rate limited) before accepting one.
        pub(crate) fail_orders: u32,
        /// Answer each certificate download with something that is not one.
        pub(crate) bad_chain: bool,
        /// Record each new order, then never answer it.
        pub(crate) hang_orders: bool,
        /// Issue the leaf on a key of the CA's own instead of the CSR's.
        pub(crate) other_key: bool,
        /// Issue the leaf for this name instead of the ordered one.
        pub(crate) other_host: Option<String>,
        /// Put the root before the leaf in the chain.
        pub(crate) root_first: bool,
        /// Issue the leaf valid between these unix times instead of from now.
        pub(crate) window: Option<(u64, u64)>,
        pub(crate) accounts: u32,
        /// Every new-order request, refused ones included: when, and its payload.
        pub(crate) orders: Vec<(Instant, Value)>,
        /// Every TLS-ALPN-01 validation's outcome.
        pub(crate) validations: Vec<bool>,
        thumbprint: String,
        order: Option<Order>,
    }

    struct Order {
        id: usize,
        identifier: Value,
        host: String,
        token: String,
        status: &'static str,
        chain: Option<String>,
    }

    type Reply = (StatusCode, Vec<(&'static str, String)>, Vec<u8>);

    impl FakeCa {
        pub(crate) fn new(lifetime: u64) -> Self {
            let key = KeyPair::generate().unwrap();
            let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
            params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            let root = params.self_signed(&key).unwrap();
            Self(Arc::new(Inner {
                root_pem: root.pem(),
                root: root.der().clone(),
                issuer: Issuer::new(params, key),
                state: Mutex::new(State {
                    lifetime,
                    ..State::default()
                }),
            }))
        }

        pub(crate) fn state(&self) -> std::sync::MutexGuard<'_, State> {
            self.0.state.lock().unwrap()
        }

        /// The ACME seam pointed at this CA, retrying after 100 ms.
        pub(crate) fn acme(&self, state_dir: &Path) -> Acme {
            let ca = self.clone();
            Acme {
                directory_url: DIRECTORY.to_string(),
                builder: Arc::new(move || Ok(Account::builder_with_http(Box::new(ca.clone())))),
                state_dir: state_dir.to_path_buf(),
                first_retry: Duration::from_millis(100),
                timeout: ACME_TIMEOUT,
            }
        }

        async fn handle(&self, method: Method, path: &str, body: &[u8]) -> Reply {
            if method == Method::GET && path == "/directory" {
                return ok(json!({
                    "newNonce": format!("{BASE}/nonce"),
                    "newAccount": format!("{BASE}/account"),
                    "newOrder": format!("{BASE}/order"),
                }));
            }
            if method == Method::HEAD {
                return (StatusCode::OK, vec![], vec![]);
            }
            let (protected, payload) = jws(body);
            let mut segments = path.trim_start_matches('/').split('/');
            match (segments.next(), segments.next()) {
                (Some("account"), None) => {
                    let mut state = self.state();
                    state.accounts += 1;
                    state.thumbprint = thumbprint(&protected["jwk"]);
                    (
                        StatusCode::CREATED,
                        vec![("location", format!("{BASE}/acct/1"))],
                        json!({"status": "valid"}).to_string().into_bytes(),
                    )
                }
                (Some("order"), None) => {
                    let hang = self.state().hang_orders;
                    let reply = self.new_order(payload);
                    if hang {
                        std::future::pending::<()>().await;
                    }
                    reply
                }
                (Some("order"), Some(_)) => ok(order_json(self.state().order.as_ref().unwrap())),
                (Some("authz"), Some(_)) => ok(authz_json(self.state().order.as_ref().unwrap())),
                (Some("chall"), Some(_)) => self.challenge().await,
                (Some("finalize"), Some(_)) => self.finalize(&payload),
                (Some("cert"), Some(_)) => {
                    let state = self.state();
                    let chain = if state.bad_chain {
                        "not a certificate".to_string()
                    } else {
                        state
                            .order
                            .as_ref()
                            .and_then(|order| order.chain.clone())
                            .unwrap_or_default()
                    };
                    (
                        StatusCode::OK,
                        vec![("content-type", "application/pem-certificate-chain".into())],
                        chain.into_bytes(),
                    )
                }
                _ => (StatusCode::NOT_FOUND, vec![], vec![]),
            }
        }

        fn new_order(&self, payload: Value) -> Reply {
            let mut state = self.state();
            state.orders.push((Instant::now(), payload.clone()));
            if state.fail_orders > 0 {
                state.fail_orders -= 1;
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    vec![("content-type", "application/problem+json".into())],
                    json!({
                        "type": "urn:ietf:params:acme:error:rateLimited",
                        "detail": "too many new orders",
                        "status": 429
                    })
                    .to_string()
                    .into_bytes(),
                );
            }
            let id = state.orders.len();
            let identifier = payload["identifiers"][0].clone();
            let order = Order {
                id,
                host: identifier["value"].as_str().unwrap_or_default().to_string(),
                identifier,
                token: format!("token-{id}"),
                status: "pending",
                chain: None,
            };
            let body = order_json(&order).to_string().into_bytes();
            state.order = Some(order);
            (
                StatusCode::CREATED,
                vec![("location", format!("{BASE}/order/{id}"))],
                body,
            )
        }

        async fn challenge(&self) -> Reply {
            let (port, host, key_authorization) = {
                let state = self.state();
                let order = state.order.as_ref().unwrap();
                (
                    state.port,
                    order.host.clone(),
                    format!("{}.{}", order.token, state.thumbprint),
                )
            };
            let valid = validate(port, &host, &sha256(key_authorization.as_bytes())).await;
            let mut state = self.state();
            state.validations.push(valid);
            let order = state.order.as_mut().unwrap();
            order.status = if valid { "ready" } else { "invalid" };
            ok(challenge_json(order))
        }

        fn finalize(&self, payload: &Value) -> Reply {
            let csr = b64url_decode(payload["csr"].as_str().unwrap_or_default());
            let mut state = self.state();
            let chain = self.issue(&state, &csr);
            let order = state.order.as_mut().unwrap();
            order.chain = Some(chain);
            order.status = "valid";
            ok(order_json(order))
        }

        /// The ordered leaf on the CSR's key, valid from now for the lifetime,
        /// unless a knob in `state` spoils it.
        fn issue(&self, state: &State, csr: &[u8]) -> String {
            let order = state.order.as_ref().unwrap();
            let host = state.other_host.as_deref().unwrap_or(&order.host);
            let now = unix_now();
            let (not_before, not_after) = state.window.unwrap_or((now, now + state.lifetime));
            let chain = if state.other_key {
                self.chain(host, &KeyPair::generate().unwrap(), not_before, not_after)
            } else {
                let key = CsrKey(csr_public_key(csr).expect("a PKCS#10 CSR"));
                self.chain(host, &key, not_before, not_after)
            };
            match chain.strip_suffix(&self.0.root_pem) {
                Some(leaf) if state.root_first => format!("{}{leaf}", self.0.root_pem),
                _ => chain,
            }
        }

        /// A leaf for `host` on `key`, valid between the unix times, and the root.
        fn chain(
            &self,
            host: &str,
            key: &impl rcgen::PublicKeyData,
            not_before: u64,
            not_after: u64,
        ) -> String {
            let epoch = rcgen::date_time_ymd(1970, 1, 1);
            let mut params = CertificateParams::new(vec![host.to_string()]).unwrap();
            params.not_before = epoch + Duration::from_secs(not_before);
            params.not_after = epoch + Duration::from_secs(not_after);
            let leaf = params.signed_by(key, &self.0.issuer).unwrap();
            format!("{}{}", leaf.pem(), self.0.root_pem)
        }

        /// Cache in `state_dir` a certificate for `host` this CA issued earlier.
        pub(crate) fn cache(&self, state_dir: &Path, host: &str, not_before: u64, not_after: u64) {
            let key = KeyPair::generate().unwrap();
            let stored = Stored {
                host: host.to_string(),
                chain_pem: self.chain(host, &key, not_before, not_after),
                key_pem: key.serialize_pem(),
            };
            write_owner_only(
                &state_dir.join(CERTIFICATE_FILE),
                &serde_json::to_vec(&stored).unwrap(),
            )
            .unwrap();
        }

        /// Trust only this CA (the "locally trusted test CA").
        pub(crate) fn client_config(&self) -> rustls::ClientConfig {
            let mut roots = rustls::RootCertStore::empty();
            roots.add(self.0.root.clone()).unwrap();
            let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
            config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
            config
        }
    }

    impl HttpClient for FakeCa {
        fn request(
            &self,
            request: Request<BodyWrapper<Bytes>>,
        ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>>
        {
            let ca = self.clone();
            Box::pin(async move {
                let (parts, body) = request.into_parts();
                let body = axum::body::to_bytes(axum::body::Body::new(body), usize::MAX)
                    .await
                    .unwrap();
                let (status, headers, body) =
                    ca.handle(parts.method, parts.uri.path(), &body).await;
                let mut response = Response::builder()
                    .status(status)
                    .header("replay-nonce", "fake-nonce");
                for (name, value) in headers {
                    response = response.header(name, value);
                }
                let (parts, ()) = response.body(()).unwrap().into_parts();
                Ok(BytesResponse {
                    parts,
                    body: Box::new(Bytes::from(body)),
                })
            })
        }
    }

    fn ok(body: Value) -> Reply {
        (StatusCode::OK, vec![], body.to_string().into_bytes())
    }

    fn order_json(order: &Order) -> Value {
        json!({
            "status": order.status,
            "identifiers": [order.identifier],
            "authorizations": [format!("{BASE}/authz/{}", order.id)],
            "finalize": format!("{BASE}/finalize/{}", order.id),
            "certificate": order.chain.as_ref().map(|_| format!("{BASE}/cert/{}", order.id)),
        })
    }

    fn authz_json(order: &Order) -> Value {
        json!({
            "identifier": order.identifier,
            "status": challenge_status(order),
            "challenges": [challenge_json(order)],
        })
    }

    fn challenge_status(order: &Order) -> &'static str {
        match order.status {
            "pending" | "invalid" => order.status,
            _ => "valid",
        }
    }

    fn challenge_json(order: &Order) -> Value {
        let mut challenge = json!({
            "type": "tls-alpn-01",
            "url": format!("{BASE}/chall/{}", order.id),
            "token": order.token,
            "status": challenge_status(order),
        });
        if order.status == "invalid" {
            challenge["error"] = json!({
                "type": "urn:ietf:params:acme:error:unauthorized",
                "detail": "tls-alpn-01 validation failed",
            });
        }
        challenge
    }

    /// Dial the agent the way a CA validates TLS-ALPN-01: ALPN `acme-tls/1`
    /// only, SNI the host (or its reverse-DNS name for an IP), and accept only
    /// a certificate carrying the critical acmeIdentifier with `digest`.
    async fn validate(port: u16, host: &str, digest: &[u8]) -> bool {
        let sni = match host.parse::<IpAddr>() {
            Ok(IpAddr::V4(ip)) => {
                let [a, b, c, d] = ip.octets();
                format!("{d}.{c}.{b}.{a}.in-addr.arpa")
            }
            Ok(IpAddr::V6(ip)) => {
                let nibbles: String = ip
                    .octets()
                    .iter()
                    .rev()
                    .flat_map(|byte| [byte & 0xf, byte >> 4])
                    .map(|nibble| format!("{nibble:x}."))
                    .collect();
                format!("{nibbles}ip6.arpa")
            }
            Err(_) => host.to_string(),
        };
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate))
        .with_no_client_auth();
        config.alpn_protocols = vec![ACME_TLS_ALPN.to_vec()];
        let Ok(tcp) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await else {
            return false;
        };
        let Ok(tls) = tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from(sni).unwrap(), tcp)
            .await
        else {
            return false;
        };
        let connection = tls.get_ref().1;
        let Some(leaf) = connection
            .peer_certificates()
            .and_then(|chain| chain.first())
        else {
            return false;
        };
        // id-pe-acmeIdentifier, critical TRUE, OCTET STRING { OCTET STRING digest }.
        let mut extension = vec![
            0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x1f, 0x01, 0x01, 0xff, 0x04,
            0x22, 0x04, 0x20,
        ];
        extension.extend_from_slice(digest);
        connection.alpn_protocol() == Some(ACME_TLS_ALPN)
            && leaf
                .as_ref()
                .windows(extension.len())
                .any(|window| window == extension)
    }

    /// The challenge certificate is self-signed and carries a critical extension
    /// webpki refuses to parse; a validator checks its content instead, so this
    /// accepts the certificate and its handshake signature.
    #[derive(Debug)]
    struct AnyCertificate;

    impl ServerCertVerifier for AnyCertificate {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    /// The subjectPublicKey of a PKCS#10 CSR: CertificationRequest >
    /// CertificationRequestInfo > (version, subject, SubjectPublicKeyInfo).
    fn csr_public_key(csr: &[u8]) -> Option<Vec<u8>> {
        let (_, request, _) = der(csr)?;
        let (_, info, _) = der(request)?;
        let (_, _, rest) = der(info)?;
        let (_, _, rest) = der(rest)?;
        let (_, spki, _) = der(rest)?;
        let (_, _, rest) = der(spki)?;
        let (_, bits, _) = der(rest)?;
        Some(bits.get(1..)?.to_vec())
    }

    /// instant-acme's CSR key: ECDSA P-256 (rcgen's default).
    struct CsrKey(Vec<u8>);

    impl rcgen::PublicKeyData for CsrKey {
        fn der_bytes(&self) -> &[u8] {
            &self.0
        }

        fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
            &rcgen::PKCS_ECDSA_P256_SHA256
        }
    }

    fn jws(body: &[u8]) -> (Value, Value) {
        let jws: Value = serde_json::from_slice(body).unwrap_or_default();
        let part = |name: &str| {
            serde_json::from_slice(&b64url_decode(jws[name].as_str().unwrap_or_default()))
                .unwrap_or_default()
        };
        (part("protected"), part("payload"))
    }

    /// The RFC 7638 thumbprint of an EC JWK.
    fn thumbprint(jwk: &Value) -> String {
        let canonical = format!(
            r#"{{"crv":{},"kty":{},"x":{},"y":{}}}"#,
            jwk["crv"], jwk["kty"], jwk["x"], jwk["y"]
        );
        b64url_encode(&sha256(canonical.as_bytes()))
    }

    fn sha256(bytes: &[u8]) -> Vec<u8> {
        let hex = shared::protocol::sha256_hex(bytes);
        (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
            .collect()
    }

    const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    fn b64url_encode(bytes: &[u8]) -> String {
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, &byte)| n | u32::from(byte) << (16 - 8 * i));
            for i in 0..=chunk.len() {
                out.push(char::from(B64URL[(n >> (18 - 6 * i) & 63) as usize]));
            }
        }
        out
    }

    fn b64url_decode(text: &str) -> Vec<u8> {
        let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0);
        for value in text
            .bytes()
            .filter_map(|c| B64URL.iter().position(|&b| b == c))
        {
            acc = acc << 6 | value as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        out
    }

    /// A data plane serving `origin` against `ca`, on a fresh loopback port.
    pub(crate) struct Node {
        pub(crate) addr: SocketAddr,
        pub(crate) status: watch::Receiver<Option<CertificateStatus>>,
        pub(crate) origin: watch::Sender<Option<String>>,
        task: tokio::task::JoinHandle<std::io::Result<()>>,
    }

    impl Node {
        pub(crate) async fn start(ca: &FakeCa, state_dir: &Path, origin: &str) -> Self {
            Self::start_with(ca, origin, ca.acme(state_dir)).await
        }

        /// [`Node::start`] with `acme` tuned by the caller.
        pub(crate) async fn start_with(ca: &FakeCa, origin: &str, acme: Acme) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            ca.state().port = addr.port();
            let (origin, origin_rx) = watch::channel(Some(origin.to_string()));
            let (status_tx, status) = watch::channel(None);
            let task = tokio::spawn(crate::dataplane::serve_https(
                listener,
                acme.state_dir.join("files"),
                origin_rx,
                status_tx,
                acme,
            ));
            Self {
                addr,
                status,
                origin,
                task,
            }
        }

        /// Wait (bounded) for a certificate status that satisfies `accept`.
        pub(crate) async fn status_until(
            &mut self,
            accept: impl Fn(&CertificateStatus) -> bool,
        ) -> CertificateStatus {
            let status = tokio::time::timeout(
                Duration::from_secs(20),
                self.status
                    .wait_for(|status| status.as_ref().is_some_and(&accept)),
            )
            .await
            .expect("the expected certificate status within 20 s")
            .expect("the data plane is running");
            status.clone().unwrap()
        }

        /// One HTTPS exchange as a browser trusting only `ca`: the served leaf
        /// and the raw response.
        pub(crate) async fn https(
            &self,
            ca: &FakeCa,
            host: &str,
            request: &[u8],
        ) -> (CertificateDer<'static>, Vec<u8>) {
            let tcp = tokio::net::TcpStream::connect(self.addr).await.unwrap();
            let mut tls = tokio_rustls::TlsConnector::from(Arc::new(ca.client_config()))
                .connect(ServerName::try_from(host.to_string()).unwrap(), tcp)
                .await
                .expect("a TLS handshake trusted through the test CA");
            let leaf = tls.get_ref().1.peer_certificates().unwrap()[0].clone();
            tls.write_all(request).await.unwrap();
            let mut reply = Vec::new();
            let _ = tls.read_to_end(&mut reply).await;
            (leaf, reply)
        }

        /// Stop the node as a restart would: its origin ends and its task stops.
        pub(crate) async fn stop(self) {
            drop(self.origin);
            self.task.abort();
            let _ = self.task.await;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "lg-agent-acme-{tag}-{}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            unix_now()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn base64url_round_trips() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\xff\xfe\x00"] {
            assert_eq!(b64url_decode(&b64url_encode(bytes)), bytes);
        }
        assert_eq!(b64url_encode(b"\xfb\xff"), "-_8");
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{temp_dir, FakeCa, Node, State};
    use super::*;
    use serde_json::json;

    const DAY: u64 = 86_400;
    const HOUR: u64 = 3_600;

    fn issued(status: &CertificateStatus) -> bool {
        status.issued_at.is_some() && status.last_error.is_none()
    }

    #[cfg(unix)]
    fn assert_owner_only(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{} must be owner-only", path.display());
    }

    // An IP origin is ordered with the `shortlived` profile and an Ip
    // identifier, proven over TLS-ALPN-01 on the data-plane listener itself, and
    // the account key and certificate are stored owner-only in the state dir.
    #[tokio::test]
    async fn an_ip_origin_gets_a_shortlived_ip_certificate_through_tls_alpn_01() {
        let ca = FakeCa::new(6 * DAY);
        let dir = temp_dir("ip");
        let mut node = Node::start(&ca, &dir, "https://127.0.0.1").await;

        let status = node.status_until(issued).await;
        assert_eq!(
            status.expires_at.unwrap() - status.issued_at.unwrap(),
            6 * DAY
        );
        {
            let state = ca.state();
            assert_eq!(
                state.orders[0].1,
                json!({"identifiers": [{"type": "ip", "value": "127.0.0.1"}], "profile": "shortlived"})
            );
            assert_eq!(
                state.validations,
                [true],
                "one TLS-ALPN-01 validation passed"
            );
            assert_eq!(state.accounts, 1);
        }
        #[cfg(unix)]
        for file in [ACCOUNT_FILE, CERTIFICATE_FILE] {
            assert_owner_only(&dir.join(file));
        }
        let (_, reply) = node
            .https(
                &ca,
                "127.0.0.1",
                b"GET /files/none HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
            )
            .await;
        assert!(reply.starts_with(b"HTTP/1.1 404"), "{reply:?}");
        node.stop().await;
    }

    // A DNS origin is ordered with the CA's default profile and a Dns
    // identifier.
    #[tokio::test]
    async fn a_dns_origin_gets_a_default_profile_dns_certificate() {
        let ca = FakeCa::new(90 * DAY);
        let dir = temp_dir("dns");
        let mut node = Node::start(&ca, &dir, "https://node.example.test").await;

        node.status_until(issued).await;
        assert_eq!(
            ca.state().orders[0].1,
            json!({"identifiers": [{"type": "dns", "value": "node.example.test"}]})
        );
        assert_eq!(ca.state().validations, [true]);
        let (_, reply) = node
            .https(
                &ca,
                "node.example.test",
                b"GET /files/none HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
            )
            .await;
        assert!(reply.starts_with(b"HTTP/1.1 404"), "{reply:?}");
        node.stop().await;
    }

    // A restart serves the cached certificate without a new order, and
    // a new certificate reuses the cached account instead of registering again.
    #[tokio::test]
    async fn a_restart_reuses_the_cached_account_and_certificate() {
        let ca = FakeCa::new(90 * DAY);
        let dir = temp_dir("restart");
        let mut first = Node::start(&ca, &dir, "https://127.0.0.1").await;
        let before = first.status_until(issued).await;
        first.stop().await;

        let mut restarted = Node::start(&ca, &dir, "https://127.0.0.1").await;
        assert_eq!(restarted.status_until(issued).await, before);
        let (served, _) = restarted
            .https(
                &ca,
                "127.0.0.1",
                b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
            )
            .await;
        assert_eq!(validity(served.as_ref()).map(|v| v.1), before.expires_at);
        assert_eq!(
            ca.state().orders.len(),
            1,
            "the cached certificate is reused"
        );
        restarted.stop().await;

        std::fs::remove_file(dir.join(CERTIFICATE_FILE)).unwrap();
        let mut again = Node::start(&ca, &dir, "https://127.0.0.1").await;
        again.status_until(issued).await;
        {
            let state = ca.state();
            assert_eq!(
                state.orders.len(),
                2,
                "a missing certificate is ordered again"
            );
            assert_eq!(state.accounts, 1, "the cached account key is reused");
        }
        again.stop().await;
    }

    // With a third of its lifetime left the certificate is renewed and
    // the listener serves the new one at once, without a restart. F-350: the
    // first certificate is cached, not issued, and the renewal lasts 90 days, so
    // a slow issuance on a loaded host cannot make either due on arrival.
    #[tokio::test]
    async fn renewal_hot_swaps_the_served_certificate_when_a_third_of_its_life_remains() {
        let ca = FakeCa::new(90 * DAY);
        let dir = temp_dir("renew");
        let now = unix_now();
        // Six seconds long: a third remains 2 s from now.
        ca.cache(&dir, "127.0.0.1", now - 2, now + 4);
        let mut node = Node::start(&ca, &dir, "https://127.0.0.1").await;

        let renewed = node
            .status_until(|status| issued(status) && status.issued_at > Some(now - 2))
            .await;
        assert!(
            renewed.issued_at.unwrap() >= now + 2,
            "renewal waits until a third of the 6 s lifetime remains: {renewed:?}"
        );
        let (renewed_leaf, _) = node
            .https(
                &ca,
                "127.0.0.1",
                b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
            )
            .await;
        assert_eq!(
            validity(renewed_leaf.as_ref()).map(|v| v.1),
            renewed.expires_at,
            "the same listener serves the renewed certificate"
        );
        node.stop().await;
    }

    // A failed issuance is reported with its error and retried with a
    // doubling backoff, and a later success clears the error.
    #[tokio::test]
    async fn a_failed_issuance_reports_the_error_and_backs_off() {
        let ca = FakeCa::new(90 * DAY);
        ca.state().fail_orders = 2;
        let dir = temp_dir("backoff");
        let mut node = Node::start(&ca, &dir, "https://127.0.0.1").await;

        let failed = node
            .status_until(|status| status.last_error.is_some())
            .await;
        assert!(
            failed
                .last_error
                .as_deref()
                .unwrap()
                .contains("too many new orders"),
            "{failed:?}"
        );
        assert_eq!(failed.issued_at, None);
        node.status_until(issued).await;
        let at: Vec<_> = ca.state().orders.iter().map(|(at, _)| *at).collect();
        assert_eq!(at.len(), 3);
        assert!(
            at[1] - at[0] >= Duration::from_millis(100),
            "first retry after 100 ms"
        );
        assert!(
            at[2] - at[1] >= Duration::from_millis(200),
            "then after 200 ms"
        );
        node.stop().await;
    }

    /// Wait (bounded) until the CA has seen `count` new orders.
    async fn orders_reach(ca: &FakeCa, count: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while ca.state().orders.len() < count {
            assert!(
                std::time::Instant::now() < deadline,
                "{count} orders within 10 s"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    // A renewal the CA answers with something that is not a certificate leaves
    // the cached, still-valid certificate on disk and serving, and reports the
    // failure.
    #[tokio::test]
    async fn an_unusable_renewal_keeps_the_still_valid_certificate() {
        let ca = FakeCa::new(90 * DAY);
        ca.state().bad_chain = true;
        let dir = temp_dir("unusable");
        let now = unix_now();
        // Six days long with one left: due for renewal and still valid.
        ca.cache(&dir, "127.0.0.1", now - 5 * DAY, now + DAY);
        let mut node = Node::start(&ca, &dir, "https://127.0.0.1").await;

        let failed = node
            .status_until(|status| status.last_error.is_some())
            .await;
        assert!(
            failed.last_error.as_deref().unwrap().contains("unusable"),
            "{failed:?}"
        );
        assert_eq!(
            read_json::<Stored>(&dir.join(CERTIFICATE_FILE))
                .and_then(Issued::parse)
                .map(|cached| cached.not_after),
            Some(now + DAY),
            "the cached certificate is kept"
        );
        // Two more failed passes later it still serves and is still reported.
        orders_reach(&ca, 3).await;
        assert_eq!(node.status.borrow().as_ref(), Some(&failed));
        let (served, _) = node
            .https(
                &ca,
                "127.0.0.1",
                b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
            )
            .await;
        assert_eq!(validity(served.as_ref()).map(|v| v.1), Some(now + DAY));
        node.stop().await;
    }

    // A new certificate that is already due for renewal (the agent's clock
    // runs days ahead) is served and reported, and not ordered again even
    // though the failure retry is only 100 ms.
    #[tokio::test]
    async fn a_certificate_due_on_arrival_backs_off_instead_of_reordering() {
        let ca = FakeCa::new(90 * DAY);
        let now = unix_now();
        // Six days long with one left, as a clock five days fast sees it.
        ca.state().window = Some((now - 5 * DAY, now + DAY));
        let dir = temp_dir("due");
        let mut node = Node::start(&ca, &dir, "https://127.0.0.1").await;

        let status = node.status_until(|status| status.issued_at.is_some()).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            ca.state().orders.len(),
            1,
            "no new order within 2 s of a due-on-arrival certificate"
        );
        assert!(status.last_error.is_some(), "{status:?}");
        assert_eq!(status.expires_at, Some(now + DAY), "{status:?}");
        node.stop().await;
    }

    // However short the failure retry, a new certificate the agent's clock
    // makes due, expired or not yet valid on arrival is not ordered again for
    // twelve hours: clock skew costs at most two duplicate orders a day.
    #[tokio::test]
    async fn clock_skew_waits_twelve_hours_before_ordering_again() {
        let now = unix_now();
        for window in [
            (now - 5 * DAY, now + DAY),
            (now - 7 * DAY, now - DAY),
            (now + DAY + HOUR, now + 7 * DAY),
        ] {
            let ca = FakeCa::new(90 * DAY);
            ca.state().window = Some(window);
            let resolver = Arc::new(CertResolver::default());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            ca.state().port = listener.local_addr().unwrap().port();
            let acceptor =
                tokio_rustls::TlsAcceptor::from(Arc::new(server_config(Arc::clone(&resolver))));
            let validations = tokio::spawn(async move {
                while let Ok((tcp, _)) = listener.accept().await {
                    let _ = acceptor.accept(tcp).await;
                }
            });
            let (status, _) = watch::channel(None);
            let acme = ca.acme(&temp_dir("skew"));
            let wait = ensure(&acme, "127.0.0.1", &resolver, &status, &mut 0, || true).await;
            validations.abort();
            assert!(
                wait >= Duration::from_secs(12 * 3600),
                "{window:?}: next order in {wait:?} after {:?}",
                status.borrow()
            );
            assert_eq!(ca.state().orders.len(), 1);
        }
    }

    // An agent clock two hours slow sees a fresh certificate as not yet
    // valid; browsers use their own clocks, so it is stored and served, with
    // no error and no clock backoff.
    #[tokio::test]
    async fn a_certificate_valid_soon_on_a_slow_clock_is_served() {
        let now = unix_now();
        let ca = FakeCa::new(90 * DAY);
        ca.state().window = Some((now + 2 * HOUR, now + 2 * HOUR + 90 * DAY));
        let resolver = Arc::new(CertResolver::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        ca.state().port = listener.local_addr().unwrap().port();
        let acceptor =
            tokio_rustls::TlsAcceptor::from(Arc::new(server_config(Arc::clone(&resolver))));
        let validations = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let _ = acceptor.accept(tcp).await;
            }
        });
        let (status, _) = watch::channel(None);
        let dir = temp_dir("slow");
        let acme = ca.acme(&dir);
        let wait = ensure(&acme, "127.0.0.1", &resolver, &status, &mut 0, || true).await;
        validations.abort();

        let reported = status.borrow().clone().unwrap();
        assert_eq!(reported.last_error, None, "{reported:?}");
        assert_eq!(reported.issued_at, Some(now + 2 * HOUR), "{reported:?}");
        assert!(
            wait > Duration::from_secs(50 * DAY),
            "next pass in {wait:?}"
        );
        let served = resolver.served.read().unwrap().clone();
        assert_eq!(
            served.and_then(|key| validity(key.end_entity_cert().ok()?.as_ref())),
            Some((now + 2 * HOUR, now + 2 * HOUR + 90 * DAY))
        );
        assert!(read_json::<Stored>(&dir.join(CERTIFICATE_FILE)).is_some());
        assert_eq!(ca.state().orders.len(), 1);
    }

    /// The CA answers the renewal of a still-valid cached certificate for
    /// `host` with one `spoil` makes wrong: it is refused before it is stored,
    /// the cached certificate keeps serving, and the status names `reason`.
    async fn a_spoiled_renewal_is_refused(
        host: &str,
        spoil: impl FnOnce(&mut State),
        reason: &str,
    ) {
        let ca = FakeCa::new(90 * DAY);
        spoil(&mut ca.state());
        let dir = temp_dir("spoiled");
        let now = unix_now();
        // Six days long with one left: due for renewal and still valid.
        ca.cache(&dir, host, now - 5 * DAY, now + DAY);
        let mut node = Node::start(&ca, &dir, &format!("https://{host}")).await;

        let status = node
            .status_until(|status| {
                status.last_error.is_some() || status.expires_at != Some(now + DAY)
            })
            .await;
        assert_eq!(
            read_json::<Stored>(&dir.join(CERTIFICATE_FILE))
                .and_then(Issued::parse)
                .map(|cached| cached.not_after),
            Some(now + DAY),
            "the still-valid cached certificate is kept: {status:?}"
        );
        assert_eq!(status.expires_at, Some(now + DAY), "{status:?}");
        assert!(
            status
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains(reason)),
            "the status names {reason:?}: {status:?}"
        );
        let (served, _) = node
            .https(
                &ca,
                host,
                b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
            )
            .await;
        assert_eq!(validity(served.as_ref()).map(|v| v.1), Some(now + DAY));
        node.stop().await;
    }

    #[tokio::test]
    async fn a_certificate_on_another_key_is_refused() {
        a_spoiled_renewal_is_refused("127.0.0.1", |ca| ca.other_key = true, "order key").await;
    }

    #[tokio::test]
    async fn a_chain_that_does_not_start_with_the_leaf_is_refused() {
        a_spoiled_renewal_is_refused("127.0.0.1", |ca| ca.root_first = true, "order key").await;
    }

    #[tokio::test]
    async fn a_certificate_for_another_name_is_refused() {
        a_spoiled_renewal_is_refused(
            "node.example.test",
            |ca| ca.other_host = Some("other.example.test".into()),
            "not for node.example.test",
        )
        .await;
    }

    #[tokio::test]
    async fn a_certificate_for_another_ip_is_refused() {
        a_spoiled_renewal_is_refused(
            "127.0.0.1",
            |ca| ca.other_host = Some("127.0.0.2".into()),
            "not for 127.0.0.1",
        )
        .await;
    }

    #[tokio::test]
    async fn an_expired_certificate_is_refused() {
        let now = unix_now();
        a_spoiled_renewal_is_refused(
            "127.0.0.1",
            |ca| ca.window = Some((now - 7 * DAY, now - DAY)),
            "not valid now",
        )
        .await;
    }

    #[tokio::test]
    async fn a_certificate_not_yet_valid_is_refused() {
        let now = unix_now();
        a_spoiled_renewal_is_refused(
            "127.0.0.1",
            |ca| ca.window = Some((now + DAY + HOUR, now + 7 * DAY)),
            "not valid now",
        )
        .await;
    }

    #[tokio::test]
    async fn a_certificate_that_ends_before_it_starts_is_refused() {
        let now = unix_now();
        a_spoiled_renewal_is_refused(
            "127.0.0.1",
            |ca| ca.window = Some((now + 10 * HOUR, now + 5 * HOUR)),
            "no validity period",
        )
        .await;
    }

    #[tokio::test]
    async fn a_zero_length_certificate_is_refused() {
        let now = unix_now();
        a_spoiled_renewal_is_refused(
            "127.0.0.1",
            |ca| ca.window = Some((now + 2 * HOUR, now + 2 * HOUR)),
            "no validity period",
        )
        .await;
    }

    // A certificate with no validity period is the CA's fault, not clock
    // skew: it takes the normal failure backoff, not the twelve-hour wait.
    #[tokio::test]
    async fn a_certificate_with_no_validity_period_takes_the_failure_backoff() {
        let now = unix_now();
        for window in [
            (now + 10 * HOUR, now + 5 * HOUR),
            (now + 2 * HOUR, now + 2 * HOUR),
        ] {
            let ca = FakeCa::new(90 * DAY);
            ca.state().window = Some(window);
            let resolver = Arc::new(CertResolver::default());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            ca.state().port = listener.local_addr().unwrap().port();
            let acceptor =
                tokio_rustls::TlsAcceptor::from(Arc::new(server_config(Arc::clone(&resolver))));
            let validations = tokio::spawn(async move {
                while let Ok((tcp, _)) = listener.accept().await {
                    let _ = acceptor.accept(tcp).await;
                }
            });
            let (status, _) = watch::channel(None);
            let acme = ca.acme(&temp_dir("empty"));
            let mut failures = 0;
            let wait = ensure(
                &acme,
                "127.0.0.1",
                &resolver,
                &status,
                &mut failures,
                || true,
            )
            .await;
            validations.abort();
            assert_eq!(
                (wait, failures),
                (acme.first_retry, 1),
                "{window:?}: {:?}",
                status.borrow()
            );
        }
    }

    // F-207: a pass that outlives its origin (another was assigned while its
    // order hung) reports nothing, so its status never stands for the new
    // origin; the next pass reports for the new one. F-350: the clock stays
    // paused until the origin changes (the hung order needs no I/O), so the 1 s
    // deadline cannot pass first on a loaded host.
    #[tokio::test(start_paused = true)]
    async fn a_pass_that_outlives_its_origin_reports_nothing() {
        let ca = FakeCa::new(90 * DAY);
        ca.state().hang_orders = true;
        let acme = Acme {
            timeout: Duration::from_secs(1),
            ..ca.acme(&temp_dir("outlived"))
        };
        let mut node = Node::start_with(&ca, "https://127.0.0.1", acme).await;
        orders_reach(&ca, 1).await;
        node.origin
            .send(Some("https://node.example.test".into()))
            .unwrap();
        // The new origin's order is validated over real sockets.
        tokio::time::resume();
        ca.state().hang_orders = false;
        let first = node.status_until(|_| true).await;
        assert!(
            issued(&first),
            "the replaced origin's pass must not report: {first:?}"
        );
        assert_eq!(
            ca.state().orders.last().unwrap().1["identifiers"][0]["value"],
            "node.example.test"
        );
        node.stop().await;
    }

    // A CA that never answers is abandoned at the deadline: the error is
    // reported, the order retried after the backoff, and an origin change
    // still followed.
    #[tokio::test]
    async fn a_ca_that_never_answers_times_out_and_origin_changes_still_apply() {
        let ca = FakeCa::new(90 * DAY);
        ca.state().hang_orders = true;
        let dir = temp_dir("hang");
        let acme = Acme {
            timeout: Duration::from_secs(2),
            ..ca.acme(&dir)
        };
        let mut node = Node::start_with(&ca, "https://127.0.0.1", acme).await;

        let failed = node
            .status_until(|status| status.last_error.is_some())
            .await;
        assert!(
            failed
                .last_error
                .as_deref()
                .unwrap()
                .contains("did not answer"),
            "{failed:?}"
        );
        orders_reach(&ca, 2).await;
        ca.state().hang_orders = false;
        node.origin
            .send(Some("https://node.example.test".into()))
            .unwrap();
        node.status_until(issued).await;
        assert_eq!(
            ca.state().orders.last().unwrap().1["identifiers"][0]["value"],
            "node.example.test"
        );
        node.stop().await;
    }

    #[test]
    fn renewal_starts_when_a_third_of_the_lifetime_remains() {
        assert_eq!(renew_at(0, 90), 60);
        assert_eq!(renew_at(1_000, 1_000 + 6 * DAY), 1_000 + 4 * DAY);
    }

    #[test]
    fn retries_double_from_the_first_delay_up_to_an_hour() {
        let first = Duration::from_secs(60);
        assert_eq!(retry_after(first, 1), first);
        assert_eq!(retry_after(first, 2), first * 2);
        assert_eq!(retry_after(first, 4), first * 8);
        assert_eq!(retry_after(first, 40), MAX_RETRY);
    }

    #[test]
    fn origin_host_drops_scheme_port_and_ipv6_brackets() {
        assert_eq!(
            origin_host("https://node.example.test").as_deref(),
            Some("node.example.test")
        );
        assert_eq!(
            origin_host("https://node.example.test:443").as_deref(),
            Some("node.example.test")
        );
        assert_eq!(
            origin_host("https://203.0.113.10").as_deref(),
            Some("203.0.113.10")
        );
        assert_eq!(
            origin_host("https://[2001:db8::10]:443").as_deref(),
            Some("2001:db8::10")
        );
        assert_eq!(origin_host("http://node.example.test"), None);
        assert_eq!(origin_host("https://"), None);
    }

    #[test]
    fn der_times_read_utc_and_generalized_time() {
        // 2026-09-26T09:44:25Z and 2050-01-01T00:00:00Z.
        assert_eq!(der_time(0x17, b"260926094425Z"), Some(1_790_415_865));
        assert_eq!(der_time(0x18, b"20500101000000Z"), Some(2_524_608_000));
        assert_eq!(der_time(0x17, b"2609260944Z"), None);
        assert_eq!(der_time(0x04, b"260926094425Z"), None);
    }

    #[test]
    fn validity_reads_not_before_and_not_after() {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["node.example.test".into()]).unwrap();
        params.not_before = rcgen::date_time_ymd(2026, 9, 26);
        params.not_after = rcgen::date_time_ymd(2051, 1, 1);
        let cert = params.self_signed(&key).unwrap();
        assert_eq!(validity(cert.der()), Some((1_790_380_800, 2_556_144_000)));
        assert_eq!(validity(b"\x30\x03\x02\x01\x00"), None);
    }
}
