//! Drives `create_ap` without leaking credentials or disturbing other sessions.
//!
//! `create_ap` keeps one state directory per instance,
//! `/tmp/create_ap.<iface>.conf.XXXXXXXX`, holding its own `pid`, the
//! `wifi_iface` it broadcasts on, and `*.pid` files for the helpers it starts
//! (`hostapd`, `dnsmasq`). Those files let us target exact processes instead of
//! pattern-matching with `pkill`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

const TMP_DIR: &str = "/tmp";
const PROC_DIR: &str = "/proc";
/// Time for create_ap to react to the terminal's own Ctrl+C before we signal it.
const SELF_EXIT_GRACE: Duration = Duration::from_secs(2);
/// Time for create_ap's cleanup (iptables, virtual interface, helpers).
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Stdin for `create_ap <wifi> <internet>`.
///
/// Started without an SSID and passphrase and with a non-TTY stdin,
/// create_ap reads them with `read` instead, which keeps the password out of
/// the process list. `read` runs without `-r`, so backslashes and surrounding
/// blanks are escaped to arrive unchanged.
pub fn credentials_stdin(ssid: &str, password: &str) -> String {
    format!("{}\n{}\n", escape_for_read(ssid), escape_for_read(password))
}

fn escape_for_read(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '\\' | ' ' | '\t') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// create_ap prints `PID: <pid>` for its main process early during setup.
pub fn parse_pid_line(line: &str) -> Option<u32> {
    line.strip_prefix("PID: ")?.trim().parse().ok()
}

/// Virtual AP interfaces owned by create_ap instances that are still running.
pub fn interfaces_in_use() -> Vec<String> {
    interfaces_in_use_at(Path::new(TMP_DIR), Path::new(PROC_DIR))
}

/// Terminates hostapd/dnsmasq processes orphaned by create_ap instances that
/// died without cleaning up. Helpers of running instances are left alone.
pub fn kill_stale_helpers() {
    for pid in stale_helper_pids(Path::new(TMP_DIR), Path::new(PROC_DIR)) {
        outln!("  Stopping orphaned helper process {pid}...");
        send_signal(pid, "TERM");
    }
}

/// Stops our create_ap instance (run through `sudo`, as `child`) and waits
/// for its own cleanup to finish.
pub fn stop(child: &mut Child, create_ap_pid: Option<u32>) {
    if wait_for_exit(child, SELF_EXIT_GRACE) {
        return;
    }
    let proc_dir = Path::new(PROC_DIR);
    if let Some(pid) = create_ap_pid.filter(|&pid| is_create_ap(proc_dir, pid)) {
        send_signal(pid, "INT");
        if wait_for_exit(child, CLEANUP_TIMEOUT) {
            return;
        }
        errln!("Warning: create_ap did not finish its cleanup in time; forcing it to stop.");
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            _ => return false,
        }
    }
}

fn send_signal(pid: u32, signal: &str) {
    let result = Command::new("sudo")
        .args(["kill", "-s", signal, &pid.to_string()])
        .status();
    if !matches!(result, Ok(status) if status.success()) {
        errln!("Warning: failed to send SIG{signal} to process {pid}.");
    }
}

fn conf_dirs(tmp_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(tmp_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("create_ap.") && name.contains(".conf."))
        })
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

fn read_pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn cmdline(proc_dir: &Path, pid: u32) -> Option<String> {
    let raw = fs::read(proc_dir.join(pid.to_string()).join("cmdline")).ok()?;
    Some(String::from_utf8_lossy(&raw).replace('\0', " "))
}

/// Guards against PID reuse: the process must still be a create_ap script.
fn is_create_ap(proc_dir: &Path, pid: u32) -> bool {
    cmdline(proc_dir, pid).is_some_and(|cmd| cmd.contains("create_ap"))
}

fn is_running_instance(proc_dir: &Path, conf_dir: &Path) -> bool {
    read_pid(&conf_dir.join("pid")).is_some_and(|pid| is_create_ap(proc_dir, pid))
}

fn interfaces_in_use_at(tmp_dir: &Path, proc_dir: &Path) -> Vec<String> {
    conf_dirs(tmp_dir)
        .iter()
        .filter(|dir| is_running_instance(proc_dir, dir))
        .filter_map(|dir| fs::read_to_string(dir.join("wifi_iface")).ok())
        .map(|iface| iface.trim().to_string())
        .filter(|iface| !iface.is_empty())
        .collect()
}

