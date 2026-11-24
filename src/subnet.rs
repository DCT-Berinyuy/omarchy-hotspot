//! Picks the hotspot's address range.
//!
//! create_ap gives the hotspot `<gateway>/24`, 192.168.12.1 unless told
//! otherwise, without checking the networks the computer is already on. When
//! one of them uses those addresses, the computer takes an address that
//! belongs to that network (often its router's), and hotspot devices handed
//! an address the other network covers too get no internet.

use std::fs;
use std::net::Ipv4Addr;

const PROC_NET_ROUTE: &str = "/proc/net/route";
/// create_ap's default, kept whenever it's free.
const DEFAULT_GATEWAY: Ipv4Addr = Ipv4Addr::new(192, 168, 12, 1);
/// create_ap always uses a /24 around its gateway.
const HOTSPOT_PREFIX: u32 = 24;

/// The hotspot's address range.
#[derive(Debug)]
pub struct Range {
    pub gateway: Ipv4Addr,
    /// The interface whose network rules out create_ap's default range.
    default_taken_by: Option<String>,
}

impl Range {
    /// For the configuration summary, e.g.
    /// `192.168.13.x (192.168.12.x is in use on enp0s31f6)`.
    pub fn describe(&self) -> String {
        let range = as_pattern(self.gateway);
        match &self.default_taken_by {
            None => range,
            Some(iface) => format!(
                "{range} ({} is in use on {iface})",
                as_pattern(DEFAULT_GATEWAY)
            ),
        }
    }
}

fn as_pattern(gateway: Ipv4Addr) -> String {
    let [a, b, c, _] = gateway.octets();
    format!("{a}.{b}.{c}.x")
}

/// create_ap's default range unless a network this computer is on uses it,
/// otherwise the first free one. `None` when every candidate is taken.
pub fn pick() -> Option<Range> {
    let table = fs::read_to_string(PROC_NET_ROUTE).unwrap_or_default();
    pick_from(&parse_routes(&table))
}

fn pick_from(routes: &[Route]) -> Option<Range> {
    let gateway = candidates().find(|&gateway| conflict(routes, gateway).is_none())?;
    let default_taken_by = conflict(routes, DEFAULT_GATEWAY).map(|route| route.iface.clone());
    Some(Range {
        gateway,
        default_taken_by,
    })
}

/// 192.168.12.0/24 (create_ap's default) and up, then NetworkManager's
/// shared-connection range for networks that use all of 192.168.0.0/16.
fn candidates() -> impl Iterator<Item = Ipv4Addr> {
    (12..=254)
        .map(|c| Ipv4Addr::new(192, 168, c, 1))
        .chain((0..=255).map(|c| Ipv4Addr::new(10, 42, c, 1)))
}

#[derive(Debug, PartialEq, Eq)]
struct Route {
    iface: String,
    destination: Ipv4Addr,
    prefix: u32,
    gateway: Option<Ipv4Addr>,
}

/// The first route that rules out `gateway`'s /24:
/// - a gateway inside it, which would be cut off;
/// - a directly attached network that overlaps it;
/// - a route at least as specific as /24 that overlaps it, which would take
///   traffic meant for hotspot devices.
///
/// Broader routes through a gateway, like a VPN's 0.0.0.0/1, don't count: the
/// hotspot's /24 is more specific, so its devices stay reachable.
fn conflict(routes: &[Route], gateway: Ipv4Addr) -> Option<&Route> {
    routes.iter().find(|route| {
        let gateway_inside = route
            .gateway
            .is_some_and(|via| same_prefix(via, gateway, HOTSPOT_PREFIX));
        let overlaps = route.prefix > 0
            && same_prefix(route.destination, gateway, route.prefix.min(HOTSPOT_PREFIX));
        gateway_inside || (overlaps && (route.gateway.is_none() || route.prefix >= HOTSPOT_PREFIX))
    })
}

