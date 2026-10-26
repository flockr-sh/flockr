//! Mobile gateway state shared by the CLI (`flockr mobile ...`) and the server.
//!
//! Everything here works on files under `state_dir()/mobile/` and needs no
//! running server. The wire contract is `docs/mobile/protocol.md`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub(crate) mod pairing;
pub(crate) mod store;

/// Shown when the gateway listens on a public address.
pub(crate) const PUBLIC_ADDRESS_NOTE: &str =
    "traffic is encrypted, but anyone on the internet can reach the pairing endpoint";

/// Whether `ip` is reachable from the internet: anything but loopback,
/// private (10/8, 172.16/12, 192.168/16), CGNAT/VPN (100.64/10), link-local
/// (169.254/16, fe80::/10), IPv6 ULA (fc00::/7), or unspecified.
pub(crate) fn is_public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(mapped) => is_public_v4(mapped),
            None => is_public_v6(ip),
        },
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    let cgnat = a == 100 && (64..128).contains(&b);
    !(ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified() || cgnat)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    let link_local = first & 0xffc0 == 0xfe80;
    let unique_local = first & 0xfe00 == 0xfc00;
    !(ip.is_loopback() || ip.is_unspecified() || link_local || unique_local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_public_and_private_addresses() {
        for private in [
            "127.0.0.1",
            "127.8.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.10",
            "100.64.0.1",
            "100.127.255.255",
            "169.254.3.4",
            "0.0.0.0",
            "::1",
            "::",
            "fe80::1",
            "febf::1",
            "fc00::1",
            "fd12:3456::1",
            "::ffff:192.168.1.1",
        ] {
            assert!(
                !is_public_address(private.parse().unwrap()),
                "{private} should not be public"
            );
        }
        for public in [
            "8.8.8.8",
            "172.15.0.1",
            "172.32.0.1",
            "100.63.255.255",
            "100.128.0.1",
            "192.169.0.1",
            "2001:db8::1",
            "2606:4700::1111",
            "fec0::1",
            "::ffff:8.8.8.8",
        ] {
            assert!(
                is_public_address(public.parse().unwrap()),
                "{public} should be public"
            );
        }
    }
}
