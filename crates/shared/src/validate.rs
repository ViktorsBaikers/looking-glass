//! Target validation — the injection/SSRF trust boundary (risk #2).
//!
//! Untrusted target input (an IP literal or a hostname) crosses this module and
//! becomes a [`ValidatedTarget`] pinned to a single *public* IP, or it is rejected
//! with a reason and no process is ever reached. Everything downstream of this file
//! (`shared::exec`, the local node, the agent) treats a `ValidatedTarget` as trusted.
//!
//! Exhaustiveness is the safety property: a non-public range that slips through is an
//! SSRF hole. The rejection set is closed — an address is public only if it matches
//! none of the special ranges enumerated in [`validate_ip`].

use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// A target that passed validation, pinned to the single public IP a command will run
/// against. The IP is captured at validation time so execution cannot re-resolve a
/// hostname and race a DNS-rebinding attacker (TOCTOU) — the pinned address is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTarget {
    ip: IpAddr,
    input: String,
}

impl ValidatedTarget {
    /// The validated public IP the command runs against.
    pub fn ip(&self) -> IpAddr {
        self.ip
    }

    /// The original untrimmed-then-trimmed user input, for display/logging only —
    /// never passed to a command.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The value to hand a command template as its discrete target argument: the pinned
    /// IP, so the process connects to the validated address rather than re-resolving.
    pub fn arg(&self) -> String {
        self.ip.to_string()
    }
}

/// Why a candidate address is not a valid public diagnostic target. One variant per
/// special range so a caller (and each test) can assert the exact reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// `0.0.0.0/8` or `::` — "this host / unspecified".
    Unspecified,
    /// `127.0.0.0/8` or `::1`.
    Loopback,
    /// `10/8`, `172.16/12`, `192.168/16` (RFC 1918).
    Private,
    /// `169.254.0.0/16` or `fe80::/10`.
    LinkLocal,
    /// `100.64.0.0/10` carrier-grade NAT shared space (RFC 6598).
    SharedCgn,
    /// `192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`, or `2001:db8::/32` (docs).
    Documentation,
    /// `198.18.0.0/15` benchmarking (RFC 2544).
    Benchmarking,
    /// `240.0.0.0/4` reserved for future use.
    Reserved,
    /// `255.255.255.255` limited broadcast.
    Broadcast,
    /// `224.0.0.0/4` or `ff00::/8` multicast.
    Multicast,
    /// `192.0.0.0/24` IETF protocol assignments (RFC 6890).
    ProtocolAssignment,
    /// `192.88.99.0/24` 6to4 relay anycast (RFC 7526, deprecated).
    SixToFourRelay,
    /// `fc00::/7` unique-local addresses.
    UniqueLocal,
    /// `fec0::/10` deprecated site-local (RFC 3879).
    SiteLocal,
    /// `2002::/16` 6to4.
    SixToFour,
    /// `2001::/32` Teredo.
    Teredo,
    /// `2001::/23` IPv6 IETF protocol assignments (ORCHIDv2 `2001:20::/28`,
    /// benchmarking `2001:2::/48`, etc. — RFC 6890).
    ProtocolAssignmentV6,
    /// `100::/64` discard-only address block (RFC 6666).
    DiscardOnly,
    /// `64:ff9b::/96` well-known + `64:ff9b:1::/48` local-use NAT64 (RFC 6052/8215).
    /// The well-known prefix re-validates its embedded IPv4; the local-use prefix is
    /// rejected outright.
    Nat64,
}

impl RejectReason {
    fn message(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified address (0.0.0.0/:: is not a routable target)",
            Self::Loopback => "loopback address (127.0.0.0/8, ::1)",
            Self::Private => "private address (10/8, 172.16/12, 192.168/16)",
            Self::LinkLocal => "link-local address (169.254.0.0/16, fe80::/10)",
            Self::SharedCgn => "carrier-grade NAT shared address (100.64.0.0/10)",
            Self::Documentation => {
                "documentation-reserved address (192.0.2/198.51.100/203.0.113, 2001:db8::/32)"
            }
            Self::Benchmarking => "benchmarking-reserved address (198.18.0.0/15)",
            Self::Reserved => "reserved address (240.0.0.0/4)",
            Self::Broadcast => "broadcast address (255.255.255.255)",
            Self::Multicast => "multicast address (224.0.0.0/4, ff00::/8)",
            Self::ProtocolAssignment => "IETF protocol-assignment address (192.0.0.0/24)",
            Self::SixToFourRelay => "6to4 relay anycast address (192.88.99.0/24)",
            Self::UniqueLocal => "unique-local address (fc00::/7)",
            Self::SiteLocal => "deprecated site-local address (fec0::/10)",
            Self::SixToFour => "6to4 address (2002::/16)",
            Self::Teredo => "Teredo address (2001::/32)",
            Self::ProtocolAssignmentV6 => {
                "IPv6 IETF protocol-assignment address (2001::/23, incl. ORCHIDv2/benchmarking)"
            }
            Self::DiscardOnly => "discard-only address (100::/64)",
            Self::Nat64 => "NAT64 address (64:ff9b::/96 well-known, 64:ff9b:1::/48 local-use)",
        }
    }
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

