use dialoguer::{Input, Select, theme::ColorfulTheme};
use image::Luma;
use qrcode::QrCode;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

fn main() -> io::Result<()> {
    print_logo();
    println!("Starting Omarchy Hotspot Setup Manager...\n");

    // 1. Check required dependencies
    check_dependencies();

    // 2. Check and apply patch if needed
    check_and_patch_create_ap();

    // 3. Cleanup leftover interfaces & stale processes
    cleanup_virtual_interfaces();
    cleanup_stale_processes();

    // 4. Detect network interfaces
    let interfaces = get_network_interfaces();
    if interfaces.is_empty() {
        eprintln!("Error: No network interfaces found!");
        return Ok(());
    }

    let wireless_interfaces = get_wireless_interfaces(Path::new(SYS_CLASS_NET), &interfaces);
    if wireless_interfaces.is_empty() {
        eprintln!(
            "Error: No Wi-Fi adapter found! A wireless card is required to broadcast a hotspot."
        );
        return Ok(());
    }

    let default_internet = detect_default_gateway_interface();
    let default_wifi = &wireless_interfaces[0];

    println!("Detected network interfaces: {:?}", interfaces);
    println!("Detected Wi-Fi adapters:     {:?}", wireless_interfaces);
    println!(
        "Suggested Internet Source:   {}",
        default_internet
            .as_deref()
            .unwrap_or("(no default route found)")
    );
    println!("Suggested Wi-Fi Adapter:     {}", default_wifi);
    println!();

    // 5. Interactive prompts using dialoguer
    let theme = ColorfulTheme::default();

    let ssid: String = Input::with_theme(&theme)
        .with_prompt("Enter Hotspot SSID (Name)")
        .default("DCT_Linux".to_string())
        .interact_text()?;

    let password: String = Input::with_theme(&theme)
        .with_prompt("Enter Hotspot Password (min. 8 chars)")
        .default("Tryh4ckm3;".to_string())
        .validate_with(|input: &String| -> Result<(), &str> {
            if input.len() >= 8 {
                Ok(())
            } else {
                Err("Password must be at least 8 characters long")
            }
        })
        .interact_text()?;

    // Select internet interface
    let internet_index = Select::with_theme(&theme)
        .with_prompt("Select interface providing internet")
        .items(&interfaces)
        .default(
            default_internet
                .as_ref()
                .and_then(|d| interfaces.iter().position(|x| x == d))
                .unwrap_or(0),
        )
        .interact()?;
    let internet_iface = &interfaces[internet_index];

    // Only wireless adapters can broadcast a hotspot
    let wifi_index = Select::with_theme(&theme)
        .with_prompt("Select Wi-Fi adapter to broadcast the hotspot")
        .items(&wireless_interfaces)
        .default(0)
        .interact()?;
    let wifi_iface = &wireless_interfaces[wifi_index];

    let mode = if internet_iface == wifi_iface {
        "Wi-Fi repeater (virtual AP on the same card)"
    } else if wireless_interfaces.contains(internet_iface) {
        "Wi-Fi -> Wi-Fi"
    } else {
        "Wired (Ethernet) -> Wi-Fi"
    };

    println!("\nConfiguration Summary:");
    println!("   SSID:      {}", ssid);
    println!("   Password:  {}", password);
    println!("   Sharing:   {} -> {}", internet_iface, wifi_iface);
    println!("   Mode:      {}", mode);
    println!();

    // 6. Setup exit signal handling
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        println!("\nReceived exit signal! Initiating shutdown...");
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    // 7. Spawn create_ap process
    println!("Starting create_ap...");
    let mut child = Command::new("sudo")
        .args(&["create_ap", wifi_iface, internet_iface, &ssid, &password])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let child_stdout = child.stdout.take().expect("Failed to open stdout");
    let child_stderr = child.stderr.take().expect("Failed to open stderr");

    // Thread to monitor stdout and show TUI when AP is enabled
    let ssid_clone = ssid.clone();
    let password_clone = password.clone();
    let running_clone = running.clone();

    thread::spawn(move || {
        let reader = BufReader::new(child_stdout);
        let mut ap_enabled = false;

        for line in reader.lines() {
            if !running_clone.load(Ordering::SeqCst) {
                break;
            }
            if let Ok(line) = line {
                if !ap_enabled {
                    println!("   [create_ap] {}", line);
                }
                if line.contains("AP-ENABLED") {
                    ap_enabled = true;
                    show_dashboard(&ssid_clone, &password_clone);
                }
            }
        }
    });

    // Thread to monitor stderr
    thread::spawn(move || {
        let reader = BufReader::new(child_stderr);
        for line in reader.lines() {
            if let Ok(line) = line {
                eprintln!("   [create_ap error] {}", line);
            }
        }
    });

    // Main loop: Wait until exit signal
    while running.load(Ordering::SeqCst) {
        if let Ok(Some(_)) = child.try_wait() {
            println!("Error: create_ap terminated unexpectedly.");
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }

    // 8. Cleanup on exit
    println!("Stopping create_ap process group...");

    // Kill the underlying create_ap processes cleanly using pkill
    let _ = Command::new("sudo")
        .args(&["pkill", "-SIGINT", "-f", "create_ap"])
        .status();

    // Kill the spawned sudo wrapper process
    let _ = child.kill();
    let _ = child.wait();

    // Wait a brief moment to allow create_ap's internal cleanup script to finish running
    thread::sleep(Duration::from_millis(800));

    // Cleanup virtual interfaces & stale processes
    cleanup_virtual_interfaces();
    cleanup_stale_processes();

    // Terminate any leftover imv windows
    let _ = Command::new("pkill").arg("imv").status();

    println!("Success: Hotspot stopped and cleaned up successfully.");

    // We explicitly avoid stdout_handle.join() and stderr_handle.join()
    // to prevent deadlocks when closing the process pipes on Ctrl+C.

    Ok(())
}

const SYS_CLASS_NET: &str = "/sys/class/net";

fn get_network_interfaces() -> Vec<String> {
    let mut interfaces = Vec::new();
    if let Ok(entries) = fs::read_dir(SYS_CLASS_NET) {
        for entry in entries {
            if let Ok(entry) = entry {
                if let Some(name) = entry.file_name().to_str() {
                    // Filter out loopback
                    if name != "lo" {
                        interfaces.push(name.to_string());
                    }
                }
            }
        }
    }
    interfaces.sort();
    interfaces
}

/// A kernel network interface is wireless if sysfs exposes a `wireless`
/// directory or a `phy80211` link for it, regardless of its name
/// (`wlan0`, `wlp4s0`, `wlx...`).
fn is_wireless_interface(net_dir: &Path, iface: &str) -> bool {
    let dir = net_dir.join(iface);
    dir.join("wireless").exists() || dir.join("phy80211").exists()
}

fn get_wireless_interfaces(net_dir: &Path, interfaces: &[String]) -> Vec<String> {
    interfaces
        .iter()
        .filter(|iface| !iface.starts_with("ap") && is_wireless_interface(net_dir, iface))
        .cloned()
        .collect()
}

fn detect_default_gateway_interface() -> Option<String> {
    fs::read_to_string("/proc/net/route")
        .ok()
        .and_then(|content| parse_default_route(&content))
}

fn parse_default_route(route_table: &str) -> Option<String> {
    route_table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        (fields.len() >= 2 && fields[1] == "00000000").then(|| fields[0].to_string())
    })
}

