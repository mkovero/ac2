//! Whether a process still runs, for files named by the process that made them: one whose
//! process is gone is left over, and nothing else will remove it.

/// Whether a process `pid` runs now. A process this user may not signal or open still
/// counts as running. A pid reused by another process reads as running too, which only
/// keeps a leftover file a while longer.
pub fn alive(pid: u32) -> bool {
    imp::alive(pid)
}

#[cfg(unix)]
mod imp {
    #[allow(unsafe_code)]
    pub(super) fn alive(pid: u32) -> bool {
        // Signal 0 only checks: pid 0 and negative ids name process groups, never one process.
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        if pid <= 0 {
            return false;
        }
        // SAFETY: kill with signal 0 sends nothing and touches no memory.
        let r = unsafe { libc::kill(pid, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    #[allow(unsafe_code)]
    pub(super) fn alive(pid: u32) -> bool {
        // SAFETY: OpenProcess takes plain values; a null handle is checked before use.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            // SAFETY: reads this thread's last error, set by the failed call above.
            return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
        }
        let mut code: u32 = 0;
        // SAFETY: `h` is a handle opened above with query rights, `code` a valid u32; the
        // handle is closed once and not used after.
        let ok = unsafe { GetExitCodeProcess(h, &mut code) };
        unsafe { CloseHandle(h) };
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn this_process_runs_and_an_ended_child_does_not() {
        assert!(alive(std::process::id()));
        let exe = std::env::current_exe().unwrap();
        let mut child = std::process::Command::new(exe)
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!alive(pid));
    }
}