/// The outcome of validating an untrusted target string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetError {
    /// Empty, or not an IP literal and not a syntactically valid hostname (includes any
    /// input carrying shell metacharacters — rejected before it can reach a resolver).
    Malformed,
    /// An address (given directly or resolved from a hostname) is not public.
    Rejected(RejectReason),
    /// A syntactically valid hostname that resolved to no address.
    Unresolvable,
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => f.write_str("target is not a valid IP address or hostname"),
            Self::Rejected(reason) => write!(f, "target rejected: {reason}"),
            Self::Unresolvable => f.write_str("hostname did not resolve to any address"),
        }
    }
}

impl std::error::Error for TargetError {}

/// Resolves a hostname to its A/AAAA addresses. Abstracted so the validator can be unit
/// tested with a deterministic stub rather than live DNS (see the tests) and so the same
/// validation runs against any resolver implementation.
pub trait HostResolver {
    /// Resolve `host` to zero or more addresses. An empty result means "no records".
    fn resolve(&self, host: &str)
        -> impl Future<Output = Result<Vec<IpAddr>, ResolveError>> + Send;
}

/// A hostname could not be resolved (network failure, NXDOMAIN, timeout). The detail is
/// kept for logs; callers surface [`TargetError::Unresolvable`] to the user.
#[derive(Debug, Clone)]
pub struct ResolveError(pub String);

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ResolveError {}

/// A [`HostResolver`] backed by the system DNS configuration, via hickory-resolver.
/// Constructed once and shared; the local node and agent use it as the production
/// resolver while tests use a stub.
pub struct DnsResolver {
    inner: hickory_resolver::TokioResolver,
}

impl DnsResolver {
    /// Build a resolver from the operating system's DNS configuration
    /// (`/etc/resolv.conf` on Unix).
    pub fn from_system() -> Result<Self, ResolveError> {
        let inner = hickory_resolver::Resolver::builder_tokio()
            .map_err(|e| ResolveError(e.to_string()))?
            .build()
            .map_err(|e| ResolveError(e.to_string()))?;
        Ok(Self { inner })
    }
}

impl HostResolver for DnsResolver {
    fn resolve(
        &self,
        host: &str,
    ) -> impl Future<Output = Result<Vec<IpAddr>, ResolveError>> + Send {
        let host = host.to_string();
        async move {
            let lookup = self
                .inner
                .lookup_ip(host)
                .await
                .map_err(|e| ResolveError(e.to_string()))?;
            Ok(lookup.iter().collect())
        }
    }
}

/// Validate an untrusted target string to a single public IP.
///
/// An IP literal is validated directly. A hostname is validated for syntax, resolved,
/// and **every** resolved address is re-validated against the same rules (AC12) — if any
/// is non-public the whole target is rejected, closing the SSRF-via-DNS hole where an
/// attacker mixes one public and one private record. The first address is pinned for use.
pub async fn validate_target<R: HostResolver>(
    input: &str,
    resolver: &R,
) -> Result<ValidatedTarget, TargetError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(TargetError::Malformed);
    }

    if let Ok(ip) = trimmed.parse::<IpAddr>() {
        validate_ip(ip).map_err(TargetError::Rejected)?;
        return Ok(ValidatedTarget {
            ip,
            input: trimmed.to_string(),
        });
    }

    if !is_valid_hostname(trimmed) {
        return Err(TargetError::Malformed);
    }

    let addrs = resolver
        .resolve(trimmed)
        .await
        .map_err(|_| TargetError::Unresolvable)?;
    let pinned = *addrs.first().ok_or(TargetError::Unresolvable)?;
    for addr in &addrs {
        validate_ip(*addr).map_err(TargetError::Rejected)?;
    }

    Ok(ValidatedTarget {
        ip: pinned,
        input: trimmed.to_string(),
    })
}

