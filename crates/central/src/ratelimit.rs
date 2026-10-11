//! Client identity derived from a *configured* trusted-proxy boundary, plus the
//! login rate limiter keyed on it. The whole point (FR-074/AC39): a forwarded
//! header is honoured only when it arrives from a proxy the operator configured
//! as trusted, so a spoofed `X-Forwarded-For` from an untrusted peer cannot
//! change the attacker's rate-limit key or forge a "secure transport" claim.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Mutex, OnceLock};
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
    /// until a proxy is configured (fail closed). An entry that is not an IP
    /// (a CIDR range, a host name) trusts nothing and is logged. Read once per
    /// process: startup calls this for the app state and the listener, and each
    /// ignored entry is logged once.
    pub fn from_env() -> Self {
        static CONFIG: OnceLock<TransportConfig> = OnceLock::new();
        CONFIG
            .get_or_init(|| Self::parse(&std::env::var("LG_TRUSTED_PROXIES").unwrap_or_default()))
            .clone()
    }

    fn parse(raw: &str) -> Self {
        let trusted_proxies = raw
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .filter_map(|entry| {
                entry
                    .parse::<IpAddr>()
                    .inspect_err(|error| {
                        tracing::warn!(
                            entry,
                            %error,
                            "LG_TRUSTED_PROXIES entry ignored: list single IP addresses \
                             (CIDR ranges and host names are not supported)"
                        );
                    })
                    .ok()
            })
            .collect();
        Self { trusted_proxies }
    }

    fn is_trusted(&self, ip: &IpAddr) -> bool {
        self.trusted_proxies.contains(ip)
    }

    /// The key the web listener counts a peer's connections under: its
    /// [`limiter_key`], or `None` for a trusted proxy, which carries many clients.
    pub(crate) fn connection_key(&self, peer: IpAddr) -> Option<IpAddr> {
        (!self.is_trusted(&peer)).then(|| limiter_key(peer))
    }

    /// The key a trusted proxy's forwarded client is counted under, as
    /// [`Self::connection_key`] counts a direct peer: `None` unless `peer` is a
    /// trusted proxy that named a client in `X-Forwarded-For`.
    pub(crate) fn forwarded_client_key(&self, peer: IpAddr, headers: &HeaderMap) -> Option<IpAddr> {
        self.client_ip(Some(peer), headers)
            .filter(|client| *client != peer)
            .map(limiter_key)
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
        for hop in forwarded_values(headers, XFF).rev() {
            match hop.and_then(parse_hop) {
                Some(candidate) if self.is_trusted(&candidate) => continue,
                Some(candidate) => return Some(candidate),
                // Everything left of an unreadable hop is client-controlled.
                None => break,
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
        // Every value on every field line must say https: a client-sent value
        // cannot be told apart from a proxy-appended one, so any other value
        // (or an unreadable line) fails closed.
        let mut protos = forwarded_values(headers, XFP).peekable();
        protos.peek().is_some()
            && protos.all(|proto| proto.is_some_and(|p| p.eq_ignore_ascii_case("https")))
    }
}

/// Every comma-separated element of every `name` field line, in wire order
/// (RFC 9110 5.3), skipping empty elements. An element that is not UTF-8
/// yields `None`; it never hides the readable elements beside it, such as the
/// hop a proxy appended after a client's bytes.
fn forwarded_values<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
) -> impl DoubleEndedIterator<Item = Option<&'a str>> {
    headers.get_all(name).iter().flat_map(|line| {
        line.as_bytes()
            .split(|&byte| byte == b',')
            .map(|v| std::str::from_utf8(v).ok().map(str::trim))
            .filter(|v| *v != Some(""))
            .collect::<Vec<_>>()
    })
}

/// One `X-Forwarded-For` hop: `ip`, `ipv4:port`, `[ipv6]` or `[ipv6]:port`.
fn parse_hop(hop: &str) -> Option<IpAddr> {
    hop.parse()
        .ok()
        .or_else(|| hop.parse::<SocketAddr>().ok().map(|addr| addr.ip()))
        .or_else(|| hop.strip_prefix('[')?.strip_suffix(']')?.parse().ok())
}

/// The rate-limit key for a client: IPv4 (plain or IPv4-mapped) as-is, native
/// IPv6 by its /64, the block one subscriber is normally given.
fn limiter_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (u128::MAX << 64))),
        v4 => v4,
    }
}

struct Window {
    count: u32,
    start: Instant,
}

