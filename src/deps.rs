//! Dependency Doctor: checks the tools omarchy-hotspot drives and which
//! `create_ap` is installed.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

/// AUR package with the maintained fork of create_ap.
const CREATE_AP_PACKAGE: &str = "linux-wifi-hotspot";

struct Dependency {
    binary: &'static str,
    package: &'static str,
    /// AUR packages can't be installed from here: AUR helpers refuse to run as root.
    from_aur: bool,
}

const DEPENDENCIES: &[Dependency] = &[
    Dependency {
        binary: "create_ap",
        package: CREATE_AP_PACKAGE,
        from_aur: true,
    },
    Dependency {
        binary: "hostapd",
        package: "hostapd",
        from_aur: false,
    },
    // Optional for linux-wifi-hotspot, but required to share an internet connection.
    Dependency {
        binary: "dnsmasq",
        package: "dnsmasq",
        from_aur: false,
    },
    Dependency {
        binary: "iptables",
        package: "iptables",
        from_aur: false,
    },
    Dependency {
        binary: "imv",
        package: "imv",
        from_aur: false,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateApKind {
    /// The maintained fork from linux-wifi-hotspot.
    Maintained,
    /// The archived original (oblique/create_ap), possibly patched in place
    /// by omarchy-hotspot 0.1.x.
    Legacy {
        patched: bool,
    },
    Unknown,
}

/// Checks dependencies and offers to install missing repo packages. Exits the
/// process when a requirement cannot be met.
pub fn run_doctor() -> CreateApKind {
    outln!("Running Dependency Doctor...");
    let missing: Vec<&Dependency> = DEPENDENCIES
        .iter()
        .filter(|dep| find_in_path(dep.binary).is_none())
        .collect();
    if missing.is_empty() {
        outln!("Success: All dependencies are installed.");
    } else {
        install_missing(&missing);
    }

    let kind = find_in_path("create_ap")
        .and_then(|path| fs::read(path).ok())
        .map_or(CreateApKind::Unknown, |script| {
            classify_create_ap(&String::from_utf8_lossy(&script))
        });
    if let CreateApKind::Legacy { patched } = kind {
        outln!(
            "Warning: The installed create_ap is the original version, which is no longer maintained."
        );
        if patched {
            outln!(
                "   An older omarchy-hotspot patched it in place, which turned off its channel check."
            );
        } else {
            outln!(
                "   It fails on current Arch, where iw reports frequencies like \"2412.0 MHz\"."
            );
        }
        print_switch_hint();
    }
    kind
}

pub fn print_switch_hint() {
    outln!("   Switch to the maintained fork, which replaces it (run as your normal user):");
    outln!("   yay -S {CREATE_AP_PACKAGE}");
}

fn install_missing(missing: &[&Dependency]) {
    let names: Vec<&str> = missing.iter().map(|dep| dep.binary).collect();
    outln!(
        "Warning: Missing required dependencies: {}",
        names.join(", ")
    );
    let packages: Vec<&str> = missing.iter().map(|dep| dep.package).collect();

    let aur: Vec<&str> = missing
        .iter()
        .filter(|dep| dep.from_aur)
        .map(|dep| dep.binary)
        .collect();
    if !aur.is_empty() {
        // yay installs official packages too, so one command covers everything.
        errln!(
            "Error: {} must be installed from the AUR, and AUR helpers refuse to run as root.",
            aur.join(", ")
        );
        errln!("   Run this as your normal user (without sudo), then start omarchy-hotspot again:");
        errln!("   yay -S --needed {}", packages.join(" "));
        process::exit(1);
    }
    install_repo_packages(&packages);
}

fn install_repo_packages(packages: &[&str]) {
    out!("Install {} with pacman? [Y/n]: ", packages.join(" "));
    let _ = io::stdout().flush();
    let mut input = String::new();
    // End of input (stdin closed or not interactive) is not consent; a bare Enter is.
    let accepted = matches!(io::stdin().read_line(&mut input), Ok(read) if read > 0)
        && matches!(input.trim().to_lowercase().as_str(), "" | "y" | "yes");
    if !accepted {
        errln!("Error: Dependencies are missing. The hotspot manager cannot run without them.");
        process::exit(1);
    }

    outln!("Installing dependencies...");
    let status = Command::new("sudo")
        .args(["pacman", "-S", "--needed", "--noconfirm"])
        .args(packages)
        .status();
    if matches!(status, Ok(status) if status.success()) {
        outln!("Success: Dependencies installed successfully!");
    } else {
        errln!("Error: Failed to install dependencies automatically.");
        errln!(
            "   Please run: sudo pacman -S --needed {}",
            packages.join(" ")
        );
        process::exit(1);
    }
}

fn classify_create_ap(script: &str) -> CreateApKind {
    // The fork strips the fractional frequencies ("2412.0") that newer `iw` prints.
    if script.contains("FREQ_MAYBE_FRACTIONAL")
        || script.contains("lakinduakash/linux-wifi-hotspot")
    {
        CreateApKind::Maintained
    } else if script.lines().any(|line| line.trim() == "VERSION=0.4.6") {
        // `cut -d. -f1` is what the old omarchy-hotspot patcher inserted.
        CreateApKind::Legacy {
            patched: script.contains("cut -d. -f1"),
        }
    } else {
        CreateApKind::Unknown
    }
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    find_in(&env::var_os("PATH")?, binary)
}

fn find_in(path_var: &OsStr, binary: &str) -> Option<PathBuf> {
    env::split_paths(path_var)
        // Relative entries resolve against the working directory; never trust them as root.
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(binary))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &str = "VERSION=0.4.6\n\
        WIFI_IFACE_FREQ=$(iw dev ${WIFI_IFACE} link | grep -i freq | awk '{print $2}')\n";

    #[test]
    fn recognises_the_maintained_fork() {
        let fork = "VERSION=0.4.9\n    local FREQ_MAYBE_FRACTIONAL=$1\n";
        assert_eq!(classify_create_ap(fork), CreateApKind::Maintained);
    }

    #[test]
    fn recognises_the_original_with_and_without_the_old_patch() {
        assert_eq!(
            classify_create_ap(ORIGINAL),
            CreateApKind::Legacy { patched: false }
        );
        let patched = ORIGINAL.replace("awk '{print $2}')", "awk '{print $2}' | cut -d. -f1)");
        assert_eq!(
            classify_create_ap(&patched),
            CreateApKind::Legacy { patched: true }
        );
        assert_eq!(classify_create_ap("VERSION=2.0\n"), CreateApKind::Unknown);
    }

    #[test]
    fn finds_only_executable_files_on_the_path() {
        let root = env::temp_dir().join(format!("omarchy-hotspot-path-{}", process::id()));
        let _ = fs::remove_dir_all(&root);
        let (plain, bin) = (root.join("plain"), root.join("bin"));
        fs::create_dir_all(&plain).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::write(plain.join("tool"), "").unwrap();
        fs::write(bin.join("tool"), "").unwrap();
        fs::set_permissions(bin.join("tool"), fs::Permissions::from_mode(0o755)).unwrap();

        let path_var = env::join_paths([&plain, &bin]).unwrap();
        assert_eq!(find_in(&path_var, "tool"), Some(bin.join("tool")));
        assert_eq!(find_in(&path_var, "missing"), None);
        fs::remove_dir_all(root).unwrap();
    }
}
