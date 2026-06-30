# omarchy-hotspot

An interactive, terminal-based Wi-Fi hotspot manager built in Rust, tailored for Arch Linux configurations (like Omarchy) running minimalist Wayland compositors (like Hyprland).

`omarchy-hotspot` wraps `create_ap` and other system commands to automate virtual interface conflicts, dependency validations, legacy script bugs, and configuration wizards, providing a clean console dashboard with a scan-to-connect QR code.

---

## 🚀 Features

*   **Dependency Doctor:** Validates system requirements (`create_ap`, `hostapd`, `dnsmasq`) at startup and offers to auto-install missing packages using `pacman`.
*   **Auto-Cleanup Daemon:** Detects and deletes orphaned virtual interfaces (like `ap0`, `ap1`, `ap2`) left by failed or interrupted hotspot sessions, avoiding `Device or resource busy` errors.
*   **Legacy Bug Patcher:** Auto-detects legacy decimal-point parsing bugs in `/usr/bin/create_ap` and applies system patches automatically.
*   **Interactive Setup Wizard:** Guided terminal UI prompts to set SSID, WPA2 password, and interface routing (e.g. sharing Wi-Fi connection over a virtual AP on the same card).
*   **ANSI QR Code Dashboard:** Generates a compact QR code in the terminal. Scan it with a mobile camera to connect instantly without typing passwords.
*   **Safe Signal Cleanup:** Handles termination signals (`Ctrl+C`) to cleanly stop the hotspot and delete virtual interfaces from your wireless card.

---

## 🛠 Prerequisites

*   Arch Linux or derivatives.
*   `sudo` permissions.
*   Wireless card supporting AP mode concurrent with Station mode (virtual APs).

---

## 💻 Usage

### Quick Start
To build and run the hotspot manager:

```bash
# Clone the repository (if not already local)
git clone https://github.com/DCT-Berinyuy/omarchy-hotspot.git
cd omarchy-hotspot

# Build and run
cargo run
```

### Installation
To compile a release binary and install it globally:

```bash
cargo build --release
sudo cp target/release/omarchy-hotspot /usr/local/bin/
```

Then you can launch the hotspot manager from anywhere by simply running:
```bash
sudo omarchy-hotspot
```

---

## 🔍 Troubleshooting

### Card limit exhausted
If you encounter `RTNETLINK answers: Device or resource busy`, the program will attempt to auto-clean old virtual interfaces. If it persists, you can manually run:
```bash
for dev in ap0 ap1 ap2 ap3; do sudo iw dev $dev del 2>/dev/null || true; done
```

---

## 📄 License
This project is open-source and available under the MIT License.