/// Fixed-window per-client counters: the one limiter shared by login, the exec
/// rate limit and tunnel pre-auth. Keyed by [`limiter_key`], so a client cannot
/// get a fresh window per address inside its IPv6 /64. The caller owns the lock.
/// Expired windows are pruned only once the map has doubled since the last
/// prune, so a flood of fresh keys costs amortised O(1) per hit, not a full scan.
pub(crate) struct IpWindows {
    windows: HashMap<IpAddr, Window>,
    prune_at: usize,
    #[cfg(test)]
    scans: usize,
}

impl Default for IpWindows {
    fn default() -> Self {
        Self {
            windows: HashMap::new(),
            prune_at: LIMITER_PRUNE_THRESHOLD,
            #[cfg(test)]
            scans: 0,
        }
    }
}

impl IpWindows {
    /// Count one hit from `client` at `now`; returns the hits in its current window.
    pub(crate) fn hit(&mut self, client: IpAddr, window: Duration, now: Instant) -> u32 {
        if self.windows.len() > self.prune_at {
            self.windows
                .retain(|_, w| now.duration_since(w.start) < window);
            self.prune_at = LIMITER_PRUNE_THRESHOLD.max(2 * self.windows.len());
            #[cfg(test)]
            {
                self.scans += 1;
            }
        }
        let entry = self.windows.entry(limiter_key(client)).or_insert(Window {
            count: 0,
            start: now,
        });
        if now.duration_since(entry.start) >= window {
            entry.count = 0;
            entry.start = now;
        }
        entry.count += 1;
        entry.count
    }

    /// The hits `client` already has in its current window, without counting one.
    pub(crate) fn count(&self, client: IpAddr, window: Duration, now: Instant) -> u32 {
        self.windows
            .get(&limiter_key(client))
            .filter(|w| now.duration_since(w.start) < window)
            .map_or(0, |w| w.count)
    }

    pub(crate) fn clear(&mut self, client: IpAddr) {
        self.windows.remove(&limiter_key(client));
    }
}

/// Fixed-window per-client login limiter. Keyed on the trusted-proxy client IP,
/// so a spoofed forwarded header lands on the same key and cannot evade the
/// limit. Bounded: stale windows are pruned once the map grows past a threshold.
#[derive(Default)]
pub struct LoginLimiter {
    windows: Mutex<IpWindows>,
}

impl std::fmt::Debug for LoginLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LoginLimiter")
    }
}

impl LoginLimiter {
    /// Record an attempt from `client`; returns `true` while under the limit.
    pub fn allow(&self, client: IpAddr) -> bool {
        self.allow_at(client, Instant::now())
    }

    fn allow_at(&self, client: IpAddr, now: Instant) -> bool {
        let mut windows = self.windows.lock().expect("login limiter mutex");
        windows.hit(client, LOGIN_WINDOW, now) <= LOGIN_MAX_ATTEMPTS
    }