/// Accept an IP only if it is public; reject every special/non-global range.
///
/// The rejection set is deliberately closed and exhaustive — this is the safety property.
/// Prefer the stdlib range predicate where one exists; fall back to explicit bit checks
/// for the ranges std has no stable predicate for (CGN, benchmarking, reserved, protocol
/// assignments, ULA, 6to4, Teredo).
pub fn validate_ip(ip: IpAddr) -> Result<(), RejectReason> {
    match ip {
        IpAddr::V4(v4) => validate_ipv4(v4),
        IpAddr::V6(v6) => validate_ipv6(v6),
    }
}

fn validate_ipv4(ip: Ipv4Addr) -> Result<(), RejectReason> {
    let o = ip.octets();
    if ip.is_unspecified() || o[0] == 0 {
        return Err(RejectReason::Unspecified); // 0.0.0.0/8
    }
    if ip.is_loopback() {
        return Err(RejectReason::Loopback);
    }
    if ip.is_private() {
        return Err(RejectReason::Private);
    }
    if ip.is_link_local() {
        return Err(RejectReason::LinkLocal); // 169.254.0.0/16
    }
    if o[0] == 100 && (o[1] & 0xc0) == 0x40 {
        return Err(RejectReason::SharedCgn); // 100.64.0.0/10
    }
    if ip.is_documentation() {
        return Err(RejectReason::Documentation);
    }
    if o[0] == 198 && (o[1] & 0xfe) == 18 {
        return Err(RejectReason::Benchmarking); // 198.18.0.0/15
    }
    if o[0] == 192 && o[1] == 0 && o[2] == 0 {
        return Err(RejectReason::ProtocolAssignment); // 192.0.0.0/24
    }
    if o[0] == 192 && o[1] == 88 && o[2] == 99 {
        return Err(RejectReason::SixToFourRelay); // 192.88.99.0/24
    }
    if ip.is_broadcast() {
        return Err(RejectReason::Broadcast); // 255.255.255.255
    }
    if ip.is_multicast() {
        return Err(RejectReason::Multicast); // 224.0.0.0/4
    }
    if o[0] >= 240 {
        return Err(RejectReason::Reserved); // 240.0.0.0/4
    }
    Ok(())
}

fn validate_ipv6(ip: Ipv6Addr) -> Result<(), RejectReason> {
    // An IPv4-mapped (::ffff:a.b.c.d) or deprecated IPv4-compatible (::a.b.c.d) address
    // embeds a v4 address; validate the embedded v4 so ::ffff:10.0.0.1 cannot bypass the
    // v4 rules. This is a real SSRF vector, so it is checked first.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return validate_ipv4(v4);
    }
    let seg = ip.segments();
    // NAT64 (RFC 6052/8215). The well-known `64:ff9b::/96` embeds an IPv4 in its low 32
    // bits — re-validate it, so a DNS64 resolver synthesizing `64:ff9b::<private-v4>` for a
    // private-only name cannot bypass the v4 rules. The `64:ff9b:1::/48` local-use prefix
    // uses a length-dependent embedding, so it is rejected outright.
    if seg[0] == 0x0064 && seg[1] == 0xff9b {
        if seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0 {
            let v4 = Ipv4Addr::new(
                (seg[6] >> 8) as u8,
                seg[6] as u8,
                (seg[7] >> 8) as u8,
                seg[7] as u8,
            );
            return validate_ipv4(v4);
        }
        return Err(RejectReason::Nat64);
    }
    if seg[..6].iter().all(|&s| s == 0) && !ip.is_unspecified() && !ip.is_loopback() {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        );
        return validate_ipv4(v4);
    }

    if ip.is_unspecified() {
        return Err(RejectReason::Unspecified); // ::
    }
    if ip.is_loopback() {
        return Err(RejectReason::Loopback); // ::1
    }
    if ip.is_multicast() {
        return Err(RejectReason::Multicast); // ff00::/8
    }
    if seg[0] == 0x0100 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0 {
        return Err(RejectReason::DiscardOnly); // 100::/64
    }
    if (seg[0] & 0xfe00) == 0xfc00 {
        return Err(RejectReason::UniqueLocal); // fc00::/7
    }
    if (seg[0] & 0xffc0) == 0xfe80 {
        return Err(RejectReason::LinkLocal); // fe80::/10
    }
    if (seg[0] & 0xffc0) == 0xfec0 {
        return Err(RejectReason::SiteLocal); // fec0::/10 (deprecated)
    }
    if seg[0] == 0x2001 && seg[1] == 0x0db8 {
        return Err(RejectReason::Documentation); // 2001:db8::/32
    }
    if seg[0] == 0x2001 && seg[1] == 0x0000 {
        return Err(RejectReason::Teredo); // 2001::/32
    }
    // 2001::/23 IETF protocol assignments (ORCHIDv2 2001:20::/28, benchmarking 2001:2::/48,
    // etc.) — Teredo above is the /32 subset with its own reason; this catches the rest.
    if seg[0] == 0x2001 && (seg[1] & 0xfe00) == 0x0000 {
        return Err(RejectReason::ProtocolAssignmentV6); // 2001::/23
    }
    if seg[0] == 0x2002 {
        return Err(RejectReason::SixToFour); // 2002::/16
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// BGP query arguments (Slice 11) — a separate, family-locked grammar.
//
// A BGP prefix is validated by a DIFFERENT rule than a diagnostic target: BGP
// inspects the node's local routing table and never opens a connection, so the
// SSRF public-range filter above is deliberately NOT applied — a private,
// reserved, or bogon prefix is a legitimate route lookup. What this boundary
// enforces instead is strict IP/CIDR *syntax*, family lock, and a canonical token
// rebuilt from parsed numbers, so nothing a daemon CLI could interpret ever
// reaches it (FR-072/AC36).
// ---------------------------------------------------------------------------

/// The address family a BGP prefix is locked to. `bgp` accepts only IPv4 and
/// `bgp6` only IPv6, so a v6 literal can never reach the v4 query template and a
/// v4 literal can never reach the v6 one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixFamily {
    V4,
    V6,
}

