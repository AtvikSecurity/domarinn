//! The real client address, resolved once per request through
//! `DOMARINN_TRUSTED_PROXIES`.
//!
//! Behind a reverse proxy the TCP peer is the proxy, and every client collapses
//! into one address — so anything keyed on "the client" (login limits, the MCP
//! limiter, request logs) would be keyed on the proxy. The fix is to read
//! `X-Forwarded-For`, but only from peers the operator has said are proxies:
//! from anyone else the header is whatever the attacker says it is, and
//! honouring it would hand every request a fresh identity.
//!
//! `client_ip_middleware` puts a [`ClientIp`] in the request extensions. It
//! needs axum's `ConnectInfo`, which [`crate::serve_on`] always provides; under
//! the `oneshot` test harness there is none, so no `ClientIp` is inserted and
//! the per-client limit does not apply there.
//!
//! Only `X-Forwarded-For` is read. `Forwarded` (RFC 7239) is not what
//! ingress-nginx or Traefik send by default, and a second header that could
//! disagree with the first is one more thing to get wrong.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderName};
use axum::middleware::Next;
use axum::response::Response;

use crate::AppState;

const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");

/// The client address of one request, as resolved by [`resolve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIp(pub IpAddr);

impl ClientIp {
    /// The address part of a limiter key. IPv6 clients are bucketed by their
    /// /64, which is what one host or one customer is normally handed, so
    /// cycling through the interface identifier does not mint fresh budgets;
    /// IPv4-mapped IPv6 is the IPv4 address it carries.
    pub fn bucket(&self) -> String {
        match self.0.to_canonical() {
            IpAddr::V4(v4) => v4.to_string(),
            IpAddr::V6(v6) => {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        }
    }
}

/// Present in a request's extensions when its peer met the misconfiguration
/// condition ([`warrants_proxy_warning`]): a private-range peer forwarding for
/// someone while `DOMARINN_TRUSTED_PROXIES` is empty. A separate marker rather
/// than a field on [`ClientIp`], so two requests from one client stay equal
/// and keep sharing one bucket; refusal logs carry it as `proxy_unconfigured`,
/// which is what tells an operator the "client" being limited is the proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyUnconfigured;

/// One trusted network, stored pre-masked so matching is a single AND.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Net {
    V4 { addr: u32, mask: u32 },
    V6 { addr: u128, mask: u128 },
}

impl Net {
    fn contains(&self, ip: IpAddr) -> bool {
        match (*self, ip) {
            (Net::V4 { addr, mask }, IpAddr::V4(v4)) => u32::from(v4) & mask == addr,
            (Net::V6 { addr, mask }, IpAddr::V6(v6)) => u128::from(v6) & mask == addr,
            _ => false,
        }
    }

    /// A /0 of either family: every peer of that family matches.
    fn is_catch_all(&self) -> bool {
        match *self {
            Net::V4 { mask, .. } => mask == 0,
            Net::V6 { mask, .. } => mask == 0,
        }
    }
}

/// The peers allowed to speak for someone else via `X-Forwarded-For`
/// (`DOMARINN_TRUSTED_PROXIES`). Empty — the default — trusts nobody.
#[derive(Debug, Clone, Default)]
pub struct TrustedProxies {
    nets: Vec<Net>,
}

impl TrustedProxies {
    /// Parse a comma-separated list of IPs and CIDRs, v4 or v6. Whitespace is
    /// trimmed and empty entries are skipped, so a trailing comma is harmless.
    ///
    /// A bad entry fails startup rather than being dropped: a typo in a trust
    /// list silently trusting less is every client collapsing into the proxy,
    /// and the operator would only find out when a lockout lands on everyone.
    ///
    /// A catch-all (`0.0.0.0/0`, `::/0`) is allowed, since a deployment that
    /// is only reachable through its proxy may genuinely want it, but it is
    /// logged: it lets any peer that can reach the port pick its own address.
    pub fn parse(raw: Option<&str>) -> anyhow::Result<TrustedProxies> {
        let nets = raw
            .into_iter()
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                let net = parse_net(entry).map_err(|reason| {
                    anyhow::anyhow!(
                        "invalid DOMARINN_TRUSTED_PROXIES entry '{entry}' ({reason}); expected an \
                         IP address or CIDR such as 10.0.0.0/8 or fd00::/8"
                    )
                })?;
                if net.is_catch_all() {
                    tracing::warn!(
                        entry,
                        "DOMARINN_TRUSTED_PROXIES entry trusts every peer: any client that can \
                         reach the server can set its own address with X-Forwarded-For"
                    );
                }
                Ok(net)
            })
            .collect::<anyhow::Result<_>>()?;
        Ok(TrustedProxies { nets })
    }

    pub fn is_empty(&self) -> bool {
        self.nets.is_empty()
    }

    /// Whether `ip` is a trusted proxy. Canonicalised first, so a dual-stack
    /// listener's `::ffff:10.0.0.1` matches the `10.0.0.1` the operator wrote.
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        self.nets.iter().any(|net| net.contains(ip))
    }
}

