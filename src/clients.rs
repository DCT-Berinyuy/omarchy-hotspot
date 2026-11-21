//! Live list of devices connected to the hotspot.
//!
//! hostapd prints `<iface>: AP-STA-CONNECTED <mac>` and `AP-STA-DISCONNECTED`
//! lines, which create_ap passes through to its stdout. Names and IPs come from
//! dnsmasq's lease file in create_ap's config dir. A device asks for its lease
//! just after connecting, so each join is announced once its lease appears, or
//! after a short wait without one.

use std::fmt;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// How long to wait for a new device's DHCP lease before announcing it by MAC address.
const LEASE_WAIT: Duration = Duration::from_secs(8);
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const MAX_HOSTNAME_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Station {
    Connected(String),
    Disconnected(String),
}

pub enum Message {
    LeaseFile(PathBuf),
    Station(Station),
}

/// Parses hostapd's join and leave lines; MAC addresses are lowercased.
pub fn parse_station_event(line: &str) -> Option<Station> {
    let mut tokens = line.split_whitespace();
    while let Some(token) = tokens.next() {
        let station: fn(String) -> Station = match token {
            "AP-STA-CONNECTED" => Station::Connected,
            "AP-STA-DISCONNECTED" => Station::Disconnected,
            _ => continue,
        };
        let mac = tokens.next().filter(|mac| is_mac_address(mac))?;
        return Some(station(mac.to_ascii_lowercase()));
    }
    None
}

/// create_ap prints `Config dir: <dir>` at startup; dnsmasq keeps its leases there.
pub fn parse_lease_file(line: &str) -> Option<PathBuf> {
    let dir = Path::new(line.strip_prefix("Config dir: ")?.trim());
    dir.is_absolute().then(|| dir.join("dnsmasq.leases"))
}

