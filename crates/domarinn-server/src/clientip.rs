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
use std::sync::atomic::{AtomicBool, Ordering};

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
    pub fn parse(raw: Option<&str>) -> anyhow::Result<TrustedProxies> {
        let nets = raw
            .into_iter()
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                parse_net(entry).ok_or_else(|| {
                    anyhow::anyhow!(
                        "invalid DOMARINN_TRUSTED_PROXIES entry '{entry}'; expected an IP \
                         address or CIDR such as 10.0.0.0/8 or fd00::/8"
                    )
                })
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

/// `addr` or `addr/prefix`. A bare address is canonicalised like a peer is,
/// so `::ffff:10.0.0.1` is the v4 host it names.
fn parse_net(entry: &str) -> Option<Net> {
    let (addr, prefix) = match entry.split_once('/') {
        Some((addr, prefix)) => (
            addr.parse::<IpAddr>().ok()?,
            Some(prefix.parse::<u32>().ok()?),
        ),
        None => (entry.parse::<IpAddr>().ok()?.to_canonical(), None),
    };
    // `checked_shl` because a /0 shifts by the full width, which `<<` rejects.
    match addr {
        IpAddr::V4(v4) => {
            let prefix = prefix.unwrap_or(32);
            if prefix > 32 {
                return None;
            }
            let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
            Some(Net::V4 {
                addr: u32::from(v4) & mask,
                mask,
            })
        }
        IpAddr::V6(v6) => {
            let prefix = prefix.unwrap_or(128);
            if prefix > 128 {
                return None;
            }
            let mask = u128::MAX.checked_shl(128 - prefix).unwrap_or(0);
            Some(Net::V6 {
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

/// Once per process: the warning is about configuration, which does not
/// change between requests, so repeating it would only bury it.
static WARNED: AtomicBool = AtomicBool::new(false);

/// Resolve the request's [`ClientIp`] into its extensions. Never rejects.
pub(crate) async fn client_ip_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    if let Some(ConnectInfo(peer)) = req.extensions().get::<ConnectInfo<SocketAddr>>().copied() {
        let peer = peer.ip();
        let trusted = state.trusted_proxies.as_ref();
        let has_xff = req.headers().contains_key(X_FORWARDED_FOR);
        if warrants_proxy_warning(peer, has_xff, trusted) && !WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                peer = %peer,
                "a private-range peer sent X-Forwarded-For but DOMARINN_TRUSTED_PROXIES is \
                 empty, so every client behind it shares the proxy's address; list the proxy \
                 there"
            );
        }
        let client = resolve(peer, req.headers(), trusted);
        req.extensions_mut().insert(ClientIp(client));
    }
    next.run(req).await
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
}
