//! Client identity derived from a *configured* trusted-proxy boundary, plus the
//! login rate limiter keyed on it. The whole point (FR-074/AC39): a forwarded
//! header is honoured only when it arrives from a proxy the operator configured
//! as trusted, so a spoofed `X-Forwarded-For` from an untrusted peer cannot
//! change the attacker's rate-limit key or forge a "secure transport" claim.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

const XFF: &str = "x-forwarded-for";
const XFP: &str = "x-forwarded-proto";

const LOGIN_MAX_ATTEMPTS: u32 = 5;
const LOGIN_WINDOW: Duration = Duration::from_secs(60);
const LIMITER_PRUNE_THRESHOLD: usize = 4096;

/// Which upstream proxies the operator trusts. A forwarded header is only read
/// when the direct peer is one of these; otherwise it is ignored entirely.
#[derive(Clone, Debug, Default)]
pub struct TransportConfig {
    trusted_proxies: HashSet<IpAddr>,
}

impl TransportConfig {
    pub fn new(trusted_proxies: impl IntoIterator<Item = IpAddr>) -> Self {
        Self {
            trusted_proxies: trusted_proxies.into_iter().collect(),
        }
    }

    /// Parse `LG_TRUSTED_PROXIES` (comma-separated IPs). Unset/empty means no
    /// proxy is trusted — forwarded headers are ignored and, since the app
    /// serves cleartext behind a TLS-terminating proxy, admin auth is refused
    /// until a proxy is configured (fail closed).
    pub fn from_env() -> Self {
        let trusted_proxies = std::env::var("LG_TRUSTED_PROXIES")
            .ok()
            .into_iter()
            .flat_map(|raw| {
                raw.split(',')
                    .filter_map(|part| part.trim().parse::<IpAddr>().ok())
                    .collect::<Vec<_>>()
            })
            .collect();
        Self { trusted_proxies }
    }

    fn is_trusted(&self, ip: &IpAddr) -> bool {
        self.trusted_proxies.contains(ip)
    }

    /// The real client IP used as the rate-limit key. When the peer is a trusted
    /// proxy we walk `X-Forwarded-For` from the right, skipping further trusted
    /// hops, and take the first untrusted address as the client. When the peer is
    /// not trusted we ignore the header and use the peer itself, so an attacker
    /// connecting directly cannot rotate their key via a forged header.
    pub fn client_ip(&self, peer: Option<IpAddr>, headers: &HeaderMap) -> Option<IpAddr> {
        let peer = peer?;
        if !self.is_trusted(&peer) {
            return Some(peer);
        }
        if let Some(forwarded) = forwarded_ips(headers) {
            for candidate in forwarded.into_iter().rev() {
                if !self.is_trusted(&candidate) {
                    return Some(candidate);
                }
            }
        }
        Some(peer)
    }

    /// TLS is attested only when a trusted proxy reports it terminated HTTPS on
    /// the external leg. A direct (untrusted) peer can never assert this, so
    /// cleartext admin auth stays refused.
    pub fn tls_attested(&self, peer: Option<IpAddr>, headers: &HeaderMap) -> bool {
        let Some(peer) = peer else {
            return false;
        };
        if !self.is_trusted(&peer) {
            return false;
        }
        headers
            .get(XFP)
            .and_then(|v| v.to_str().ok())
            .map(|proto| proto.eq_ignore_ascii_case("https"))
            .unwrap_or(false)
    }
}

fn forwarded_ips(headers: &HeaderMap) -> Option<Vec<IpAddr>> {
    let raw = headers.get(XFF)?.to_str().ok()?;
    let ips: Vec<IpAddr> = raw
        .split(',')
        .filter_map(|part| part.trim().parse::<IpAddr>().ok())
        .collect();
    (!ips.is_empty()).then_some(ips)
}

struct Window {
    count: u32,
    start: Instant,
}

/// Fixed-window per-client login limiter. Keyed on the trusted-proxy client IP,
/// so a spoofed forwarded header lands on the same key and cannot evade the
/// limit. Bounded: stale windows are pruned once the map grows past a threshold.
#[derive(Default)]
pub struct LoginLimiter {
    windows: Mutex<HashMap<IpAddr, Window>>,
}

