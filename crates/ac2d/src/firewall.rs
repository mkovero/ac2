//! Network mode and the host firewall: a firewall that drops the two TCP ports makes the
//! daemon unreachable without a word on either side (the client only sees "not
//! responding"), so the daemon names the ports at startup and warns when it can see an
//! active firewall.
//!
//! Detection is best effort and needs no privileges: ufw's own switch in
//! `/etc/ufw/ufw.conf`, and a running `firewalld`. Rules themselves are only readable by
//! root, so an open port cannot be confirmed; the warning says what to allow either way.

use std::path::Path;

/// A host firewall the daemon can see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Firewall {
    Ufw,
    Firewalld,
}

/// `ENABLED=yes` in ufw's configuration (comments and spacing as ufw writes them).
fn ufw_enabled(conf: &str) -> bool {
    conf.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .any(|(k, v)| k.trim() == "ENABLED" && v.trim().trim_matches(['"', '\'']) == "yes")
}

/// Whether a process named `name` runs, from `/proc/<pid>/comm`.
fn process_running(proc: &Path, name: &str) -> bool {
    let Ok(dir) = std::fs::read_dir(proc) else {
        return false;
    };
    dir.flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        .any(|e| std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == name))
}

/// Active firewalls on this host that the daemon can see.
pub(crate) fn detect() -> Vec<Firewall> {
    detect_in(Path::new("/etc/ufw/ufw.conf"), Path::new("/proc"))
}

fn detect_in(ufw_conf: &Path, proc: &Path) -> Vec<Firewall> {
    let mut found = Vec::new();
    if std::fs::read_to_string(ufw_conf).is_ok_and(|c| ufw_enabled(&c)) {
        found.push(Firewall::Ufw);
    }
    if process_running(proc, "firewalld") {
        found.push(Firewall::Firewalld);
    }
    found
}

/// The TCP port of a `tcp://host:port` endpoint.
pub(crate) fn tcp_port(endpoint: &str) -> Option<u16> {
    endpoint
        .strip_prefix("tcp://")?
        .rsplit_once(':')?
        .1
        .parse()
        .ok()
}

/// What to allow, one rule per port: with a port range, ufw lists the rule but a kernel
/// without the iptables `multiport` match (some real-time kernels) never enforces it.
pub(crate) fn advice(fw: Firewall, ctrl: u16, data: u16) -> String {
    match fw {
        Firewall::Ufw => format!(
            "ufw is enabled on this host: remote clients get no answer unless TCP ports {ctrl} \
             and {data} are allowed, one rule per port: `sudo ufw allow {ctrl}/tcp` and `sudo \
             ufw allow {data}/tcp`"
        ),
        Firewall::Firewalld => format!(
            "firewalld runs on this host: remote clients get no answer unless TCP ports {ctrl} \
             and {data} are open: `sudo firewall-cmd --permanent --add-port={ctrl}/tcp \
             --add-port={data}/tcp` then `sudo firewall-cmd --reload`"
        ),
    }
}

/// Logs the ports a network-mode daemon needs open, and a warning per firewall it sees.
pub(crate) fn report(ctrl_endpoint: &str, data_endpoint: &str) {
    let (Some(ctrl), Some(data)) = (tcp_port(ctrl_endpoint), tcp_port(data_endpoint)) else {
        return;
    };
    tracing::info!(
        "network mode: remote clients connect to TCP ports {ctrl} (ctrl) and {data} (data); a \
         host firewall must allow both"
    );
    for fw in detect() {
        tracing::warn!("{}", advice(fw, ctrl, data));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ufw_switch_is_read_like_ufw_writes_it() {
        assert!(ufw_enabled(
            "# /etc/ufw/ufw.conf\nENABLED=yes\nLOGLEVEL=low\n"
        ));
        assert!(ufw_enabled("ENABLED = \"yes\"\n"));
        assert!(!ufw_enabled("ENABLED=no\n"));
        assert!(!ufw_enabled("#ENABLED=yes\n"));
        assert!(!ufw_enabled(""));
    }

    #[test]
    fn detection_reads_the_config_and_process_list() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let conf = dir.path().join("ufw.conf");
        let proc = dir.path().join("proc");
        std::fs::create_dir_all(proc.join("42")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(proc.join("42/comm"), "firewalld\n").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(detect_in(&conf, &proc), [Firewall::Firewalld]);
        std::fs::write(&conf, "ENABLED=yes\n").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            detect_in(&conf, &proc),
            [Firewall::Ufw, Firewall::Firewalld]
        );
        assert!(detect_in(&dir.path().join("none"), &dir.path().join("none")).is_empty());
    }

    #[test]
    fn advice_names_each_port_on_its_own() {
        assert_eq!(tcp_port("tcp://0.0.0.0:47820"), Some(47820));
        assert_eq!(tcp_port("tcp://[::]:5000"), Some(5000));
        assert_eq!(tcp_port("ipc:///run/x"), None);
        let a = advice(Firewall::Ufw, 47820, 47821);
        assert!(
            a.contains("ufw allow 47820/tcp") && a.contains("ufw allow 47821/tcp"),
            "{a}"
        );
        assert!(!a.contains("47820:47821"), "never a port range: {a}");
        let f = advice(Firewall::Firewalld, 47820, 47821);
        assert!(f.contains("--add-port=47820/tcp") && f.contains("--add-port=47821/tcp"));
    }
}
