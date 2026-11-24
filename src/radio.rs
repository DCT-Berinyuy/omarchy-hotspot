//! Wi-Fi radio switches (rfkill).
//!
//! The hotspot broadcasts through the Wi-Fi card, so its radio has to be on
//! even when the hotspot shares a wired connection. With the radio switched
//! off, create_ap fails with "Operation not possible due to RF-kill" and
//! wrongly suggests `--no-virt`.
//!
//! NetworkManager turns Wi-Fi off by soft-blocking the radio, and keeps the
//! user's choice separately. Lifting only the kernel block gives the hotspot
//! the radio while NetworkManager keeps Wi-Fi off, so it doesn't start
//! connecting the card to a network halfway through hotspot setup.

use dialoguer::{Confirm, theme::Theme};
use std::fs;
use std::path::Path;
use std::process::{self, Command};
use std::thread;
use std::time::{Duration, Instant};

const SYS_CLASS_NET: &str = "/sys/class/net";
const SYS_CLASS_RFKILL: &str = "/sys/class/rfkill";
/// Time for the driver to report the radio unblocked.
const UNBLOCK_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, PartialEq, Eq)]
enum Radio {
    On,
    /// Switched off in software (the panel, `nmcli radio wifi off`): the IDs
    /// of the soft-blocked Wi-Fi switches.
    SwitchedOff(Vec<String>),
    /// Switched off by a hardware switch or key.
    HardBlocked,
}

/// Soft blocks lifted for the hotspot. Dropping this puts them back, so Wi-Fi
/// ends up off again, the way the hotspot found it.
pub struct Unblocked {
    switches: Vec<String>,
}

/// Makes sure `iface` can broadcast, asking before turning Wi-Fi on, and exits
/// when its radio stays off. `shares_card` is true when the hotspot also takes
/// its internet connection from `iface`.
pub fn ensure_on(iface: &str, shares_card: bool, theme: &dyn Theme) -> Option<Unblocked> {
    match state(iface) {
        Radio::On => None,
        Radio::HardBlocked => exit_hard_blocked(),
        Radio::SwitchedOff(_) if shares_card => {
            errln!("Error: Wi-Fi is turned off, so {iface} has no connection to share.");
            errln!("   Turn Wi-Fi on and connect to a network, or pick another internet source.");
            process::exit(1);
        }
        Radio::SwitchedOff(switches) => turn_on(iface, switches, theme),
    }
}

fn turn_on(iface: &str, switches: Vec<String>, theme: &dyn Theme) -> Option<Unblocked> {
    outln!("Wi-Fi is turned off. The hotspot broadcasts through {iface}, so Wi-Fi has to be on,");
    outln!("even when sharing a wired connection.");
    // No terminal to answer on (an error here) is not consent.
    let accepted = Confirm::with_theme(theme)
        .with_prompt("Turn Wi-Fi on until the hotspot stops?")
        .default(true)
        .interact()
        .unwrap_or(false);
    if !accepted {
        errln!("Error: The hotspot can't start while Wi-Fi is off. Turn it on, then try again.");
        process::exit(1);
    }

    let unblocked = Unblocked { switches };
    // Success is judged by the radio's state below, not by rfkill's exit status.
    unblocked.run_rfkill("unblock");
    let deadline = Instant::now() + UNBLOCK_TIMEOUT;
    loop {
        match state(iface) {
            Radio::On => {
                outln!("Wi-Fi is on.");
                return Some(unblocked);
            }
            Radio::HardBlocked => {
                drop(unblocked);
                exit_hard_blocked();
            }
            Radio::SwitchedOff(_) if Instant::now() >= deadline => {
                drop(unblocked);
                errln!(
                    "Error: Could not turn Wi-Fi on. Turn it on from the panel, then try again."
                );
                process::exit(1);
            }
            Radio::SwitchedOff(_) => thread::sleep(POLL_INTERVAL),
        }
    }
}

fn exit_hard_blocked() -> ! {
    errln!("Error: Wi-Fi is switched off by a hardware switch or key (such as airplane mode).");
    errln!("   Switch it on, then start omarchy-hotspot again.");
    process::exit(1);
}

impl Unblocked {
    /// Runs `rfkill <command> <id>` for every switch; true if all succeeded.
    fn run_rfkill(&self, command: &str) -> bool {
        let mut all_succeeded = true;
        for id in &self.switches {
            let status = Command::new("sudo").args(["rfkill", command, id]).status();
            all_succeeded &= matches!(status, Ok(status) if status.success());
        }
        all_succeeded
    }
}

impl Drop for Unblocked {
    fn drop(&mut self) {
        outln!("Turning Wi-Fi back off...");
        if !self.run_rfkill("block") {
            errln!("Warning: Could not turn Wi-Fi back off. Turn it off from the panel if needed.");
        }
    }
}