impl PrefixFamily {
    /// The wire method name for this family's BGP query (`bgp` / `bgp6`) — the
    /// single mapping both central and the agent recognise, so they cannot drift.
    pub fn wire(self) -> &'static str {
        match self {
            PrefixFamily::V4 => "bgp",
            PrefixFamily::V6 => "bgp6",
        }
    }

    /// Recognise a BGP wire method name, or `None` for a non-BGP method.
    pub fn from_wire(method: &str) -> Option<Self> {
        match method {
            "bgp" => Some(PrefixFamily::V4),
            "bgp6" => Some(PrefixFamily::V6),
            _ => None,
        }
    }
}

/// A BGP query argument that passed [`bgp_arg`]: a syntactically valid IP or CIDR
/// prefix, locked to an address family, and rebuilt into a canonical token from its
/// parsed numeric components. [`Self::arg`] is what a daemon template injects, so
/// the value handed to `birdc` / `vtysh` is derived from parsed numbers — never the
/// raw user input — which is the injection boundary for BGP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPrefix {
    canonical: String,
    family: PrefixFamily,
}

impl ValidatedPrefix {
    /// The canonical prefix token to pass a daemon template as its query argument.
    pub fn arg(&self) -> &str {
        &self.canonical
    }

    /// The address family this prefix is locked to (selects the FRR query form).
    pub fn family(&self) -> PrefixFamily {
        self.family
    }
}

/// Why a BGP query argument is not a valid prefix. Distinguished so a caller (and
/// each test) can assert the exact reason a malformed or wrong-family arg was
/// refused before any daemon command was built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgpArgError {
    /// Not a syntactically valid IP or CIDR prefix — a hostname, an out-of-range
    /// mask, whitespace, or any shell metacharacter fails the strict parse here.
    Malformed,
    /// A syntactically valid literal of the wrong address family for the method (a
    /// v6 literal for `bgp`, or a v4 literal for `bgp6`).
    WrongFamily,
}

impl fmt::Display for BgpArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => f.write_str("BGP argument is not a valid IP address or CIDR prefix"),
            Self::WrongFamily => {
                f.write_str("BGP argument is the wrong address family for this method")
            }
        }
    }
}

impl std::error::Error for BgpArgError {}

/// Validate an untrusted BGP query argument to a family-locked [`ValidatedPrefix`].
///
/// The grammar is strict IP-or-CIDR and nothing else: a bare address, or an address
/// with a `/len` mask whose length is in range for the family. Parsing goes through
/// the standard library's address and integer parsers, so a hostname, whitespace,
/// or any shell metacharacter fails to parse and is rejected here — *before* any
/// daemon command is built (FR-072/AC36). The canonical token is reconstructed from
/// the parsed value, so a daemon never sees the raw input. No public-range (SSRF)
/// filter is applied: a private/reserved/bogon prefix is a legitimate route lookup.
pub fn bgp_arg(input: &str, family: PrefixFamily) -> Result<ValidatedPrefix, BgpArgError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(BgpArgError::Malformed);
    }
    let (addr_part, mask_part) = match trimmed.split_once('/') {
        Some((addr, mask)) => (addr, Some(mask)),
        None => (trimmed, None),
    };
    let canonical = match family {
        PrefixFamily::V4 => {
            let addr = parse_v4(addr_part)?;
            match mask_part {
                Some(mask) => format!("{addr}/{}", parse_prefix_len(mask, 32)?),
                None => addr.to_string(),
            }
        }
        PrefixFamily::V6 => {
            let addr = parse_v6(addr_part)?;
            match mask_part {
                Some(mask) => format!("{addr}/{}", parse_prefix_len(mask, 128)?),
                None => addr.to_string(),
            }
        }
    };
    Ok(ValidatedPrefix { canonical, family })
}

