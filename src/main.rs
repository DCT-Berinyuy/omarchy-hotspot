use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use qrcode::QrCode;
use image::Luma;

fn main() -> io::Result<()> {
    println!("Starting Omarchy Hotspot Setup Manager...\n");

    // 1. Check required dependencies
    check_dependencies();

    // 2. Check and apply patch if needed
    check_and_patch_create_ap();

    // 3. Cleanup leftover interfaces
    cleanup_virtual_interfaces();

    // 4. Detect network interfaces
    let interfaces = get_network_interfaces();
    if interfaces.is_empty() {
        eprintln!("Error: No network interfaces found!");
        return Ok(());
    }

    let default_internet = detect_default_gateway_interface().unwrap_or_else(|| "wlan0".to_string());
    let default_wifi = interfaces
        .iter()
        .find(|iface| iface.starts_with("wlan"))
        .cloned()
        .unwrap_or_else(|| "wlan0".to_string());

    println!("Detected network interfaces: {:?}", interfaces);
    println!("Suggested Internet Source: {}", default_internet);
    println!("Suggested Wi-Fi Adapter: {}", default_wifi);
    println!();

    // 5. Interactive prompts using dialoguer
    let theme = ColorfulTheme::default();
    
    let ssid: String = Input::with_theme(&theme)
        .with_prompt("Enter Hotspot SSID (Name)")
        .default("DCT_Linux".to_string())
        .interact_text()?;

    let password: String = Input::with_theme(&theme)
        .with_prompt("Enter Hotspot Password (min. 8 chars)")
        .default("Tryh4ckm3".to_string())
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
        .default(interfaces.iter().position(|x| *x == default_internet).unwrap_or(0))
        .interact()?;
    let internet_iface = &interfaces[internet_index];

    // Select wifi interface
    let wifi_index = Select::with_theme(&theme)
        .with_prompt("Select Wi-Fi interface to host hotspot")
        .items(&interfaces)
        .default(interfaces.iter().position(|x| *x == default_wifi).unwrap_or(0))
        .interact()?;
    let wifi_iface = &interfaces[wifi_index];

    println!("\nConfiguration Summary:");
    println!("   SSID:      {}", ssid);
    println!("   Password:  {}", password);
    println!("   Sharing:   {} -> {}", internet_iface, wifi_iface);
    println!();

    // 6. Setup exit signal handling
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        println!("\nReceived exit signal! Initiating shutdown...");
        r.store(false, Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    // 7. Spawn create_ap process
    println!("Starting create_ap...");
    let mut child = Command::new("sudo")
        .args(&[
            "create_ap",
            wifi_iface,
            internet_iface,
            &ssid,
            &password,
        ])
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

    // Cleanup virtual interfaces
    cleanup_virtual_interfaces();
    
    // Terminate any leftover imv windows
    let _ = Command::new("pkill")
        .arg("imv")
        .status();

    println!("Success: Hotspot stopped and cleaned up successfully.");

    // We explicitly avoid stdout_handle.join() and stderr_handle.join() 
    // to prevent deadlocks when closing the process pipes on Ctrl+C.
    
    Ok(())
}

fn get_network_interfaces() -> Vec<String> {
    let mut interfaces = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/net") {
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

fn detect_default_gateway_interface() -> Option<String> {
    if let Ok(content) = fs::read_to_string("/proc/net/route") {
        for line in content.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 2 && fields[1] == "00000000" {
                return Some(fields[0].to_string());
            }
        }
    }
    None
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
                        .args(&["sed", "-i", "/WIFI_IFACE_FREQ=/s/awk '{print $2}'/awk '{print $2}' | cut -d. -f1/", "/usr/bin/create_ap"])
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

fn save_qr_code_png(ssid: &str, password: &str) -> Option<String> {
    let wifi_str = format!("WIFI:T:WPA;S:{};P:{};;", ssid, password);
    if let Ok(code) = QrCode::new(wifi_str.as_bytes()) {
        let image = code.render::<Luma<u8>>()
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

    println!("========================================================");
    println!("          DCT OMARCHY HOTSPOT IS ACTIVE                 ");
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
    let wifi_str = format!("WIFI:T:WPA;S:{};P:{};;", ssid, password);
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
                    Ok(s) if s.success() => println!("Success: Dependencies installed successfully!"),
                    _ => {
                        eprintln!("Error: Failed to install dependencies automatically.");
                        eprintln!("   Please run: sudo pacman -S {}", missing.join(" "));
                        std::process::exit(1);
                    }
                }
            } else {
                println!("Error: Dependencies are missing. The hotspot manager cannot run without them.");
                std::process::exit(1);
            }
        }
    } else {
        println!("Success: All dependencies are installed.");
    }
}
