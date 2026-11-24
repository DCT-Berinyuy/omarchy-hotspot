<p align="center">
  <img src="assets/Graphical Logo.png" width="200" alt="omarchy-hotspot Logo">
</p>

# omarchy-hotspot

[![CI](https://github.com/DCT-Berinyuy/omarchy-hotspot/actions/workflows/ci.yml/badge.svg)](https://github.com/DCT-Berinyuy/omarchy-hotspot/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/DCT-Berinyuy/omarchy-hotspot)](https://github.com/DCT-Berinyuy/omarchy-hotspot/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

An interactive, terminal-based Wi-Fi hotspot manager built in Rust, tailored for Arch Linux configurations (like Omarchy) running minimalist Wayland compositors (like Hyprland).

`omarchy-hotspot` wraps `create_ap` and other system commands to automate virtual interface conflicts, dependency validations, and configuration wizards, providing a clean console dashboard with a scan-to-connect QR code and a live list of connected devices.

<p align="center">
  <img src="assets/dashboard.png" width="420" alt="omarchy-hotspot running in a terminal: the Omarchy Hotspot banner, the network name and password, and a QR code to scan">
</p>

Devices then appear below the dashboard as they join and leave:

```text
   + Pixel-7 (192.168.12.34) joined · 1 device connected
   - Pixel-7 (192.168.12.34) left · 0 devices connected
```

---

## Features

*   **Dependency Doctor:** Validates system requirements (`create_ap`, `hostapd`, `dnsmasq`, `iptables`, `imv`) at startup. It offers to install missing official packages with `pacman`, and prints the `yay` command when `create_ap` itself is missing.
*   **create_ap Check:** Warns if the original, unmaintained `create_ap` is installed and shows how to switch to the maintained fork. `omarchy-hotspot` never modifies `create_ap`.
*   **Auto-Cleanup:** Detects and deletes orphaned virtual interfaces (like `ap0`, `ap1`, `ap2`) left by failed or interrupted hotspot sessions, avoiding `Device or resource busy` errors. Interfaces and processes belonging to other running `create_ap` sessions are left alone.
*   **Interactive Setup Wizard:** Guided terminal UI prompts to set SSID, WPA2 password (a fresh random one is suggested each run), and interface routing (e.g. sharing Wi-Fi connection over a virtual AP on the same card). The password is passed to `create_ap` over stdin, never on the command line, and the QR image is kept in a private temporary directory that is deleted on exit.
*   **QR Code Dashboard:** Opens a high-contrast QR code in `imv`, with an ANSI QR code in the terminal as a fallback. Scan it with a phone camera to connect without typing the password.
*   **Connected Devices:** While the hotspot runs, prints a line whenever a device joins or leaves, with its name and IP address and how many devices are connected.
*   **Wi-Fi Radio Check:** The hotspot broadcasts through your Wi-Fi card, so Wi-Fi has to be on even when you share a wired connection. If it's turned off, `omarchy-hotspot` offers to turn it on, and turns it back off when the hotspot stops.
*   **Address Check:** Hotspot devices get addresses in `192.168.12.x`, unless a network you're on already uses that range. Then it picks a free one, such as `192.168.13.x`, so the hotspot never takes another network's addresses, like its router's.
*   **Safe Signal Cleanup:** Cleanly stops the hotspot it started on `Ctrl+C`, when the terminal window is closed, or on `SIGTERM`, waiting for `create_ap`'s own cleanup and deleting virtual interfaces from your wireless card.

---

## 🛠 Prerequisites

*   Arch Linux or derivatives.
*   `sudo` permissions.
*   Wireless card supporting AP mode concurrent with Station mode (virtual APs).
*   `create_ap` from [linux-wifi-hotspot](https://github.com/lakinduakash/linux-wifi-hotspot), the maintained fork, plus `dnsmasq`, `iptables` and `imv`. Install them as your normal user, not with `sudo` (AUR helpers refuse to run as root):

    ```bash
    yay -S --needed linux-wifi-hotspot dnsmasq iptables imv
    ```

    The package also installs a GTK app, `wihotspot`; `omarchy-hotspot` only uses its `create_ap`. The original `create_ap` (AUR `create_ap` or `create_ap-git`) is no longer maintained and fails on current Arch; installing `linux-wifi-hotspot` replaces it.

---

## 💻 Usage

### Quick Install (Precompiled Binary)
If you do not have Rust or Cargo installed, you can download the precompiled binary (under 1 MB) directly from the [GitHub Releases](https://github.com/DCT-Berinyuy/omarchy-hotspot/releases) page:

```bash
# Download the latest binary and its checksum
curl -fLO https://github.com/DCT-Berinyuy/omarchy-hotspot/releases/latest/download/omarchy-hotspot
curl -fLO https://github.com/DCT-Berinyuy/omarchy-hotspot/releases/latest/download/omarchy-hotspot.sha256

# Check the download is intact (prints "omarchy-hotspot: OK")
sha256sum -c omarchy-hotspot.sha256

# Optional: confirm this repository's GitHub Actions built it, since it runs as root
# (releases v0.1.1 and later; needs the GitHub CLI)
gh attestation verify omarchy-hotspot --repo DCT-Berinyuy/omarchy-hotspot

# Install it globally as a root-owned program, then delete the downloads
sudo install -m 755 omarchy-hotspot /usr/local/bin/omarchy-hotspot
rm omarchy-hotspot omarchy-hotspot.sha256

# Start the hotspot manager from anywhere
sudo omarchy-hotspot
```

To update, run the same commands again.

### Build from Source
If you prefer to build the program from source (requires Cargo/Rust):

```bash
# Clone the repository
git clone https://github.com/DCT-Berinyuy/omarchy-hotspot.git
cd omarchy-hotspot

# Build and run
cargo run
```

### Installation
To compile a release binary and install it globally:

```bash
cargo build --release
sudo install -m 755 target/release/omarchy-hotspot /usr/local/bin/omarchy-hotspot
```

Then you can launch the hotspot manager from anywhere by simply running:
```bash
sudo omarchy-hotspot
```

To update, run `git pull`, then both commands again.

`sudo install` writes a new, root-owned copy. Don't use `sudo mv` or `sudo cp`: `mv` keeps your user as the owner, and `cp` keeps an existing copy's owner, which lets anything running as your user replace a program you run as root. Rerunning the install command fixes a copy installed that way.

### Uninstall
```bash
sudo rm /usr/local/bin/omarchy-hotspot
```

---

## 🔍 Troubleshooting

### Sharing a wired (Ethernet) connection
Plug in your Ethernet cable, run `sudo omarchy-hotspot`, and select:

*   **Interface providing internet:** your Ethernet interface (e.g. `enp0s31f6`). It is pre-selected when it holds the default route.
*   **Wi-Fi adapter to broadcast the hotspot:** your wireless card (e.g. `wlp4s0`). Only real wireless adapters are listed here, detected via `/sys/class/net/<iface>/wireless`, so predictable names like `wlp4s0` work.

Your Wi-Fi card does not need to be connected to any network; it only acts as the transmitter. Its radio does need to be on: if Wi-Fi is turned off, `omarchy-hotspot` offers to turn it on until the hotspot stops.

### `Operation not possible due to RF-kill`
Wi-Fi is turned off, from the panel or by an airplane-mode key, so the card can't broadcast. `create_ap` also suggests `--no-virt`, which doesn't help here. From v0.3.1, `omarchy-hotspot` offers to turn Wi-Fi on itself. With older versions, turn Wi-Fi on and start again.

### Card limit exhausted
If you encounter `RTNETLINK answers: Device or resource busy`, the program will attempt to auto-clean old virtual interfaces. If it persists, you can manually run:
```bash
for dev in ap0 ap1 ap2 ap3; do sudo iw dev $dev del 2>/dev/null || true; done
```

### `can not transmit to channel` or `2412.0: arithmetic syntax error`
These errors come from the original `create_ap`, which can't parse the frequencies current `iw` prints (`2412.0 MHz`). Replace it with the maintained fork, as your normal user:
```bash
yay -S linux-wifi-hotspot
```

---

## 📄 License
This project is open-source and available under the [MIT License](LICENSE).