impl std::fmt::Debug for LoginLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LoginLimiter")
    }
}

impl LoginLimiter {
    /// Record an attempt from `client`; returns `true` while under the limit.
    pub fn allow(&self, client: IpAddr) -> bool {
        let now = Instant::now();
        let mut windows = self.windows.lock().expect("login limiter mutex");
        if windows.len() > LIMITER_PRUNE_THRESHOLD {
            windows.retain(|_, w| now.duration_since(w.start) < LOGIN_WINDOW);
        }
        let window = windows.entry(client).or_insert(Window {
            count: 0,
            start: now,
        });
        if now.duration_since(window.start) >= LOGIN_WINDOW {
            window.count = 0;
            window.start = now;
        }
        window.count += 1;
        window.count <= LOGIN_MAX_ATTEMPTS
    }

    /// Clear a client's window on a successful login so a legitimate operator is
    /// not locked out by their own earlier typos.
    pub fn clear(&self, client: IpAddr) {
        self.windows
            .lock()
            .expect("login limiter mutex")
            .remove(&client);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn headers_with(name: &'static str, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(name, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn untrusted_peer_ignores_forwarded_header() {
        let cfg = TransportConfig::new([ip("10.0.0.1")]);
        let headers = headers_with(XFF, "203.0.113.9");
        // Peer is not a trusted proxy → the spoofed XFF is ignored; the key is
        // the peer itself, which an attacker cannot change.
        assert_eq!(
            cfg.client_ip(Some(ip("198.51.100.7")), &headers),
            Some(ip("198.51.100.7"))
        );
    }

    #[test]
    fn trusted_proxy_forwarded_client_is_used() {
        let cfg = TransportConfig::new([ip("10.0.0.1")]);
        let headers = headers_with(XFF, "203.0.113.9");
        assert_eq!(
            cfg.client_ip(Some(ip("10.0.0.1")), &headers),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn trusted_proxy_walks_past_further_trusted_hops() {
        let cfg = TransportConfig::new([ip("10.0.0.1"), ip("10.0.0.2")]);
        let headers = headers_with(XFF, "203.0.113.9, 10.0.0.2");
        assert_eq!(
            cfg.client_ip(Some(ip("10.0.0.1")), &headers),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn spoofed_header_cannot_rotate_key_from_untrusted_peer() {
        let cfg = TransportConfig::new([ip("10.0.0.1")]);
        let attacker = ip("198.51.100.7");
        let spoof_a = headers_with(XFF, "1.1.1.1");
        let spoof_b = headers_with(XFF, "2.2.2.2");
        // Two different forged headers from the same untrusted peer resolve to
        // the same key → the attacker cannot spread attempts across keys.
        assert_eq!(
            cfg.client_ip(Some(attacker), &spoof_a),
            cfg.client_ip(Some(attacker), &spoof_b)
        );
    }

    #[test]
    fn tls_attested_only_from_trusted_proxy() {
        let cfg = TransportConfig::new([ip("10.0.0.1")]);
        let https = headers_with(XFP, "https");
        assert!(cfg.tls_attested(Some(ip("10.0.0.1")), &https));
        assert!(!cfg.tls_attested(Some(ip("198.51.100.7")), &https));
        assert!(!cfg.tls_attested(Some(ip("10.0.0.1")), &HeaderMap::new()));
        assert!(!cfg.tls_attested(None, &https));
    }

    #[test]
    fn limiter_blocks_after_max_attempts() {
        let limiter = LoginLimiter::default();
        let client = ip("203.0.113.9");
        for _ in 0..LOGIN_MAX_ATTEMPTS {
            assert!(limiter.allow(client));
        }
        assert!(!limiter.allow(client));
    }

    #[test]
    fn limiter_clear_resets_a_client() {
        let limiter = LoginLimiter::default();
        let client = ip("203.0.113.9");
        for _ in 0..LOGIN_MAX_ATTEMPTS {
            limiter.allow(client);
        }
        limiter.clear(client);
        assert!(limiter.allow(client));
    }
}