/// Parse the address half of a `bgp` (IPv4) argument. A valid v6 literal here is a
/// family error, not malformed — so the caller can tell the two apart.
fn parse_v4(addr: &str) -> Result<Ipv4Addr, BgpArgError> {
    if let Ok(v4) = addr.parse::<Ipv4Addr>() {
        Ok(v4)
    } else if addr.parse::<Ipv6Addr>().is_ok() {
        Err(BgpArgError::WrongFamily)
    } else {
        Err(BgpArgError::Malformed)
    }
}

/// Parse the address half of a `bgp6` (IPv6) argument. A valid v4 literal here is a
/// family error, not malformed.
fn parse_v6(addr: &str) -> Result<Ipv6Addr, BgpArgError> {
    if let Ok(v6) = addr.parse::<Ipv6Addr>() {
        Ok(v6)
    } else if addr.parse::<Ipv4Addr>().is_ok() {
        Err(BgpArgError::WrongFamily)
    } else {
        Err(BgpArgError::Malformed)
    }
}

/// Parse a CIDR mask length: an integer in `0..=max`. A strict `u8` parse rejects a
/// sign, whitespace, or any non-digit, so `10.0.0.0/8; ...` cannot slip a
/// metacharacter through the mask half.
fn parse_prefix_len(mask: &str, max: u8) -> Result<u8, BgpArgError> {
    match mask.parse::<u8>() {
        Ok(len) if len <= max => Ok(len),
        _ => Err(BgpArgError::Malformed),
    }
}

/// A syntactically valid DNS hostname: 1..=253 chars, dot-separated LDH labels, each label
/// 1..=63 chars, not starting or ending with a hyphen. This rejects any input carrying a
/// shell metacharacter or whitespace before it can reach the resolver — the injection
/// boundary — and keeps malformed input from being handed to a command.
fn is_valid_hostname(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() {
        return false;
    }
    host.split('.').all(is_valid_label)
}