/// `addr` or `addr/prefix`, or why not. Host bits below the prefix are
/// ignored (`10.1.2.3/8` is `10.0.0.0/8`), as most tools treat them.
///
/// IPv4-mapped IPv6 is turned into the v4 net it names, because peers are
/// canonicalised to v4 before matching: left as v6, `::ffff:10.0.0.0/104`
/// would parse fine and then never match anything. A mapped prefix shorter
/// than /96 reaches past the mapped range into real v6 space, so it is not
/// one v4 net and is refused rather than guessed at.
fn parse_net(entry: &str) -> Result<Net, &'static str> {
    let (addr, prefix) = match entry.split_once('/') {
        Some((addr, prefix)) => (
            addr.parse::<IpAddr>().map_err(|_| "not an IP address")?,
            Some(prefix.parse::<u32>().map_err(|_| "not a prefix length")?),
        ),
        None => (
            entry.parse::<IpAddr>().map_err(|_| "not an IP address")?,
            None,
        ),
    };
    let (addr, prefix) = match addr {
        IpAddr::V6(v6) => match (v6.to_ipv4_mapped(), prefix) {
            (Some(v4), None) => (IpAddr::V4(v4), None),
            (Some(v4), Some(p)) if (96..=128).contains(&p) => (IpAddr::V4(v4), Some(p - 96)),
            (Some(_), Some(p)) if p < 96 => {
                return Err("an IPv4-mapped prefix must be /96 or longer")
            }
            _ => (addr, prefix),
        },
        IpAddr::V4(_) => (addr, prefix),
    };
    // `checked_shl` because a /0 shifts by the full width, which `<<` rejects.
    match addr {
        IpAddr::V4(v4) => {
            let prefix = prefix.unwrap_or(32);
            if prefix > 32 {
                return Err("prefix longer than /32");
            }
            let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
            Ok(Net::V4 {
                addr: u32::from(v4) & mask,
                mask,
            })
        }
        IpAddr::V6(v6) => {
            let prefix = prefix.unwrap_or(128);
            if prefix > 128 {
                return Err("prefix longer than /128");
            }
            let mask = u128::MAX.checked_shl(128 - prefix).unwrap_or(0);
            Ok(Net::V6 {
                addr: u128::from(v6) & mask,
                mask,
            })
        }
    }
}

/// One `X-Forwarded-For` hop: a plain IP, `a.b.c.d:port`, or `[v6]:port`.
/// Some proxies append the port; nothing downstream wants it.
fn parse_hop(hop: &str) -> Option<IpAddr> {
    hop.parse::<IpAddr>()
        .ok()
        .or_else(|| hop.parse::<SocketAddr>().ok().map(|addr| addr.ip()))
}

/// The client address for a request from `peer`.
///
/// An untrusted peer is the client, whatever it claims. A trusted peer's
/// `X-Forwarded-For` is walked from the right — the end the nearest proxy
/// appended, and so the only end the chain of trust vouches for — skipping
/// hops that are themselves trusted; the first untrusted hop is the client.
/// Anything malformed, or a chain that is trusted all the way, falls back to
/// the peer: the hop the walk could not read is exactly where an attacker's
/// text would begin, and guessing past it would let them pick the answer.
pub fn resolve(peer: IpAddr, headers: &HeaderMap, trusted: &TrustedProxies) -> IpAddr {
    let peer = peer.to_canonical();
    if !trusted.contains(peer) {
        return peer;
    }
    let mut values = Vec::new();
    for value in headers.get_all(X_FORWARDED_FOR) {
        match value.to_str() {
            Ok(value) => values.push(value),
            Err(_) => return peer,
        }
    }
    if values.is_empty() {
        return peer;
    }
    // Repeated header lines are one list (RFC 9110 §5.3), so join before
    // splitting rather than reading only the last line.
    let joined = values.join(",");
    for hop in joined.split(',').rev() {
        let Some(ip) = parse_hop(hop.trim()) else {
            return peer;
        };
        let ip = ip.to_canonical();
        if !trusted.contains(ip) {
            return ip;
        }
    }
    peer
}