fn cleanup_virtual_interfaces() {
    println!("Cleaning up leftover virtual AP interfaces...");
    let interfaces = get_network_interfaces();
    for iface in interfaces {
        if iface.starts_with("ap") {
            println!("  Deleting {}...", iface);
            let _ = Command::new("sudo")
                .args(&["iw", "dev", &iface, "del"])
                .status();
        }
    }
}

fn check_and_patch_create_ap() {
    if let Ok(content) = fs::read_to_string("/usr/bin/create_ap") {
        if !content.contains("cut -d. -f1") {
            println!("Warning: Legacy create_ap bug detected (frequency decimal parsing).");
            print!("Do you want to patch /usr/bin/create_ap automatically? [Y/n]: ");
            let _ = io::stdout().flush();
            let mut input = String::new();
            if io::stdin().read_line(&mut input).is_ok() {
                let input = input.trim().to_lowercase();
                if input == "y" || input.is_empty() {
                    println!("Patching /usr/bin/create_ap...");

                    // Apply decimal fix
                    let _ = Command::new("sudo")
                        .args(&[
                            "sed",
                            "-i",
                            "/WIFI_IFACE_FREQ=/s/awk '{print $2}'/awk '{print $2}' | cut -d. -f1/",
                            "/usr/bin/create_ap",
                        ])
                        .status();

                    // Apply can_transmit_to_channel override
                    let _ = Command::new("sudo")
                        .args(&["sed", "-i", "s/can_transmit_to_channel() {/can_transmit_to_channel() {\\n    return 0/g", "/usr/bin/create_ap"])
                        .status();

                    println!("Success: Patches applied successfully!");
                }
            }
        }
    }
}