fn is_valid_label(label: &str) -> bool {
    if label.is_empty() || label.len() > 63 {
        return false;
    }
    if label.starts_with('-') || label.ends_with('-') {
        return false;
    }
    label
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }
    fn v6(s: &str) -> Ipv6Addr {
        s.parse().unwrap()
    }
    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    // A resolver stub returning a fixed address set — deterministic, no live DNS.
    struct StubResolver {
        addrs: Vec<IpAddr>,
    }
    impl HostResolver for StubResolver {
        async fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, ResolveError> {
            Ok(self.addrs.clone())
        }
    }

    struct FailingResolver;
    impl HostResolver for FailingResolver {
        async fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, ResolveError> {
            Err(ResolveError("nxdomain".into()))
        }
    }

    // ---- ACCEPT: public addresses ----

    #[test]
    fn accepts_public_ipv4() {
        assert_eq!(validate_ipv4(v4("8.8.8.8")), Ok(()));
        assert_eq!(validate_ipv4(v4("1.1.1.1")), Ok(()));
        assert_eq!(validate_ipv4(v4("93.184.216.34")), Ok(()));
    }

    #[test]
    fn accepts_public_ipv6() {
        assert_eq!(validate_ipv6(v6("2606:4700:4700::1111")), Ok(()));
        assert_eq!(validate_ipv6(v6("2001:4860:4860::8888")), Ok(()));
    }

    // ---- REJECT: one test per special range (no process started — validation only) ----

    #[test]
    fn rejects_private_10() {
        assert_eq!(validate_ipv4(v4("10.0.0.1")), Err(RejectReason::Private));
    }
    #[test]
    fn rejects_private_172_16() {
        assert_eq!(validate_ipv4(v4("172.16.5.4")), Err(RejectReason::Private));
    }
    #[test]
    fn rejects_private_192_168() {
        assert_eq!(validate_ipv4(v4("192.168.1.1")), Err(RejectReason::Private));
    }
    #[test]
    fn rejects_loopback_v4() {
        assert_eq!(validate_ipv4(v4("127.0.0.1")), Err(RejectReason::Loopback));
    }
    #[test]
    fn rejects_link_local_v4() {
        assert_eq!(
            validate_ipv4(v4("169.254.1.1")),
            Err(RejectReason::LinkLocal)
        );
    }
    #[test]
    fn rejects_shared_cgn_v4() {
        assert_eq!(
            validate_ipv4(v4("100.64.0.1")),
            Err(RejectReason::SharedCgn)
        );
    }
    #[test]
    fn rejects_documentation_v4() {
        assert_eq!(
            validate_ipv4(v4("192.0.2.1")),
            Err(RejectReason::Documentation)
        );
        assert_eq!(
            validate_ipv4(v4("198.51.100.1")),
            Err(RejectReason::Documentation)
        );
        assert_eq!(
            validate_ipv4(v4("203.0.113.1")),
            Err(RejectReason::Documentation)
        );
    }
    #[test]
    fn rejects_benchmarking_v4() {
        assert_eq!(
            validate_ipv4(v4("198.18.0.1")),
            Err(RejectReason::Benchmarking)
        );
        assert_eq!(
            validate_ipv4(v4("198.19.255.1")),
            Err(RejectReason::Benchmarking)
        );
    }
    #[test]
    fn rejects_protocol_assignment_v4() {
        assert_eq!(
            validate_ipv4(v4("192.0.0.8")),
            Err(RejectReason::ProtocolAssignment)
        );
    }
    #[test]
    fn rejects_broadcast_v4() {
        assert_eq!(
            validate_ipv4(v4("255.255.255.255")),
            Err(RejectReason::Broadcast)
        );
    }
    #[test]
    fn rejects_multicast_v4() {
        assert_eq!(validate_ipv4(v4("224.0.0.1")), Err(RejectReason::Multicast));
    }
    #[test]
    fn rejects_reserved_v4() {
        assert_eq!(validate_ipv4(v4("240.0.0.1")), Err(RejectReason::Reserved));
    }
    #[test]
    fn rejects_unspecified_v4() {
        assert_eq!(validate_ipv4(v4("0.0.0.0")), Err(RejectReason::Unspecified));
    }

    #[test]
    fn rejects_unspecified_v6() {
        assert_eq!(validate_ipv6(v6("::")), Err(RejectReason::Unspecified));
    }
    #[test]
    fn rejects_loopback_v6() {
        assert_eq!(validate_ipv6(v6("::1")), Err(RejectReason::Loopback));
    }
    #[test]
    fn rejects_link_local_v6() {
        assert_eq!(validate_ipv6(v6("fe80::1")), Err(RejectReason::LinkLocal));
    }
    #[test]
    fn rejects_unique_local_v6() {
        assert_eq!(validate_ipv6(v6("fc00::1")), Err(RejectReason::UniqueLocal));
        assert_eq!(
            validate_ipv6(v6("fd12:3456::1")),
            Err(RejectReason::UniqueLocal)
        );
    }
    #[test]
    fn rejects_documentation_v6() {
        assert_eq!(
            validate_ipv6(v6("2001:db8::1")),
            Err(RejectReason::Documentation)
        );
    }
    #[test]
    fn rejects_teredo_v6() {
        assert_eq!(
            validate_ipv6(v6("2001:0:abcd::1")),
            Err(RejectReason::Teredo)
        );
    }
    #[test]
    fn rejects_six_to_four_v6() {
        assert_eq!(
            validate_ipv6(v6("2002:c000:0204::1")),
            Err(RejectReason::SixToFour)
        );
    }
    #[test]
    fn rejects_multicast_v6() {
        assert_eq!(validate_ipv6(v6("ff02::1")), Err(RejectReason::Multicast));
    }
    #[test]
    fn rejects_ipv4_mapped_private_v6() {
        // ::ffff:10.0.0.1 must not bypass the v4 private rule.
        assert_eq!(
            validate_ipv6(v6("::ffff:10.0.0.1")),
            Err(RejectReason::Private)
        );
    }
    #[test]
    fn rejects_site_local_v6() {
        assert_eq!(validate_ipv6(v6("fec0::1")), Err(RejectReason::SiteLocal));
    }
    #[test]
    fn rejects_orchid_v6() {
        // ORCHIDv2 2001:20::/28 and benchmarking 2001:2::/48 fall in 2001::/23.
        assert_eq!(
            validate_ipv6(v6("2001:20::1")),
            Err(RejectReason::ProtocolAssignmentV6)
        );
        assert_eq!(
            validate_ipv6(v6("2001:2::1")),
            Err(RejectReason::ProtocolAssignmentV6)
        );
    }
    #[test]
    fn rejects_discard_only_v6() {
        assert_eq!(validate_ipv6(v6("100::1")), Err(RejectReason::DiscardOnly));
    }
    #[test]
    fn rejects_nat64_well_known_embedding_private_v6() {
        // 64:ff9b::10.0.0.1 and 64:ff9b::192.168.0.10 embed a private v4 that must be
        // re-validated (RFC 6052 well-known prefix) — the DNS64 SSRF vector.
        assert_eq!(
            validate_ipv6(v6("64:ff9b::a00:1")), // ::10.0.0.1
            Err(RejectReason::Private)
        );
        assert_eq!(
            validate_ipv6(v6("64:ff9b::c0a8:a")), // ::192.168.0.10
            Err(RejectReason::Private)
        );
    }
    #[test]
    fn rejects_nat64_local_use_v6() {
        // 64:ff9b:1::/48 local-use prefix is rejected outright.
        assert_eq!(
            validate_ipv6(v6("64:ff9b:1::10.0.0.1")),
            Err(RejectReason::Nat64)
        );
    }
    #[test]
    fn accepts_nat64_well_known_embedding_public_v6() {
        // A public-embedded well-known NAT64 address re-validates its v4 and passes.
        assert_eq!(validate_ipv6(v6("64:ff9b::808:808")), Ok(())); // ::8.8.8.8
    }

    #[test]
    fn rejects_6to4_relay_anycast_v4() {
        assert_eq!(
            validate_ipv4(v4("192.88.99.1")),
            Err(RejectReason::SixToFourRelay)
        );
    }

    // ---- validate_target: end-to-end via literals and the resolver stub ----

    #[tokio::test]
    async fn accepts_public_ip_literal() {
        let r = StubResolver { addrs: vec![] };
        let t = validate_target("8.8.8.8", &r).await.unwrap();
        assert_eq!(t.ip(), ip("8.8.8.8"));
        assert_eq!(t.arg(), "8.8.8.8");
    }

    #[tokio::test]
    async fn rejects_private_ip_literal_no_resolution() {
        let r = StubResolver { addrs: vec![] };
        assert_eq!(
            validate_target("10.1.2.3", &r).await,
            Err(TargetError::Rejected(RejectReason::Private))
        );
    }

    #[tokio::test]
    async fn rejects_malformed_target() {
        let r = StubResolver { addrs: vec![] };
        // Shell metacharacters and whitespace never reach the resolver.
        assert_eq!(
            validate_target("bad;rm -rf /", &r).await,
            Err(TargetError::Malformed)
        );
        assert_eq!(
            validate_target("$(whoami)", &r).await,
            Err(TargetError::Malformed)
        );
        assert_eq!(
            validate_target("   ", &r).await,
            Err(TargetError::Malformed)
        );
    }

    #[tokio::test]
    async fn resolves_hostname_then_accepts_public_ip() {
        let r = StubResolver {
            addrs: vec![ip("93.184.216.34")],
        };
        let t = validate_target("example.com", &r).await.unwrap();
        assert_eq!(t.ip(), ip("93.184.216.34"));
        // The pinned public IP, not the hostname, becomes the command argument.
        assert_eq!(t.arg(), "93.184.216.34");
    }

    #[tokio::test]
    async fn rejects_hostname_resolving_to_private_ip() {
        // SSRF-via-DNS: a name that resolves to a private address must be rejected.
        let r = StubResolver {
            addrs: vec![ip("192.168.0.10")],
        };
        assert_eq!(
            validate_target("internal.evil.test", &r).await,
            Err(TargetError::Rejected(RejectReason::Private))
        );
    }

    #[tokio::test]
    async fn rejects_hostname_with_any_private_record() {
        // One public + one private record: the private record still rejects the target.
        let r = StubResolver {
            addrs: vec![ip("8.8.8.8"), ip("10.0.0.5")],
        };
        assert_eq!(
            validate_target("split.evil.test", &r).await,
            Err(TargetError::Rejected(RejectReason::Private))
        );
    }

    #[tokio::test]
    async fn rejects_hostname_dns64_synthesized_private() {
        // DNS64: an operator resolver synthesizes AAAA 64:ff9b::<private-v4> for a
        // private-only name. The re-validated embedded v4 must still reject it.
        let r = StubResolver {
            addrs: vec![ip("64:ff9b::c0a8:a")], // 64:ff9b::192.168.0.10
        };
        assert_eq!(
            validate_target("dns64.evil.test", &r).await,
            Err(TargetError::Rejected(RejectReason::Private))
        );
    }

    #[tokio::test]
    async fn rejects_unresolvable_hostname() {
        assert_eq!(
            validate_target("nope.invalid", &FailingResolver).await,
            Err(TargetError::Unresolvable)
        );
    }

    #[tokio::test]
    async fn rejects_hostname_with_no_records() {
        let r = StubResolver { addrs: vec![] };
        assert_eq!(
            validate_target("empty.test", &r).await,
            Err(TargetError::Unresolvable)
        );
    }

    // ---- BGP argument grammar (Slice 11 / T4) --------------------------------

    #[test]
    fn bgp_arg_accepts_a_bare_v4_and_canonicalises_it() {
        let prefix = bgp_arg("8.8.8.8", PrefixFamily::V4).unwrap();
        assert_eq!(prefix.arg(), "8.8.8.8");
        assert_eq!(prefix.family(), PrefixFamily::V4);
    }

    #[test]
    fn bgp_arg_accepts_v4_and_v6_cidr() {
        assert_eq!(
            bgp_arg("203.0.113.0/24", PrefixFamily::V4).unwrap().arg(),
            "203.0.113.0/24"
        );
        // The v6 address half is canonicalised (compressed) on the way out.
        assert_eq!(
            bgp_arg("2001:0db8::/32", PrefixFamily::V6).unwrap().arg(),
            "2001:db8::/32"
        );
    }

    #[test]
    fn bgp_arg_accepts_a_private_prefix_no_ssrf_filter() {
        // The deliberate difference from validate_target: BGP inspects the local
        // RIB and never connects, so a private/bogon prefix is a legitimate route
        // lookup and MUST be accepted here.
        assert_eq!(
            bgp_arg("10.0.0.0/8", PrefixFamily::V4).unwrap().arg(),
            "10.0.0.0/8"
        );
        assert_eq!(
            bgp_arg("192.168.0.0/16", PrefixFamily::V4).unwrap().arg(),
            "192.168.0.0/16"
        );
        assert_eq!(
            bgp_arg("fc00::/7", PrefixFamily::V6).unwrap().arg(),
            "fc00::/7"
        );
    }

    #[test]
    fn bgp_arg_rejects_injection_and_metacharacters_before_any_command() {
        // The read-only boundary: a shell/daemon injection attempt is rejected as
        // malformed syntax, so no daemon command is ever built (AC36/FR-072).
        for injected in [
            "8.8.8.8; configure",
            "8.8.8.8 && reboot",
            "8.8.8.8 | sh",
            "$(whoami)",
            "8.8.8.8\nconfigure terminal",
            "10.0.0.0/8 add route",
            "10.0.0.0/33", // mask out of range
            "8.8.8.8/",    // empty mask
            "8.8.8.8/-1",  // negative mask
            "   ",
        ] {
            assert_eq!(
                bgp_arg(injected, PrefixFamily::V4),
                Err(BgpArgError::Malformed),
                "injected/malformed BGP arg must be rejected: {injected:?}"
            );
        }
    }

    #[test]
    fn bgp_arg_rejects_a_hostname() {
        assert_eq!(
            bgp_arg("route-server.example.com", PrefixFamily::V4),
            Err(BgpArgError::Malformed)
        );
    }

    #[test]
    fn bgp_arg_is_family_locked() {
        // A v6 literal for bgp (v4) and a v4 literal for bgp6 are wrong-family, not
        // malformed — the family lock keeps a prefix off the wrong query template.
        assert_eq!(
            bgp_arg("2001:db8::/32", PrefixFamily::V4),
            Err(BgpArgError::WrongFamily)
        );
        assert_eq!(
            bgp_arg("8.8.8.8", PrefixFamily::V6),
            Err(BgpArgError::WrongFamily)
        );
    }

    #[test]
    fn prefix_family_wire_round_trips() {
        assert_eq!(PrefixFamily::from_wire("bgp"), Some(PrefixFamily::V4));
        assert_eq!(PrefixFamily::from_wire("bgp6"), Some(PrefixFamily::V6));
        assert_eq!(PrefixFamily::from_wire("ping"), None);
        assert_eq!(PrefixFamily::V4.wire(), "bgp");
        assert_eq!(PrefixFamily::V6.wire(), "bgp6");
    }
}