/// Whether `a` and `b` share their first `len` bits.
fn same_prefix(a: Ipv4Addr, b: Ipv4Addr, len: u32) -> bool {
    let mask = u32::MAX.checked_shl(32 - len).unwrap_or(0);
    u32::from(a) & mask == u32::from(b) & mask
}

/// Parses `/proc/net/route` (the main routing table). It prints addresses as
/// hex in memory order: network byte order read as a native integer.
fn parse_routes(table: &str) -> Vec<Route> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 8 {
                return None;
            }
            let gateway = hex_addr(fields[2])?;
            Some(Route {
                iface: fields[0].to_string(),
                destination: hex_addr(fields[1])?,
                prefix: u32::from(hex_addr(fields[7])?).leading_ones(),
                gateway: (!gateway.is_unspecified()).then_some(gateway),
            })
        })
        .collect()
}

fn hex_addr(hex: &str) -> Option<Ipv4Addr> {
    u32::from_str_radix(hex, 16)
        .ok()
        .map(|value| Ipv4Addr::from(value.to_ne_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(iface: &str, destination: [u8; 4], prefix: u32, gateway: Option<[u8; 4]>) -> Route {
        Route {
            iface: iface.to_string(),
            destination: destination.into(),
            prefix,
            gateway: gateway.map(Ipv4Addr::from),
        }
    }

    /// A wired network whose router has create_ap's default address.
    fn wired_network_on_the_default_range() -> [Route; 2] {
        [
            route("enp0s31f6", [0, 0, 0, 0], 0, Some([192, 168, 12, 1])),
            route("enp0s31f6", [192, 168, 12, 0], 26, None),
        ]
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn parses_the_kernel_routing_table() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                     enp0s31f6\t00000000\t010CA8C0\t0003\t0\t0\t20100\t00000000\t0\t0\t0\n\
                     enp0s31f6\t000CA8C0\t00000000\t0001\t0\t0\t100\tC0FFFFFF\t0\t0\t0\n";
        assert_eq!(parse_routes(table), wired_network_on_the_default_range());
    }

    #[test]
    fn moves_off_a_network_that_uses_the_default_range() {
        let range = pick_from(&wired_network_on_the_default_range()).unwrap();
        assert_eq!(range.gateway, Ipv4Addr::new(192, 168, 13, 1));
        assert_eq!(
            range.describe(),
            "192.168.13.x (192.168.12.x is in use on enp0s31f6)"
        );
    }

    #[test]
    fn keeps_the_default_range_beside_other_networks_and_broad_vpn_routes() {
        let routes = [
            route("wlp4s0", [0, 0, 0, 0], 0, Some([192, 168, 0, 254])),
            route("wlp4s0", [192, 168, 0, 0], 24, None),
            // OpenVPN's redirect-gateway def1 covers everything, through a gateway.
            route("tun0", [0, 0, 0, 0], 1, Some([10, 8, 0, 1])),
            route("tun0", [128, 0, 0, 0], 1, Some([10, 8, 0, 1])),
        ];
        let range = pick_from(&routes).unwrap();
        assert_eq!(range.gateway, DEFAULT_GATEWAY);
        assert_eq!(range.describe(), "192.168.12.x");
    }

    #[test]
    fn specific_routes_through_a_gateway_rule_a_range_out() {
        let routes = [route("tun0", [192, 168, 12, 0], 25, Some([10, 8, 0, 1]))];
        assert_eq!(
            pick_from(&routes).unwrap().gateway,
            Ipv4Addr::new(192, 168, 13, 1)
        );
    }

    #[test]
    fn leaves_192_168_when_a_network_uses_all_of_it() {
        let routes = [route("enp0s31f6", [192, 168, 0, 0], 16, None)];
        assert_eq!(
            pick_from(&routes).unwrap().gateway,
            Ipv4Addr::new(10, 42, 0, 1)
        );
    }
}
