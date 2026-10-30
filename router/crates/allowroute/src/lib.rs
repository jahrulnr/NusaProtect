//! Decide whether a DNS answer stays on the normal network or fails over to the tunnel.
//!
//! A name that resolves to a public address stays direct. Any other result
//! (NXDOMAIN, timeout, or only non-public addresses such as 10.0.0.0/8)
//! fails over to the tunnel resolver.

use std::net::IpAddr;

/// Outcome of comparing a system-DNS answer with a tunnel-DNS answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// True when the tunnel answer should be returned and routed via the tunnel.
    pub use_tunnel: bool,
    /// Addresses the client should use. Empty means keep the original failure.
    pub addresses: Vec<IpAddr>,
}

/// True for globally routable unicast. Private, loopback, link-local, CGNAT,
/// and documentation ranges are not public.
pub fn is_public_unicast(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !v4.is_private()
                && !v4.is_loopback()
                && !v4.is_link_local()
                && !v4.is_broadcast()
                && !v4.is_unspecified()
                && !v4.is_multicast()
                && !is_cgnat(o)
                && !is_v4_documentation(o)
                && o[0] != 0
        }
        IpAddr::V6(v6) => {
            !v6.is_loopback()
                && !v6.is_unspecified()
                && !v6.is_multicast()
                && !is_ula(v6.segments()[0])
                && !is_v6_link_local(v6.segments()[0])
                && !is_v6_documentation_addr(ip)
        }
    }
}

fn is_cgnat(o: [u8; 4]) -> bool {
    o[0] == 100 && (o[1] & 0xc0) == 64
}

fn is_v4_documentation(o: [u8; 4]) -> bool {
    (o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
}

fn is_ula(first: u16) -> bool {
    (first & 0xfe00) == 0xfc00
}

fn is_v6_link_local(first: u16) -> bool {
    (first & 0xffc0) == 0xfe80
}

/// `2001:db8::/32` is documentation. The rest of `2001::/16` is public space.
fn is_v6_documentation_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            s[0] == 0x2001 && s[1] == 0x0db8
        }
        IpAddr::V4(_) => false,
    }
}

/// System DNS wins when it produced a public address. Otherwise use the tunnel answer.
pub fn decide(system: &[IpAddr], tunnel: &[IpAddr]) -> Decision {
    let public: Vec<IpAddr> = system
        .iter()
        .copied()
        .filter(|ip| is_public_unicast(*ip))
        .collect();
    if !public.is_empty() {
        return Decision {
            use_tunnel: false,
            addresses: public,
        };
    }
    let tunnel_addrs: Vec<IpAddr> = tunnel
        .iter()
        .copied()
        .filter(|ip| !ip.is_unspecified())
        .collect();
    Decision {
        use_tunnel: !tunnel_addrs.is_empty(),
        addresses: tunnel_addrs,
    }
}

/// Question name and address records from a DNS message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsView {
    pub qname: String,
    pub answers: Vec<IpAddr>,
}

/// Read the first question and any A/AAAA answers. Compressed names are followed.
pub fn inspect_dns(packet: &[u8]) -> Option<DnsView> {
    if packet.len() < 12 {
        return None;
    }
    let qd = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    let an = u16::from_be_bytes([packet[6], packet[7]]) as usize;
    let mut off = 12usize;
    let mut qname = String::new();
    for i in 0..qd {
        let (name, next) = read_name(packet, off)?;
        if next + 4 > packet.len() {
            return None;
        }
        if i == 0 {
            qname = name;
        }
        off = next + 4;
    }
    let mut answers = Vec::new();
    for _ in 0..an {
        let (_name, next) = read_name(packet, off)?;
        if next + 10 > packet.len() {
            return None;
        }
        let rtype = u16::from_be_bytes([packet[next], packet[next + 1]]);
        let rdlen = u16::from_be_bytes([packet[next + 8], packet[next + 9]]) as usize;
        let rdata = next + 10;
        if rdata + rdlen > packet.len() {
            return None;
        }
        if rtype == 1 && rdlen == 4 {
            let o: [u8; 4] = packet[rdata..rdata + 4].try_into().ok()?;
            answers.push(IpAddr::from(o));
        } else if rtype == 28 && rdlen == 16 {
            let o: [u8; 16] = packet[rdata..rdata + 16].try_into().ok()?;
            answers.push(IpAddr::from(o));
        }
        off = rdata + rdlen;
    }
    Some(DnsView { qname, answers })
}

fn read_name(packet: &[u8], offset: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut pos = offset;
    let mut end = offset;
    let mut jumped = false;
    for _ in 0..16 {
        if pos >= packet.len() {
            return None;
        }
        let len = packet[pos];
        if len == 0 {
            if !jumped {
                end = pos + 1;
            }
            return Some((labels.join("."), end));
        }
        if len & 0xc0 == 0xc0 {
            if pos + 1 >= packet.len() {
                return None;
            }
            if !jumped {
                end = pos + 2;
            }
            pos = (((len as usize) & 0x3f) << 8) | packet[pos + 1] as usize;
            jumped = true;
            continue;
        }
        if len & 0xc0 != 0 {
            return None;
        }
        let start = pos + 1;
        let stop = start + len as usize;
        if stop > packet.len() {
            return None;
        }
        labels.push(String::from_utf8_lossy(&packet[start..stop]).to_ascii_lowercase());
        pos = stop;
        if !jumped {
            end = pos;
        }
    }
    None
}