    /// Clear a client's window on a successful login so a legitimate operator is
    /// not locked out by their own earlier typos.
    pub fn clear(&self, client: IpAddr) {
        self.windows
            .lock()
            .expect("login limiter mutex")
            .clear(client);
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

    fn xff_lines(lines: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for line in lines {
            h.append(XFF, HeaderValue::from_str(line).unwrap());
        }
        h
    }

    #[test]
    fn client_ip_walks_xff_right_to_left() {
        // The client is the right-most untrusted hop; everything left of it is
        // client-supplied and must not win (kills the dropped-`.rev()` mutant).
        let cfg = TransportConfig::new([ip("10.0.0.1")]);
        let headers = headers_with(XFF, "198.51.100.7, 203.0.113.9");
        assert_eq!(
            cfg.client_ip(Some(ip("10.0.0.1")), &headers),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn client_ip_reads_every_xff_field_line() {
        // A proxy may append its own field line; a client-sent first line must
        // not change the key.
        let cfg = TransportConfig::new([ip("10.0.0.1")]);
        let peer = Some(ip("10.0.0.1"));
        for spoof in ["1.2.3.4", "5.6.7.8, 9.9.9.9"] {
            let headers = xff_lines(&[spoof, "203.0.113.50"]);
            assert_eq!(cfg.client_ip(peer, &headers), Some(ip("203.0.113.50")));
        }
    }

    #[test]
    fn client_ip_parses_ip_port_hops() {
        let cfg = TransportConfig::new([ip("10.0.0.1"), ip("10.0.0.2")]);
        let peer = Some(ip("10.0.0.1"));
        for (xff, want) in [
            ("9.9.9.9, 203.0.113.50:4711", "203.0.113.50"),
            ("9.9.9.9, [2001:db8::1]:443", "2001:db8::1"),
            ("9.9.9.9, [2001:db8::1]", "2001:db8::1"),
            ("9.9.9.9, 203.0.113.50, 10.0.0.2:80", "203.0.113.50"),
        ] {
            let headers = headers_with(XFF, xff);
            assert_eq!(cfg.client_ip(peer, &headers), Some(ip(want)), "{xff}");
        }
    }

    #[test]
    fn client_ip_stops_at_an_unparseable_hop() {
        // Anything left of a hop we cannot read is client-controlled: fall back
        // to the trusted peer rather than skipping to an attacker's entry.
        let cfg = TransportConfig::new([ip("10.0.0.1")]);
        let peer = Some(ip("10.0.0.1"));
        for lines in [&["9.9.9.9, unknown"][..], &["9.9.9.9", "garbage"]] {
            assert_eq!(cfg.client_ip(peer, &xff_lines(lines)), peer, "{lines:?}");
        }
        // Empty list elements are ignored (RFC 9110 5.6.1).
        let headers = headers_with(XFF, "203.0.113.9, ");
        assert_eq!(cfg.client_ip(peer, &headers), Some(ip("203.0.113.9")));
    }

    #[test]
    fn tls_attested_requires_https_on_every_xfp_line() {
        let cfg = TransportConfig::new([ip("10.0.0.1")]);
        let peer = Some(ip("10.0.0.1"));
        let xfp = |lines: &[&str]| {
            let mut h = HeaderMap::new();
            for line in lines {
                h.append(XFP, HeaderValue::from_str(line).unwrap());
            }
            h
        };
        assert!(!cfg.tls_attested(peer, &xfp(&["https", "http"])));
        assert!(!cfg.tls_attested(peer, &xfp(&["https, http"])));
        assert!(!cfg.tls_attested(peer, &xfp(&["http", "https"])));
        assert!(cfg.tls_attested(peer, &xfp(&["https", "HTTPS"])));
        assert!(cfg.tls_attested(peer, &xfp(&["https, https"])));
    }

    // F-241: a trusted proxy appends the client it saw to whatever the client
    // sent. A client byte that is not UTF-8 must not hide that appended hop, or
    // the request is keyed as the proxy and escapes the per-client cap.
    #[test]
    fn a_non_utf8_forwarded_element_does_not_hide_the_appended_hop() {
        let proxy = ip("10.0.0.1");
        let cfg = TransportConfig::new([proxy]);
        let raw = |bytes: &[u8]| {
            let mut h = HeaderMap::new();
            h.insert(XFF, HeaderValue::from_bytes(bytes).unwrap());
            h
        };
        let headers = raw(b"\xff, 203.0.113.9");
        assert_eq!(
            cfg.client_ip(Some(proxy), &headers),
            Some(ip("203.0.113.9"))
        );
        assert_eq!(
            cfg.forwarded_client_key(proxy, &headers),
            Some(ip("203.0.113.9"))
        );
        // An unreadable element is still a stop: nothing left of it is used.
        let headers = raw(b"203.0.113.9, \xff");
        assert_eq!(cfg.client_ip(Some(proxy), &headers), Some(proxy));
        let mut headers = HeaderMap::new();
        headers.insert(XFP, HeaderValue::from_bytes(b"https, \xff").unwrap());
        assert!(!cfg.tls_attested(Some(proxy), &headers));
    }

    #[test]
    fn limiter_keys_ipv6_by_64() {
        // 200 addresses in one /64 share one budget.
        let limiter = LoginLimiter::default();
        let allowed = (1..=200u16)
            .filter(|i| limiter.allow(ip(&format!("2001:db8:0:1::{i:x}"))))
            .count();
        assert_eq!(allowed, LOGIN_MAX_ATTEMPTS as usize);
        // A different /64 has its own budget.
        assert!(limiter.allow(ip("2001:db8:0:2::1")));
        // IPv4 clients (plain or IPv4-mapped) are never lumped together.
        for _ in 0..LOGIN_MAX_ATTEMPTS {
            assert!(limiter.allow(ip("198.51.100.7")));
        }
        assert!(!limiter.allow(ip("::ffff:198.51.100.7")));
        assert!(limiter.allow(ip("::ffff:198.51.100.8")));
    }

    #[test]
    fn connection_key_groups_ipv6_by_64_and_exempts_trusted_proxies() {
        let cfg = TransportConfig::new([ip("10.0.0.1")]);
        assert_eq!(cfg.connection_key(ip("10.0.0.1")), None);
        assert_eq!(
            cfg.connection_key(ip("198.51.100.7")),
            Some(ip("198.51.100.7"))
        );
        assert_eq!(
            cfg.connection_key(ip("2001:db8:1:2::1")),
            cfg.connection_key(ip("2001:db8:1:2:ffff::9"))
        );
        assert_ne!(
            cfg.connection_key(ip("2001:db8:1:2::1")),
            cfg.connection_key(ip("2001:db8:1:3::1"))
        );
    }

    #[test]
    fn limiter_window_resets_after_login_window() {
        let limiter = LoginLimiter::default();
        let client = ip("203.0.113.9");
        let t0 = Instant::now();
        for _ in 0..LOGIN_MAX_ATTEMPTS {
            assert!(limiter.allow_at(client, t0));
        }
        let almost = t0 + LOGIN_WINDOW - Duration::from_millis(1);
        assert!(!limiter.allow_at(client, almost), "still inside the window");
        let later = t0 + LOGIN_WINDOW;
        assert!(limiter.allow_at(client, later), "a new window starts");
        for _ in 1..LOGIN_MAX_ATTEMPTS {
            assert!(limiter.allow_at(client, later));
        }
        assert!(
            !limiter.allow_at(client, later),
            "the new window has the full limit"
        );
    }

    #[test]
    fn count_resets_once_the_window_expires() {
        let window = Duration::from_secs(60);
        let mut windows = IpWindows::default();
        let client = ip("203.0.113.9");
        let t0 = Instant::now();
        windows.hit(client, window, t0);
        windows.hit(client, window, t0);
        let almost = t0 + window - Duration::from_millis(1);
        assert_eq!(windows.count(client, window, almost), 2);
        assert_eq!(
            windows.count(client, window, t0 + window),
            0,
            "an expired window counts nothing"
        );
    }

    #[test]
    fn stale_windows_are_pruned() {
        let window = Duration::from_secs(60);
        let mut windows = IpWindows::default();
        let t0 = Instant::now();
        for i in 0..=LIMITER_PRUNE_THRESHOLD as u32 {
            windows.hit(IpAddr::from(i.to_be_bytes()), window, t0);
        }
        windows.hit(ip("203.0.113.9"), window, t0 + window);
        assert_eq!(windows.windows.len(), 1, "expired windows are dropped");
    }

    #[test]
    fn pruning_is_amortised_under_a_flood_of_fresh_keys() {
        // All keys are fresh, so a prune removes nothing: it must not rescan the
        // whole map on every call.
        let window = Duration::from_secs(60);
        let mut windows = IpWindows::default();
        let now = Instant::now();
        let hits = 10 * LIMITER_PRUNE_THRESHOLD as u32;
        for i in 0..hits {
            windows.hit(IpAddr::from(i.to_be_bytes()), window, now);
        }
        assert_eq!(windows.windows.len(), hits as usize);
        assert!(windows.scans <= 5, "{} full scans", windows.scans);
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

    #[derive(Clone, Default)]
    struct LogSink(std::sync::Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for LogSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // F-279: an entry that is not a single IP (a CIDR, a typo) is still ignored
    // (fail closed), but each one is logged once with the reason.
    #[test]
    fn ignored_trusted_proxy_entries_are_logged_once_each() {
        let logs = LogSink::default();
        let sink = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish();
        let cfg = tracing::subscriber::with_default(subscriber, || {
            TransportConfig::parse(" 192.0.2.1 , 172.18.0.0/16,proxy.local,,")
        });

        assert!(cfg.is_trusted(&ip("192.0.2.1")));
        assert!(!cfg.is_trusted(&ip("172.18.0.1")));
        assert_eq!(cfg.trusted_proxies.len(), 1);
        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert_eq!(logs.lines().count(), 2, "{logs}");
        for entry in ["172.18.0.0/16", "proxy.local"] {
            let line = logs.lines().find(|line| line.contains(entry));
            assert!(
                line.is_some_and(|line| line.contains("WARN") && line.contains("CIDR")),
                "{entry} must be logged with the reason: {logs}"
            );
        }
    }
}