/// Whether a request looks like it came through a proxy nobody told us about:
/// a private-range peer forwarding for someone while the trust list is empty.
/// That is the misconfiguration that keys every client on the proxy, and it is
/// silent otherwise — so it earns one log line.
fn warrants_proxy_warning(peer: IpAddr, has_xff: bool, trusted: &TrustedProxies) -> bool {
    if !trusted.is_empty() || !has_xff {
        return false;
    }
    match peer.to_canonical() {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                // CGNAT, 100.64.0.0/10: where some clusters put pod networks.
                || (a == 100 && b & 0xc0 == 64)
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback()
                // ULA fc00::/7 and link-local fe80::/10; the std predicates
                // for these are still unstable.
                || first & 0xfe00 == 0xfc00
                || first & 0xffc0 == 0xfe80
        }
    }
}

/// At most one warning per this many seconds. Once per process was too quiet:
/// after an upgrade the one line scrolls away at startup while every user
/// keeps sharing the proxy's budget, and a long-lived pod never says it
/// again. Every ten minutes keeps it findable without burying the logs.
const WARN_EVERY_SECS: u64 = 10 * 60;

/// The epoch second of the last warning; 0 means never.
static LAST_WARNED: AtomicU64 = AtomicU64::new(0);

/// Whether a warning is due at `now_secs`, claiming the slot if so. The
/// compare-exchange means concurrent requests cannot both win one slot, and
/// there is no lock on the request path.
fn claim_warning(last: &AtomicU64, now_secs: u64) -> bool {
    let prev = last.load(Ordering::Relaxed);
    if prev != 0 && now_secs < prev.saturating_add(WARN_EVERY_SECS) {
        return false;
    }
    last.compare_exchange(prev, now_secs.max(1), Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Resolve the request's [`ClientIp`] into its extensions, with a
/// [`ProxyUnconfigured`] marker beside it when the peer looks like an
/// unlisted proxy. Never rejects.
pub(crate) async fn client_ip_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    annotate(&mut req, state.trusted_proxies.as_ref());
    next.run(req).await
}

/// The middleware's work, apart from `next` so tests can inspect the
/// extensions it leaves. Nothing is inserted without connect info.
fn annotate(req: &mut Request, trusted: &TrustedProxies) {
    let Some(ConnectInfo(peer)) = req.extensions().get::<ConnectInfo<SocketAddr>>().copied() else {
        return;
    };
    let peer = peer.ip();
    let has_xff = req.headers().contains_key(X_FORWARDED_FOR);
    let unconfigured = warrants_proxy_warning(peer, has_xff, trusted);
    if unconfigured && claim_warning(&LAST_WARNED, epoch_secs()) {
        tracing::warn!(
            peer = %peer,
            "a private-range peer sent X-Forwarded-For but DOMARINN_TRUSTED_PROXIES is \
             empty, so every client behind it shares the proxy's address; list the proxy \
             there"
        );
    }
    let client = resolve(peer, req.headers(), trusted);
    req.extensions_mut().insert(ClientIp(client));
    if unconfigured {
        req.extensions_mut().insert(ProxyUnconfigured);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn trusted(raw: &str) -> TrustedProxies {
        TrustedProxies::parse(Some(raw)).unwrap()
    }

    fn xff(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(X_FORWARDED_FOR, value.parse().unwrap());
        }
        headers
    }

    /// The default deployment: nothing is trusted, so the header is the
    /// attacker's to write and must change nothing. Trusting a range the peer
    /// is not in must not change that either.
    #[test]
    fn an_untrusted_peer_spoofing_xff_is_the_client() {
        let peer = ip("198.51.100.9");
        let headers = xff(&["192.0.2.44"]);
        assert_eq!(
            resolve(peer, &headers, &TrustedProxies::parse(None).unwrap()),
            peer
        );
        assert_eq!(resolve(peer, &headers, &trusted("10.0.0.0/8")), peer);
    }

    #[test]
    fn a_multi_hop_chain_resolves_to_the_first_untrusted_from_the_right() {
        let headers = xff(&["203.0.113.5, 198.51.100.7, 192.0.2.1"]);
        assert_eq!(
            resolve(ip("10.1.1.1"), &headers, &trusted("10.0.0.0/8, 192.0.2.1")),
            ip("198.51.100.7")
        );
    }

    /// RFC 9110 lets a list header arrive split across several lines; the
    /// split carries no meaning, so the lines read as one list, in order.
    #[test]
    fn repeated_xff_headers_are_one_list_in_order() {
        let headers = xff(&["203.0.113.5", "198.51.100.7, 10.0.0.2"]);
        assert_eq!(
            resolve(ip("10.0.0.1"), &headers, &trusted("10.0.0.0/8")),
            ip("198.51.100.7")
        );
    }

    #[test]
    fn malformed_missing_or_all_trusted_falls_back_to_the_peer() {
        let peer = ip("10.0.0.1");
        let t = trusted("10.0.0.0/8");
        for headers in [
            xff(&["garbage"]),
            xff(&[""]),
            xff(&["203.0.113.5, ,"]),
            HeaderMap::new(),
            xff(&["10.0.0.3"]),
        ] {
            assert_eq!(resolve(peer, &headers, &t), peer, "headers: {headers:?}");
        }
    }

    #[test]
    fn ports_and_bracketed_v6_hops_parse() {
        let peer = ip("10.0.0.1");
        let t = trusted("10.0.0.0/8");
        assert_eq!(
            resolve(peer, &xff(&["198.51.100.7:4711"]), &t),
            ip("198.51.100.7")
        );
        assert_eq!(
            resolve(peer, &xff(&["[2001:db8::5]:443"]), &t),
            ip("2001:db8::5")
        );
    }

    /// A dual-stack listener reports v4 peers as `::ffff:a.b.c.d`; the
    /// operator wrote the v4 address, and it must still match.
    #[test]
    fn mapped_v4_peers_match_v4_entries() {
        assert_eq!(
            resolve(
                ip("::ffff:10.0.0.1"),
                &xff(&["198.51.100.7"]),
                &trusted("10.0.0.1")
            ),
            ip("198.51.100.7")
        );
    }

    #[test]
    fn v6_cidrs_match() {
        let t = trusted("2001:db8:aa::/48");
        assert!(t.contains(ip("2001:db8:aa:1::1")));
        assert!(!t.contains(ip("2001:db8:ab::1")));
    }

    #[test]
    fn parse_rejects_bad_entries_naming_the_setting() {
        for bad in ["10.0.0.0/33", "nope", "::/129"] {
            let msg = TrustedProxies::parse(Some(bad)).unwrap_err().to_string();
            assert!(
                msg.contains("DOMARINN_TRUSTED_PROXIES"),
                "message was: {msg}"
            );
            assert!(msg.contains(bad), "message was: {msg}");
        }
        let ok = TrustedProxies::parse(Some(" 10.0.0.1 , ,fd00::/8")).unwrap();
        assert_eq!(ok.nets.len(), 2);
        assert!(ok.contains(ip("10.0.0.1")));
        assert!(ok.contains(ip("fd12::1")));
        assert!(TrustedProxies::parse(None).unwrap().is_empty());
    }

    /// Moved from the login throttle's `peer_bucket` tests: the v4 bucket is
    /// the address itself, so two clients share one only by sharing it.
    #[test]
    fn the_bucket_of_a_v4_client_is_its_address() {
        assert_eq!(ClientIp(ip("10.0.0.1")).bucket(), "10.0.0.1");
        assert_eq!(
            ClientIp(ip("10.0.0.1")).bucket(),
            ClientIp(ip("10.0.0.1")).bucket()
        );
        assert_ne!(
            ClientIp(ip("10.0.0.1")).bucket(),
            ClientIp(ip("10.0.0.2")).bucket()
        );
    }

    #[test]
    fn the_bucket_groups_ipv6_per_64_and_mapped_v4_as_v4() {
        let a = ClientIp(ip("2001:db8:1:2::1"));
        let b = ClientIp(ip("2001:db8:1:2:ffff::9"));
        let c = ClientIp(ip("2001:db8:1:3::1"));
        assert_eq!(a.bucket(), b.bucket());
        assert_ne!(a.bucket(), c.bucket());
        assert_eq!(a.bucket(), "2001:db8:1:2::/64");
        assert_eq!(
            ClientIp(ip("::ffff:10.0.0.1")).bucket(),
            ClientIp(ip("10.0.0.1")).bucket()
        );
    }

    #[test]
    fn the_warning_fires_only_for_private_peers_with_xff_and_no_trust() {
        let none = TrustedProxies::parse(None).unwrap();
        for peer in ["10.0.0.1", "127.0.0.1", "fd00::1"] {
            assert!(warrants_proxy_warning(ip(peer), true, &none), "{peer}");
        }
        assert!(!warrants_proxy_warning(ip("10.0.0.1"), false, &none));
        assert!(!warrants_proxy_warning(
            ip("10.0.0.1"),
            true,
            &trusted("10.0.0.0/8")
        ));
        assert!(!warrants_proxy_warning(ip("198.51.100.1"), true, &none));
    }

    fn annotated(peer: &str, xff_value: Option<&str>, t: &TrustedProxies) -> Request {
        let mut builder = axum::http::Request::builder().uri("/");
        if let Some(value) = xff_value {
            builder = builder.header(X_FORWARDED_FOR, value);
        }
        let mut req = builder.body(axum::body::Body::empty()).unwrap();
        let addr: SocketAddr = format!("{peer}:40000").parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        annotate(&mut req, t);
        req
    }

    /// The marker rides beside the `ClientIp` exactly when the warning
    /// predicate holds, and the `ClientIp` itself (so its bucket) is the
    /// same either way.
    #[test]
    fn the_unconfigured_marker_follows_the_warning_condition() {
        let none = TrustedProxies::parse(None).unwrap();
        let behind = annotated("10.0.0.5", Some("198.51.100.7"), &none);
        assert!(behind.extensions().get::<ProxyUnconfigured>().is_some());
        assert_eq!(
            behind.extensions().get::<ClientIp>(),
            Some(&ClientIp(ip("10.0.0.5")))
        );

        let listed = annotated("10.0.0.5", Some("198.51.100.7"), &trusted("10.0.0.0/8"));
        assert!(listed.extensions().get::<ProxyUnconfigured>().is_none());
        assert_eq!(
            listed.extensions().get::<ClientIp>(),
            Some(&ClientIp(ip("198.51.100.7")))
        );

        for req in [
            annotated("10.0.0.5", None, &none),
            annotated("198.51.100.1", Some("192.0.2.1"), &none),
        ] {
            assert!(req.extensions().get::<ProxyUnconfigured>().is_none());
            assert!(req.extensions().get::<ClientIp>().is_some());
        }
    }

    #[test]
    fn the_warning_recurs_at_most_once_per_interval() {
        let last = AtomicU64::new(0);
        let t0 = 1_800_000_000;
        assert!(claim_warning(&last, t0), "the first is always due");
        assert!(!claim_warning(&last, t0));
        assert!(!claim_warning(&last, t0 + WARN_EVERY_SECS - 1));
        assert!(claim_warning(&last, t0 + WARN_EVERY_SECS));
        assert!(!claim_warning(&last, t0 + WARN_EVERY_SECS + 1));
    }

    /// Prefix edges, host bits, and IPv4-mapped entries. Each row is
    /// `(entry, peer, should match)`.
    #[test]
    fn cidr_entries_match_as_written() {
        for (entry, peer, expected) in [
            ("0.0.0.0/0", "203.0.113.9", true),
            ("0.0.0.0/0", "2001:db8::1", false),
            ("::/0", "2001:db8::1", true),
            ("::/0", "203.0.113.9", false),
            ("192.0.2.7/32", "192.0.2.7", true),
            ("192.0.2.7/32", "192.0.2.8", false),
            ("2001:db8::7/128", "2001:db8::7", true),
            ("2001:db8::7/128", "2001:db8::8", false),
            // Host bits below the prefix are ignored.
            ("10.1.2.3/8", "10.200.0.1", true),
            ("10.1.2.3/8", "11.0.0.1", false),
            // Mapped entries become the v4 net they name, and match both a
            // v4 peer and a dual-stack listener's mapped form of it.
            ("::ffff:10.0.0.0/104", "10.20.30.40", true),
            ("::ffff:10.0.0.0/104", "::ffff:10.20.30.40", true),
            ("::ffff:10.0.0.0/104", "11.0.0.1", false),
            ("::ffff:192.0.2.7/128", "192.0.2.7", true),
            ("::ffff:0.0.0.0/96", "198.51.100.1", true),
        ] {
            assert_eq!(
                trusted(entry).contains(ip(peer)),
                expected,
                "{entry} vs {peer}"
            );
        }
        assert_eq!(
            parse_net("::ffff:10.0.0.0/104"),
            parse_net("10.0.0.0/8"),
            "a mapped /104 is a v4 /8"
        );
        assert!(parse_net("0.0.0.0/0").unwrap().is_catch_all());
        assert!(parse_net("::/0").unwrap().is_catch_all());
        assert!(!parse_net("10.0.0.0/8").unwrap().is_catch_all());
    }

    #[test]
    fn a_mapped_prefix_shorter_than_96_is_rejected() {
        for bad in ["::ffff:10.0.0.0/95", "::ffff:0.0.0.0/80"] {
            let msg = TrustedProxies::parse(Some(bad)).unwrap_err().to_string();
            assert!(msg.contains("DOMARINN_TRUSTED_PROXIES"), "{msg}");
            assert!(msg.contains(bad), "{msg}");
            assert!(msg.contains("/96"), "{msg}");
        }
    }
}