fn state(iface: &str) -> Radio {
    state_at(Path::new(SYS_CLASS_NET), Path::new(SYS_CLASS_RFKILL), iface)
}

/// The radio state of `iface`'s card. Besides the card's own switch, a
/// laptop's built-in Wi-Fi switch can soft-block the card too.
fn state_at(net_dir: &Path, rfkill_dir: &Path, iface: &str) -> Radio {
    let Some(card) = card_switch(net_dir, iface) else {
        // A card without an rfkill switch can't be switched off this way.
        return Radio::On;
    };
    if !card.soft && !card.hard {
        return Radio::On;
    }
    let mut soft_blocked: Vec<String> = wlan_switches(rfkill_dir)
        .into_iter()
        .filter(|switch| switch.soft)
        .map(|switch| switch.id)
        .collect();
    if card.soft && !soft_blocked.contains(&card.id) {
        soft_blocked.push(card.id);
    }
    if soft_blocked.is_empty() {
        Radio::HardBlocked
    } else {
        Radio::SwitchedOff(soft_blocked)
    }
}

struct Switch {
    id: String,
    soft: bool,
    hard: bool,
}

/// Reads an `rfkill<id>` sysfs directory.
fn read_switch(dir: &Path) -> Option<Switch> {
    let id = dir
        .file_name()?
        .to_str()?
        .strip_prefix("rfkill")
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))?;
    let blocked = |name: &str| {
        fs::read_to_string(dir.join(name))
            .ok()
            .map(|value| value.trim() == "1")
    };
    Some(Switch {
        id: id.to_string(),
        soft: blocked("soft")?,
        hard: blocked("hard")?,
    })
}

/// The rfkill switch of the card behind `iface`.
fn card_switch(net_dir: &Path, iface: &str) -> Option<Switch> {
    fs::read_dir(net_dir.join(iface).join("phy80211"))
        .ok()?
        .flatten()
        .find_map(|entry| read_switch(&entry.path()))
}

/// Every Wi-Fi (`wlan`) rfkill switch.
fn wlan_switches(rfkill_dir: &Path) -> Vec<Switch> {
    let Ok(entries) = fs::read_dir(rfkill_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            fs::read_to_string(entry.path().join("type")).is_ok_and(|kind| kind.trim() == "wlan")
        })
        .filter_map(|entry| read_switch(&entry.path()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    /// A fake sysfs laid out like the real one: wlp4s0's card on switch
    /// rfkill2, a soft-blocked Bluetooth switch rfkill0 and, optionally, the
    /// laptop's own Wi-Fi switch rfkill5. Switches are `(soft, hard)`.
    fn fake_sysfs(name: &str, card: (&str, &str), laptop: Option<(&str, &str)>) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "omarchy-hotspot-radio-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut switches = vec![
            ("phy0/rfkill2", "wlan", card),
            ("hci0/rfkill0", "bluetooth", ("1", "0")),
        ];
        if let Some(laptop) = laptop {
            switches.push(("platform/rfkill5", "wlan", laptop));
        }
        fs::create_dir_all(root.join("class/rfkill")).unwrap();
        for (path, kind, (soft, hard)) in switches {
            let dir = root.join("devices").join(path);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("type"), format!("{kind}\n")).unwrap();
            fs::write(dir.join("soft"), format!("{soft}\n")).unwrap();
            fs::write(dir.join("hard"), format!("{hard}\n")).unwrap();
            symlink(
                &dir,
                root.join("class/rfkill").join(dir.file_name().unwrap()),
            )
            .unwrap();
        }
        fs::create_dir_all(root.join("class/net/wlp4s0")).unwrap();
        symlink(
            root.join("devices/phy0"),
            root.join("class/net/wlp4s0/phy80211"),
        )
        .unwrap();
        root
    }

    fn state_of(root: &Path) -> Radio {
        state_at(
            &root.join("class/net"),
            &root.join("class/rfkill"),
            "wlp4s0",
        )
    }

    #[test]
    fn switched_off_radio_lists_only_wifi_switches() {
        let root = fake_sysfs("soft", ("1", "0"), None);
        assert_eq!(state_of(&root), Radio::SwitchedOff(vec!["2".to_string()]));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hardware_switch_cannot_be_lifted() {
        let root = fake_sysfs("hard", ("0", "1"), None);
        assert_eq!(state_of(&root), Radio::HardBlocked);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn laptop_wifi_switch_blocking_the_card_can_be_lifted() {
        let root = fake_sysfs("laptop", ("0", "1"), Some(("1", "0")));
        assert_eq!(state_of(&root), Radio::SwitchedOff(vec!["5".to_string()]));
        fs::remove_dir_all(root).unwrap();
    }
}