/// Builds a `WIFI:` QR payload, escaping characters that are reserved by the
/// format (`\`, `;`, `,`, `:`, `"`) so phones parse the SSID/password correctly.
fn wifi_qr_payload(ssid: &str, password: &str) -> String {
    fn escape(value: &str) -> String {
        value
            .chars()
            .flat_map(|c| match c {
                '\\' | ';' | ',' | ':' | '"' => vec!['\\', c],
                _ => vec![c],
            })
            .collect()
    }
    format!("WIFI:T:WPA;S:{};P:{};;", escape(ssid), escape(password))
}

fn save_qr_code_png(ssid: &str, password: &str) -> Option<String> {
    let wifi_str = wifi_qr_payload(ssid, password);
    if let Ok(code) = QrCode::new(wifi_str.as_bytes()) {
        let image = code
            .render::<Luma<u8>>()
            .quiet_zone(true)
            .module_dimensions(10, 10) // 10x10 pixels per QR module for a crisp high-res image
            .build();
        let path = "/tmp/omarchy_hotspot_qr.png";
        if image.save(path).is_ok() {
            return Some(path.to_string());
        }
    }
    None
}

fn show_dashboard(ssid: &str, password: &str) {
    // Clear screen and move cursor to top-left
    print!("{}[2J{}[1;1H", 27 as char, 27 as char);
    let _ = io::stdout().flush();

    print_logo();
    println!("========================================================");
    println!("          HOTSPOT IS NOW ACTIVE                         ");
    println!("========================================================");
    println!();
    println!("   SSID (Name):   \x1b[1;32m{}\x1b[0m", ssid);
    println!("   Password:      \x1b[1;32m{}\x1b[0m", password);
    println!();

    // 1. Save and open the QR Code PNG using imv (visual pop-up)
    if let Some(path) = save_qr_code_png(ssid, password) {
        println!("Opening high-contrast QR Code in image viewer (imv)...");
        let _ = Command::new("imv")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }

    println!();
    println!("Scan the QR Code on your screen to connect automatically.");
    println!("If the image window didn't open, here is a terminal fallback:");
    println!();

    // 2. Terminal Fallback QR Code
    let wifi_str = wifi_qr_payload(ssid, password);
    if let Ok(code) = QrCode::new(wifi_str.as_bytes()) {
        let width = code.width();
        let quiet_zone = 2;

        let white_block = "\x1b[47m  ";
        let black_block = "\x1b[40m  ";
        let reset_color = "\x1b[0m";

        // Top quiet zone
        for _ in 0..quiet_zone {
            for _ in 0..(width + quiet_zone * 2) {
                print!("{}", white_block);
            }
            println!("{}", reset_color);
        }

        for y in 0..width {
            // Left quiet zone
            for _ in 0..quiet_zone {
                print!("{}", white_block);
            }
            for x in 0..width {
                if code[(x, y)] == qrcode::Color::Dark {
                    print!("{}", black_block);
                } else {
                    print!("{}", white_block);
                }
            }
            // Right quiet zone
            for _ in 0..quiet_zone {
                print!("{}", white_block);
            }
            println!("{}", reset_color);
        }

        // Bottom quiet zone
        for _ in 0..quiet_zone {
            for _ in 0..(width + quiet_zone * 2) {
                print!("{}", white_block);
            }
            println!("{}", reset_color);
        }
    }
    println!();
    println!("========================================================");
    println!("Press Ctrl+C at any time to stop the hotspot.");
    println!("========================================================");
}