/// Prints a line whenever a device joins or leaves, until `messages` is closed.
pub fn report_changes(messages: Receiver<Message>, running: Arc<AtomicBool>) {
    let mut tracker = Tracker::default();
    let mut lease_file: Option<PathBuf> = None;
    loop {
        let mut changes = Vec::new();
        match messages.recv_timeout(POLL_INTERVAL) {
            Ok(Message::LeaseFile(path)) => lease_file = Some(path),
            Ok(Message::Station(station)) => {
                changes.extend(tracker.record(station, Instant::now()));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if tracker.has_pending() {
            let leases = lease_file
                .as_deref()
                .and_then(|path| fs::read_to_string(path).ok())
                .unwrap_or_default();
            changes.extend(tracker.announce(&leases, Instant::now()));
        }
        // Stay quiet once shutdown starts: hostapd drops every device as it stops.
        if running.load(Ordering::SeqCst) {
            for change in changes {
                outln!("{change}");
            }
        }
    }
}

fn is_mac_address(value: &str) -> bool {
    let parts: Vec<&str> = value.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Lease {
    ip: IpAddr,
    hostname: Option<String>,
}

/// dnsmasq lease lines are `<expiry> <mac> <ip> <hostname or *> <client id or *>`.
fn find_lease(leases: &str, mac: &str) -> Option<Lease> {
    leases.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 || !fields[1].eq_ignore_ascii_case(mac) {
            return None;
        }
        Some(Lease {
            ip: fields[2].parse().ok()?,
            hostname: sanitize_hostname(fields[3]),
        })
    })
}

/// The connecting device chooses its hostname, so keep only characters that
/// are safe to print to a terminal.
fn sanitize_hostname(name: &str) -> Option<String> {
    let clean: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(MAX_HOSTNAME_LEN)
        .collect();
    (!clean.is_empty()).then_some(clean)
}

struct Device {
    mac: String,
    connected_at: Instant,
    lease: Option<Lease>,
    announced: bool,
}

impl Device {
    fn label(&self) -> String {
        match &self.lease {
            Some(Lease {
                ip,
                hostname: Some(name),
            }) => format!("{name} ({ip})"),
            Some(Lease { ip, hostname: None }) => format!("{} ({ip})", self.mac),
            None => self.mac.clone(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Change {
    Joined { device: String, connected: usize },
    Left { device: String, connected: usize },
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (sign, device, verb, connected) = match self {
            Change::Joined { device, connected } => ('+', device, "joined", *connected),
            Change::Left { device, connected } => ('-', device, "left", *connected),
        };
        let noun = if connected == 1 { "device" } else { "devices" };
        write!(
            f,
            "   {sign} {device} {verb} · {connected} {noun} connected"
        )
    }
}

/// Only announced devices are counted, so the count never includes a device
/// that was never shown joining.
#[derive(Default)]
struct Tracker {
    devices: Vec<Device>,
}

impl Tracker {
    fn count(&self) -> usize {
        self.devices
            .iter()
            .filter(|device| device.announced)
            .count()
    }

    fn has_pending(&self) -> bool {
        self.devices.iter().any(|device| !device.announced)
    }

    fn record(&mut self, station: Station, now: Instant) -> Option<Change> {
        match station {
            Station::Connected(mac) => {
                if !self.devices.iter().any(|device| device.mac == mac) {
                    self.devices.push(Device {
                        mac,
                        connected_at: now,
                        lease: None,
                        announced: false,
                    });
                }
                None
            }
            Station::Disconnected(mac) => {
                let index = self.devices.iter().position(|device| device.mac == mac)?;
                let device = self.devices.remove(index);
                device.announced.then(|| Change::Left {
                    device: device.label(),
                    connected: self.count(),
                })
            }
        }
    }

    /// Announces devices whose lease has appeared or whose wait is over.
    fn announce(&mut self, leases: &str, now: Instant) -> Vec<Change> {
        let mut connected = self.count();
        let mut changes = Vec::new();
        for device in self.devices.iter_mut().filter(|device| !device.announced) {
            device.lease = find_lease(leases, &device.mac);
            if device.lease.is_some()
                || now.saturating_duration_since(device.connected_at) >= LEASE_WAIT
            {
                device.announced = true;
                connected += 1;
                changes.push(Change::Joined {
                    device: device.label(),
                    connected,
                });
            }
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: &str = "3a:1f:22:aa:bb:9c";
    const LAPTOP: &str = "d4:5e:01:02:03:04";
    const PHONE_LEASE: &str =
        "1795230000 3a:1f:22:aa:bb:9c 192.168.12.34 Pixel-7 01:3a:1f:22:aa:bb:9c\n";

    #[test]
    fn parses_hostapd_join_and_leave_lines() {
        assert_eq!(
            parse_station_event("ap0: AP-STA-CONNECTED 3A:1F:22:AA:BB:9C"),
            Some(Station::Connected(PHONE.into()))
        );
        assert_eq!(
            parse_station_event("ap0: AP-STA-DISCONNECTED 3a:1f:22:aa:bb:9c"),
            Some(Station::Disconnected(PHONE.into()))
        );
        assert_eq!(parse_station_event("ap0: AP-ENABLED"), None);
        assert_eq!(parse_station_event("ap0: AP-STA-CONNECTED not-a-mac"), None);
    }

    #[test]
    fn finds_the_lease_file_in_create_aps_config_dir() {
        assert_eq!(
            parse_lease_file("Config dir: /tmp/create_ap.wlp4s0.conf.brguZ5Rf"),
            Some(PathBuf::from(
                "/tmp/create_ap.wlp4s0.conf.brguZ5Rf/dnsmasq.leases"
            ))
        );
        assert_eq!(parse_lease_file("PID: 95149"), None);
    }

    #[test]
    fn hostnames_from_devices_are_sanitised() {
        let hostile = "1795230000 3a:1f:22:aa:bb:9c 192.168.12.34 evil\x1b[2Jname *";
        assert_eq!(
            find_lease(hostile, PHONE).and_then(|lease| lease.hostname),
            Some("evil2Jname".to_string())
        );
        let unnamed = "1795230000 3a:1f:22:aa:bb:9c 192.168.12.34 * *";
        assert_eq!(
            find_lease(unnamed, PHONE).and_then(|lease| lease.hostname),
            None
        );
    }

    #[test]
    fn announces_joins_by_name_or_after_the_wait_and_counts_them() {
        let start = Instant::now();
        let mut tracker = Tracker::default();
        assert_eq!(
            tracker.record(Station::Connected(PHONE.into()), start),
            None
        );
        assert_eq!(
            tracker.record(Station::Connected(LAPTOP.into()), start),
            None
        );

        // The phone's lease appears; the laptop never asks for one.
        assert_eq!(
            tracker.announce(PHONE_LEASE, start + Duration::from_secs(1)),
            vec![Change::Joined {
                device: "Pixel-7 (192.168.12.34)".into(),
                connected: 1
            }]
        );
        assert_eq!(
            tracker.announce(PHONE_LEASE, start + LEASE_WAIT),
            vec![Change::Joined {
                device: LAPTOP.into(),
                connected: 2
            }]
        );
        assert_eq!(
            tracker.record(Station::Disconnected(PHONE.into()), start + LEASE_WAIT),
            Some(Change::Left {
                device: "Pixel-7 (192.168.12.34)".into(),
                connected: 1
            })
        );
    }

    #[test]
    fn devices_that_leave_before_being_announced_are_not_reported() {
        let start = Instant::now();
        let mut tracker = Tracker::default();
        tracker.record(Station::Connected(PHONE.into()), start);
        assert_eq!(
            tracker.record(Station::Disconnected(PHONE.into()), start),
            None
        );
        assert!(tracker.announce(PHONE_LEASE, start + LEASE_WAIT).is_empty());
    }

    #[test]
    fn describes_changes_with_a_correctly_pluralised_count() {
        let joined = Change::Joined {
            device: "Pixel-7 (192.168.12.34)".into(),
            connected: 1,
        };
        assert_eq!(
            joined.to_string(),
            "   + Pixel-7 (192.168.12.34) joined · 1 device connected"
        );
        let left = Change::Left {
            device: LAPTOP.into(),
            connected: 0,
        };
        assert_eq!(
            left.to_string(),
            "   - d4:5e:01:02:03:04 left · 0 devices connected"
        );
    }
}