fn stale_helper_pids(tmp_dir: &Path, proc_dir: &Path) -> Vec<u32> {
    let mut pids = Vec::new();
    for dir in conf_dirs(tmp_dir) {
        if !dir.join("pid").exists() || is_running_instance(proc_dir, &dir) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let dir_marker = dir.to_string_lossy().into_owned();
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            let is_helper_pid_file = path.extension().is_some_and(|ext| ext == "pid")
                && path.file_name().is_some_and(|name| name != "pid");
            if !is_helper_pid_file {
                continue;
            }
            // Helpers are started with config files from this directory, so a
            // matching command line also rules out a reused PID.
            if let Some(pid) = read_pid(&path)
                && cmdline(proc_dir, pid).is_some_and(|cmd| cmd.contains(&dir_marker))
            {
                pids.push(pid);
            }
        }
    }
    pids.sort_unstable();
    pids
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        tmp: PathBuf,
        proc: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "omarchy-hotspot-create-ap-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            let (tmp, proc) = (root.join("tmp"), root.join("proc"));
            fs::create_dir_all(&tmp).unwrap();
            fs::create_dir_all(&proc).unwrap();
            Self { root, tmp, proc }
        }

        fn process(&self, pid: u32, args: &[&str]) {
            let dir = self.proc.join(pid.to_string());
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("cmdline"), args.join("\0")).unwrap();
        }

        fn instance(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
            let dir = self.tmp.join(name);
            fs::create_dir_all(&dir).unwrap();
            for (file, content) in files {
                fs::write(dir.join(file), content).unwrap();
            }
            dir
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn credentials_survive_bash_read_without_r() {
        assert_eq!(
            credentials_stdin("My Net\\x", "  p\\a;ss \t"),
            "My\\ Net\\\\x\n\\ \\ p\\\\a;ss\\ \\\t\n"
        );
    }

    #[test]
    fn parses_create_ap_pid_line() {
        assert_eq!(parse_pid_line("PID: 4242"), Some(4242));
        assert_eq!(parse_pid_line("Config dir: /tmp/create_ap.x"), None);
        assert_eq!(parse_pid_line("PID: abc"), None);
    }

    #[test]
    fn only_orphaned_helpers_of_dead_instances_are_targeted() {
        let fx = Fixture::new("stale");
        let dead = fx.instance(
            "create_ap.wlan0.conf.AAAA",
            &[
                ("pid", "100\n"),
                ("dnsmasq.pid", "101\n"),
                ("hostapd.pid", "102\n"),
            ],
        );
        fx.process(
            101,
            &["dnsmasq", "-C", &format!("{}/dnsmasq.conf", dead.display())],
        );
        fx.process(102, &["firefox"]); // PID reused by an unrelated process

        let live = fx.instance(
            "create_ap.wlan1.conf.BBBB",
            &[("pid", "200\n"), ("dnsmasq.pid", "201\n")],
        );
        fx.process(200, &["/bin/bash", "/usr/bin/create_ap", "wlan1", "eth0"]);
        fx.process(
            201,
            &["dnsmasq", "-C", &format!("{}/dnsmasq.conf", live.display())],
        );
        fx.instance("create_ap.common.conf", &[("haveged.pid", "300\n")]);

        assert_eq!(stale_helper_pids(&fx.tmp, &fx.proc), vec![101]);
    }

    #[test]
    fn interfaces_of_running_instances_are_reported_in_use() {
        let fx = Fixture::new("ifaces");
        fx.instance(
            "create_ap.wlan0.conf.AAAA",
            &[("pid", "100\n"), ("wifi_iface", "ap0\n")],
        );
        fx.instance(
            "create_ap.wlan1.conf.BBBB",
            &[("pid", "200\n"), ("wifi_iface", "ap1\n")],
        );
        fx.process(200, &["/bin/bash", "/usr/bin/create_ap", "wlan1", "eth0"]);

        assert_eq!(interfaces_in_use_at(&fx.tmp, &fx.proc), vec!["ap1"]);
    }
}
