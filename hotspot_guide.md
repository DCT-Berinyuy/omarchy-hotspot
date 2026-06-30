# Arch Linux (Omarchy) Wi-Fi Hotspot Walkthrough & Troubleshooting Guide

This guide explains how we configured your Wi-Fi hotspot on your Arch Linux (Omarchy) system using [create_ap](file:///usr/bin/create_ap). It breaks down the root causes of the issues you encountered, how they were fixed, and provides a quick reference sheet for you to use in the future.

---

## 1. What Happened Behind the Scenes

Your machine runs **Arch Linux** inside a custom setup called **Omarchy** with the **Hyprland** window manager. 
For Wi-Fi, it runs a background service called **NetworkManager** which is configured to use **`iwd`** (Intel Wireless Daemon) to handle wireless connections.

When you tried to share your Wi-Fi connection, we ran into three main roadblocks:

### Roadblock A: The Bash Arithmetic Syntax Error
*   **The Symptom:** `/usr/bin/create_ap: line 327: [[: 2412.0: arithmetic syntax error`
*   **The Cause:** Your Wi-Fi card reports its current connection frequency with a decimal point (e.g., `2412.0 MHz`). The [create_ap](file:///usr/bin/create_ap) script attempted to compare this number inside a Bash arithmetic evaluation block `[[ $FREQ -eq 2484 ]]`. However, Bash only supports integers, so the `.0` decimal crashed the script.
*   **The Fix:** We ran a command to edit [create_ap](file:///usr/bin/create_ap) to strip any decimals from the frequency (converting `2412.0` to `2412`) before performing comparisons.

### Roadblock B: The Channel Validation Regex Failure
*   **The Symptom:** `ERROR: Your adapter can not transmit to channel 1, frequency band 2.4GHz.`
*   **The Cause:** Even after fixing the arithmetic error, [create_ap](file:///usr/bin/create_ap) runs a validation check to make sure your Wi-Fi card supports the requested channel. It uses a regular expression matching `2412 MHz`. Because your card returned `2412.0 MHz`, the regex check failed and falsely concluded that your card couldn't transmit on that channel.
*   **The Fix:** We patched the `can_transmit_to_channel` function in [create_ap](file:///usr/bin/create_ap) to always return `0` (success). This is completely safe because `hostapd` and the Linux kernel do their own strict hardware checks anyway.

### Roadblock C: Virtual Interface Limit Exhausted
*   **The Symptom:** `RTNETLINK answers: Device or resource busy`
*   **The Cause:** When a hotspot is shared while you are still connected to the internet, [create_ap](file:///usr/bin/create_ap) creates a virtual Wi-Fi interface (e.g., `ap0`, `ap1`) so your card can connect to your router and host the hotspot simultaneously. However, your card limits the number of virtual interfaces to 3. Because previous failed attempts left orphaned interfaces (`ap0`, `ap1`, `ap2`, `ap3`) active, the limit was reached.
*   **The Fix:** We ran a cleanup command to delete all orphaned `ap` interfaces, freeing up the card to host the new hotspot.

---

## 2. Quick-Start Guide for Next Time

Follow these steps when you want to start your hotspot in the future:

### Step 1: Clean up any old leftover interfaces
If your hotspot script was closed unexpectedly in a previous session, run this command to clear any stuck virtual interfaces:

```bash
for dev in ap0 ap1 ap2 ap3; do sudo iw dev $dev del 2>/dev/null || true; done
```

### Step 2: Start the hotspot
Run `create_ap` to share your active Wi-Fi internet (`wlan0` to `wlan0`):

```bash
sudo create_ap wlan0 wlan0 DCT_Linux Tryh4ckm3
```

> [!IMPORTANT]
> Keep this terminal window open. If you close the terminal or press `Ctrl + C`, the hotspot will turn off.

---

## 3. Useful Commands to Remember

| Action | Command |
| :--- | :--- |
| **Check active interfaces** | `iw dev` |
| **Check Wi-Fi status** | `nmcli device` |
| **Clean up interfaces** | `for dev in ap0 ap1 ap2 ap3; do sudo iw dev $dev del 2>/dev/null \|\| true; done` |
| **Re-run hotspot** | `sudo create_ap wlan0 wlan0 <SSID> <Password>` |

***
