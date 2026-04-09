//! Network interface address discovery.
//!
//! Enumerates system interfaces to find addresses suitable for advertising
//! as `declared_address` in the handshake. Currently: IPv6 global unicast only.
//! IPv4 NAT traversal is handled by the UPnP module.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};

/// Find the first global unicast IPv6 address on any system interface.
///
/// Skips link-local (fe80::), loopback (::1), and other non-global scopes.
/// Returns a `SocketAddr` combining the discovered IP with the given port.
pub fn find_global_ipv6(port: u16) -> Option<SocketAddr> {
    let ifaces = match if_addrs::get_if_addrs() {
        Ok(addrs) => addrs,
        Err(e) => {
            tracing::info!(error = %e, "IPv6 auto-detect: failed to enumerate interfaces");
            return None;
        }
    };

    for iface in &ifaces {
        if let IpAddr::V6(v6) = iface.ip() {
            if is_global_unicast(v6) {
                tracing::info!(addr = %v6, iface = %iface.name, "IPv6 auto-detect: found global unicast address");
                return Some(SocketAddr::new(IpAddr::V6(v6), port));
            }
        }
    }

    tracing::info!("IPv6 auto-detect: no global unicast address found");
    None
}

/// Check if an IPv6 address is global unicast (not link-local, loopback,
/// multicast, or other special-purpose).
fn is_global_unicast(addr: Ipv6Addr) -> bool {
    // Reject loopback (::1)
    if addr.is_loopback() {
        return false;
    }
    // Reject unspecified (::)
    if addr.is_unspecified() {
        return false;
    }
    // Reject multicast (ff00::/8)
    if addr.is_multicast() {
        return false;
    }
    // Reject link-local (fe80::/10)
    let segments = addr.segments();
    if segments[0] & 0xffc0 == 0xfe80 {
        return false;
    }
    // Reject unique-local (fc00::/7) — not globally routable
    if segments[0] & 0xfe00 == 0xfc00 {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_unicast_accepted() {
        // 2001:db8::1 is documentation range, but structurally global unicast
        let addr: Ipv6Addr = "2001:db8::1".parse().unwrap();
        assert!(is_global_unicast(addr));
    }

    #[test]
    fn link_local_rejected() {
        let addr: Ipv6Addr = "fe80::1".parse().unwrap();
        assert!(!is_global_unicast(addr));
    }

    #[test]
    fn loopback_rejected() {
        let addr: Ipv6Addr = "::1".parse().unwrap();
        assert!(!is_global_unicast(addr));
    }

    #[test]
    fn unique_local_rejected() {
        let addr: Ipv6Addr = "fd00::1".parse().unwrap();
        assert!(!is_global_unicast(addr));
    }

    #[test]
    fn multicast_rejected() {
        let addr: Ipv6Addr = "ff02::1".parse().unwrap();
        assert!(!is_global_unicast(addr));
    }

    #[test]
    fn unspecified_rejected() {
        let addr: Ipv6Addr = "::".parse().unwrap();
        assert!(!is_global_unicast(addr));
    }
}
