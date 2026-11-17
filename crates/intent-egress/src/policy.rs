//! The address policy (D-012): only public unicast addresses may be connected to.
//!
//! An allowlist for IPv6 (global unicast, `2000::/3`, minus the special blocks inside it) and a
//! denylist for IPv4 that covers every special-purpose block IANA lists as not globally
//! reachable. IPv4 addresses embedded in IPv6 (mapped, NAT64, 6to4) are judged as the IPv4
//! address they carry. There is no switch that relaxes it: tests use fixture resolvers and
//! connectors instead (D-012, D-060).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Why an address is never connected to. Stable codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forbidden {
    /// `0.0.0.0/8`, `::`.
    Unspecified,
    /// `127.0.0.0/8`, `::1`.
    Loopback,
    /// RFC 1918: `10/8`, `172.16/12`, `192.168/16`.
    Private,
    /// `100.64.0.0/10` (carrier-grade NAT).
    SharedAddressSpace,
    /// `169.254.0.0/16` (cloud metadata lives here), `fe80::/10`.
    LinkLocal,
    /// `fc00::/7`.
    UniqueLocal,
    /// `224.0.0.0/4`, `ff00::/8`.
    Multicast,
    /// `192.0.2/24`, `198.51.100/24`, `203.0.113/24`, `2001:db8::/32`.
    Documentation,
    /// `198.18.0.0/15`.
    Benchmarking,
    /// Everything else not globally reachable: `192.0.0/24`, `192.88.99/24`, `240/4` and the
    /// broadcast address, IPv4-compatible IPv6, `100::/64`, `2001::/23`, site-local, and any
    /// IPv6 outside `2000::/3`.
    Reserved,
}

impl Forbidden {
    /// The wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::SharedAddressSpace => "shared_address_space",
            Self::LinkLocal => "link_local",
            Self::UniqueLocal => "unique_local",
            Self::Multicast => "multicast",
            Self::Documentation => "documentation",
            Self::Benchmarking => "benchmarking",
            Self::Reserved => "reserved",
        }
    }
}

/// `Ok` for a public unicast address; otherwise why not.
pub fn check(ip: IpAddr) -> Result<(), Forbidden> {
    match ip {
        IpAddr::V4(v4) => check_v4(v4),
        IpAddr::V6(v6) => check_v6(v6),
    }
}

fn in_v4(ip: Ipv4Addr, net: [u8; 4], prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    (u32::from(ip) & mask) == (u32::from(Ipv4Addr::from(net)) & mask)
}

fn check_v4(ip: Ipv4Addr) -> Result<(), Forbidden> {
    use Forbidden::{
        Benchmarking, Documentation, LinkLocal, Loopback, Multicast, Private, Reserved,
        SharedAddressSpace, Unspecified,
    };
    let blocks: [([u8; 4], u32, Forbidden); 16] = [
        ([0, 0, 0, 0], 8, Unspecified),
        ([10, 0, 0, 0], 8, Private),
        ([100, 64, 0, 0], 10, SharedAddressSpace),
        ([127, 0, 0, 0], 8, Loopback),
        ([169, 254, 0, 0], 16, LinkLocal),
        ([172, 16, 0, 0], 12, Private),
        ([192, 0, 0, 0], 24, Reserved),
        ([192, 0, 2, 0], 24, Documentation),
        ([192, 88, 99, 0], 24, Reserved),
        ([192, 168, 0, 0], 16, Private),
        ([198, 18, 0, 0], 15, Benchmarking),
        ([198, 51, 100, 0], 24, Documentation),
        ([203, 0, 113, 0], 24, Documentation),
        ([224, 0, 0, 0], 4, Multicast),
        ([240, 0, 0, 0], 4, Reserved),
        ([255, 255, 255, 255], 32, Reserved),
    ];
    match blocks
        .iter()
        .find(|(net, prefix, _)| in_v4(ip, *net, *prefix))
    {
        Some((_, _, why)) => Err(*why),
        None => Ok(()),
    }
}

fn in_v6(ip: Ipv6Addr, net: u128, prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    (u128::from(ip) & mask) == (net & mask)
}

/// The IPv4 address in the low 32 bits.
fn low_v4(ip: Ipv6Addr) -> Ipv4Addr {
    let bits = u128::from(ip);
    // The mask keeps 32 bits, so the conversion cannot fail.
    Ipv4Addr::from(u32::try_from(bits & 0xFFFF_FFFF).unwrap_or_default())
}