/// True when `ip` is already covered by an aggregate such as `10.0.0.0/8`.
pub fn prefix_covers(prefix: &str, ip: IpAddr) -> bool {
    let Some((addr, len)) = prefix.split_once('/') else {
        return false;
    };
    let Ok(len) = len.parse::<u8>() else {
        return false;
    };
    match (addr.parse::<IpAddr>(), ip) {
        (Ok(IpAddr::V4(net)), IpAddr::V4(host)) if len <= 32 => {
            let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
            (u32::from(net) & mask) == (u32::from(host) & mask)
        }
        (Ok(IpAddr::V6(net)), IpAddr::V6(host)) if len <= 128 => {
            let net = u128::from(net);
            let host = u128::from(host);
            let mask = if len == 0 { 0 } else { u128::MAX << (128 - len) };
            (net & mask) == (host & mask)
        }
        _ => false,
    }
}

/// True when `ip route get` output resolves through `dev` (e.g. the tunnel or
/// `lo` for a local address). Used to keep resolvers that only exist behind
/// the tunnel out of the underlay list.
pub fn route_uses_dev(route_get: &str, dev: &str) -> bool {
    let mut parts = route_get.split_whitespace();
    while let Some(token) = parts.next() {
        if token == "dev" {
            return parts.next() == Some(dev);
        }
    }
    false
}

/// Host route for one answered address.
pub fn host_route(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => format!("{v4}/32"),
        IpAddr::V6(v6) => format!("{v6}/128"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn public_and_private_addresses() {
        assert!(is_public_unicast(v4(1, 1, 1, 1)));
        assert!(is_public_unicast(v4(8, 8, 8, 8)));
        assert!(is_public_unicast(v4(172, 15, 0, 1)));
        assert!(!is_public_unicast(v4(10, 1, 2, 3)));
        assert!(!is_public_unicast(v4(10, 255, 255, 255)));
        assert!(!is_public_unicast(v4(172, 16, 0, 1)));
        assert!(!is_public_unicast(v4(192, 168, 1, 1)));
        assert!(!is_public_unicast(v4(100, 64, 0, 1)));
        assert!(!is_public_unicast(v4(127, 0, 0, 1)));
        assert!(!is_public_unicast(v4(169, 254, 1, 1)));
        assert!(!is_public_unicast(v4(203, 0, 113, 1)));
        assert!(is_public_unicast(IpAddr::V6(Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888))));
        assert!(!is_public_unicast(IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1))));
        assert!(!is_public_unicast("fc00::1".parse().unwrap()));
        assert!(!is_public_unicast("fe80::1".parse().unwrap()));
    }

    #[test]
    fn public_answer_stays_direct() {
        let system = [v4(1, 1, 1, 1), v4(10, 1, 2, 3)];
        let tunnel = [v4(10, 9, 9, 9)];
        let decision = decide(&system, &tunnel);
        assert!(!decision.use_tunnel);
        assert_eq!(decision.addresses, vec![v4(1, 1, 1, 1)]);
    }

    #[test]
    fn missing_public_answer_uses_tunnel() {
        let decision = decide(&[v4(10, 0, 0, 5)], &[v4(10, 8, 0, 9)]);
        assert!(decision.use_tunnel);
        assert_eq!(decision.addresses, vec![v4(10, 8, 0, 9)]);
    }

    #[test]
    fn empty_tunnel_does_not_claim_a_route() {
        let decision = decide(&[], &[]);
        assert!(!decision.use_tunnel);
        assert!(decision.addresses.is_empty());
    }

    #[test]
    fn parses_uncompressed_a_record() {
        let mut pkt = vec![0x12, 0x34, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0];
        push_name(&mut pkt, "example.com");
        pkt.extend_from_slice(&[0, 1, 0, 1]);
        push_name(&mut pkt, "example.com");
        pkt.extend_from_slice(&[0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 1, 2, 3, 4]);
        let view = inspect_dns(&pkt).unwrap();
        assert_eq!(view.qname, "example.com");
        assert_eq!(view.answers, vec![v4(1, 2, 3, 4)]);
    }

    #[test]
    fn parses_compressed_answer_name() {
        let mut pkt = vec![0x12, 0x34, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0];
        push_name(&mut pkt, "private.domain");
        pkt.extend_from_slice(&[0, 1, 0, 1]);
        pkt.extend_from_slice(&[0xc0, 0x0c]);
        pkt.extend_from_slice(&[0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 10, 1, 2, 3]);
        let view = inspect_dns(&pkt).unwrap();
        assert_eq!(view.qname, "private.domain");
        assert_eq!(view.answers, vec![v4(10, 1, 2, 3)]);
        assert_eq!(host_route(v4(10, 1, 2, 3)), "10.1.2.3/32");
        assert!(prefix_covers("10.0.0.0/8", v4(10, 1, 2, 3)));
        assert!(!prefix_covers("10.0.0.0/8", v4(11, 0, 0, 1)));
        assert!(!prefix_covers("10.1.2.0/24", v4(10, 1, 3, 1)));
    }

    #[test]
    fn detects_route_device() {
        assert!(route_uses_dev(
            "100.127.46.1 dev tun0 src 100.127.9.180 uid 0 \n    cache ",
            "tun0"
        ));
        assert!(route_uses_dev(
            "local 100.127.9.180 dev lo src 100.127.9.180 uid 0",
            "lo"
        ));
        assert!(!route_uses_dev(
            "203.142.82.222 via 192.168.18.1 dev wlp2s0 src 192.168.18.150 uid 0",
            "tun0"
        ));
        assert!(!route_uses_dev("broadcast 127.255.255.255 dev lo", "tun0"));
        assert!(!route_uses_dev("unreachable", "tun0"));
    }

    fn push_name(buf: &mut Vec<u8>, name: &str) {
        for label in name.split('.') {
            buf.push(label.len() as u8);
            buf.extend(label.as_bytes());
        }
        buf.push(0);
    }
}