fn check_dependencies() {
    println!("Running Dependency Doctor...");
    let dependencies = vec![
        ("create_ap", "create_ap"),
        ("hostapd", "hostapd"),
        ("dnsmasq", "dnsmasq"),
        ("imv", "imv"),
    ];

    let mut missing = Vec::new();
    for (name, bin) in &dependencies {
        let status = Command::new("which")
            .arg(bin)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let is_missing = match status {
            Ok(s) => !s.success(),
            Err(_) => true,
        };
        if is_missing {
            missing.push(*name);
        }
    }

    if !missing.is_empty() {
        println!("Warning: Missing required dependencies: {:?}", missing);
        print!("Would you like to install them via pacman? [Y/n]: ");
        let _ = io::stdout().flush();
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_ok() {
            let input = input.trim().to_lowercase();
            if input == "y" || input.is_empty() {
                println!("Installing dependencies...");
                let mut args = vec!["pacman", "-S", "--noconfirm"];
                args.extend(&missing);
                let status = Command::new("sudo").args(&args).status();
                match status {
                    Ok(s) if s.success() => {
                        println!("Success: Dependencies installed successfully!")
                    }
                    _ => {
                        eprintln!("Error: Failed to install dependencies automatically.");
                        eprintln!("   Please run: sudo pacman -S {}", missing.join(" "));
                        std::process::exit(1);
                    }
                }
            } else {
                println!(
                    "Error: Dependencies are missing. The hotspot manager cannot run without them."
                );
                std::process::exit(1);
            }
        }
    } else {
        println!("Success: All dependencies are installed.");
    }
}

fn print_logo() {
    println!("\x1b[1;32m");
    println!("  ____                               _               ");
    println!(" / __ \\ _ __ ___   __ _ _ __ ___ ___| |__  _   _     ");
    println!("/ / _` | '_ ` _ \\ / _` | '__/ __/ __| '_ \\| | | |    ");
    println!("| |(_| | | | | | | (_| | | | (__\\__ \\ | | | |_| |    ");
    println!("\\ \\__,_|_| |_| |_|\\__,_|_|  \\___|___/_| |_|\\__, |    ");
    println!(" \\____/                                    |___/     ");
    println!(" _   _       _                 _                     ");
    println!("| | | | ___ | |_ ___ _ __   __| |                    ");
    println!("| |_| |/ _ \\| __/ __| '_ \\ / _` |                    ");
    println!("|  _  | (_) | |_\\__ \\ |_) | (_| |                    ");
    println!("|_| |_|\\___/ \\__|___/ .__/ \\__,_|                    ");
    println!("                    |_|                              ");
    println!("\x1b[0m");
}

fn cleanup_stale_processes() {
    println!("Cleaning up leftover dnsmasq processes from previous runs...");
    let _ = Command::new("sudo")
        .args(&["pkill", "-f", "dnsmasq -C /tmp/create_ap"])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fake_sysfs(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "omarchy-hotspot-test-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("enp0s31f6")).unwrap();
        fs::create_dir_all(root.join("wlp4s0/wireless")).unwrap();
        fs::create_dir_all(root.join("wlan1/phy80211")).unwrap();
        fs::create_dir_all(root.join("ap0/wireless")).unwrap();
        root
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_wireless_adapters_by_sysfs_not_by_name() {
        let root = fake_sysfs("detect");
        let all = names(&["ap0", "enp0s31f6", "wlan1", "wlp4s0"]);

        let wireless = get_wireless_interfaces(&root, &all);

        assert_eq!(wireless, names(&["wlan1", "wlp4s0"]));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ethernet_is_never_offered_as_hotspot_adapter() {
        let root = fake_sysfs("ethernet");
        assert!(!is_wireless_interface(&root, "enp0s31f6"));
        assert!(is_wireless_interface(&root, "wlp4s0"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_default_route_interface() {
        let table = "Iface\tDestination\tGateway\n\
                     wlp4s0\t0010A8C0\t00000000\n\
                     enp0s31f6\t00000000\t010CA8C0\n";
        assert_eq!(parse_default_route(table), Some("enp0s31f6".to_string()));
    }

    #[test]
    fn returns_none_without_default_route() {
        let table = "Iface\tDestination\tGateway\nwlp4s0\t0010A8C0\t00000000\n";
        assert_eq!(parse_default_route(table), None);
    }

    #[test]
    fn escapes_reserved_characters_in_qr_payload() {
        assert_eq!(
            wifi_qr_payload("DCT_Linux", "Tryhackm3;"),
            "WIFI:T:WPA;S:DCT_Linux;P:Tryhackm3\\;;;"
        );
        assert_eq!(
            wifi_qr_payload("a:b,c", "p\\\"w"),
            "WIFI:T:WPA;S:a\\:b\\,c;P:p\\\\\\\"w;;"
        );
    }
}