fn check_v6(ip: Ipv6Addr) -> Result<(), Forbidden> {
    use Forbidden::{
        Documentation, LinkLocal, Loopback, Multicast, Reserved, UniqueLocal, Unspecified,
    };
    if ip.is_unspecified() {
        return Err(Unspecified);
    }
    if ip.is_loopback() {
        return Err(Loopback);
    }
    // ::ffff:a.b.c.d — the IPv4 address is what is reached.
    if in_v6(ip, 0xFFFF_0000_0000, 96) {
        return check_v4(low_v4(ip));
    }
    // 64:ff9b::/96 (NAT64): judged by the embedded IPv4 address.
    if in_v6(ip, 0x0064_ff9b_0000_0000_0000_0000_0000_0000, 96) {
        return check_v4(low_v4(ip));
    }
    // 2002::/16 (6to4): the IPv4 address is bits 16..48.
    if in_v6(ip, 0x2002_0000_0000_0000_0000_0000_0000_0000, 16) {
        let embedded = u32::try_from((u128::from(ip) >> 80) & 0xFFFF_FFFF).unwrap_or_default();
        return check_v4(Ipv4Addr::from(embedded));
    }
    let blocks: [(u128, u32, Forbidden); 8] = [
        (0, 96, Reserved),                                         // IPv4-compatible
        (0x0064_ff9b_0001_0000_0000_0000_0000_0000, 48, Reserved), // local-use NAT64
        (0x0100_0000_0000_0000_0000_0000_0000_0000, 64, Reserved), // discard
        (0x2001_0db8_0000_0000_0000_0000_0000_0000, 32, Documentation),
        (0x2001_0000_0000_0000_0000_0000_0000_0000, 23, Reserved), // IETF protocol assignments, Teredo
        (0xfc00_0000_0000_0000_0000_0000_0000_0000, 7, UniqueLocal),
        (0xfe80_0000_0000_0000_0000_0000_0000_0000, 10, LinkLocal),
        (0xff00_0000_0000_0000_0000_0000_0000_0000, 8, Multicast),
    ];
    if let Some((_, _, why)) = blocks
        .iter()
        .find(|(net, prefix, _)| in_v6(ip, *net, *prefix))
    {
        return Err(*why);
    }
    // Everything else must be global unicast, 2000::/3 (site-local fec0::/10 and every other
    // unassigned block fall here).
    if in_v6(ip, 0x2000_0000_0000_0000_0000_0000_0000_0000, 3) {
        Ok(())
    } else {
        Err(Reserved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Forbidden::{
        Benchmarking, Documentation, LinkLocal, Loopback, Multicast, Private, Reserved,
        SharedAddressSpace, UniqueLocal, Unspecified,
    };

    fn verdict(text: &str) -> Result<(), Forbidden> {
        check(text.parse().unwrap())
    }

    #[test]
    fn every_special_purpose_block_is_refused_at_its_edges() {
        let cases = [
            ("0.0.0.0", Unspecified),
            ("0.255.255.255", Unspecified),
            ("10.0.0.0", Private),
            ("10.255.255.255", Private),
            ("100.64.0.0", SharedAddressSpace),
            ("100.127.255.255", SharedAddressSpace),
            ("127.0.0.1", Loopback),
            ("127.255.255.254", Loopback),
            ("169.254.169.254", LinkLocal),
            ("172.16.0.0", Private),
            ("172.31.255.255", Private),
            ("192.0.0.170", Reserved),
            ("192.0.2.1", Documentation),
            ("192.88.99.1", Reserved),
            ("192.168.1.1", Private),
            ("198.18.0.1", Benchmarking),
            ("198.19.255.255", Benchmarking),
            ("198.51.100.7", Documentation),
            ("203.0.113.9", Documentation),
            ("224.0.0.1", Multicast),
            ("239.255.255.255", Multicast),
            ("240.0.0.1", Reserved),
            ("255.255.255.255", Reserved),
            ("::", Unspecified),
            ("::1", Loopback),
            ("::ffff:127.0.0.1", Loopback),
            ("::ffff:169.254.169.254", LinkLocal),
            ("::ffff:10.1.2.3", Private),
            ("::10.1.2.3", Reserved),
            ("64:ff9b::a9fe:a9fe", LinkLocal),
            ("64:ff9b:1::1", Reserved),
            ("100::1", Reserved),
            ("2001::1", Reserved),
            ("2001:db8::1", Documentation),
            ("2002:7f00:0001::1", Loopback),
            ("2002:c0a8:0101::1", Private),
            ("fc00::1", UniqueLocal),
            ("fd12:3456::1", UniqueLocal),
            ("fe80::1", LinkLocal),
            ("febf::1", LinkLocal),
            ("fec0::1", Reserved),
            ("ff02::1", Multicast),
            ("4000::1", Reserved),
        ];
        for (address, why) in cases {
            assert_eq!(verdict(address), Err(why), "{address}");
        }
    }

    #[test]
    fn public_unicast_passes_next_to_the_edges() {
        for address in [
            "1.1.1.1",
            "9.255.255.255",
            "11.0.0.0",
            "100.63.255.255",
            "100.128.0.0",
            "126.255.255.255",
            "128.0.0.0",
            "169.253.255.255",
            "172.15.255.255",
            "172.32.0.0",
            "192.167.255.255",
            "192.169.0.0",
            "198.17.255.255",
            "198.20.0.0",
            "223.255.255.255",
            "::ffff:93.184.215.14",
            "64:ff9b::5db8:d70e",
            "2002:5db8:d70e::1",
            "2001:200::1",
            "2001:4860:4860::8888",
            "2606:4700:4700::1111",
            "2a00:1450:4009:81f::200e",
        ] {
            assert_eq!(verdict(address), Ok(()), "{address}");
        }
    }
}
