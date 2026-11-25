//! Free space on the file system holding a path: what an unprivileged writer may still use
//! there (a Unix file system's root reserve excluded), so a daemon can refuse a write that
//! would leave its logs and autosave no room.

use std::io;
use std::path::Path;

/// Bytes an unprivileged process may still write on the file system holding `path`, which
/// must exist.
pub fn free_space(path: &Path) -> io::Result<u64> {
    imp::free_space(path)
}

#[cfg(unix)]
mod imp {
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    #[allow(unsafe_code)]
    pub(super) fn free_space(path: &Path) -> io::Result<u64> {
        let c = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path holds a NUL byte"))?;
        let mut st = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `c` is a NUL-terminated path that outlives the call, and `st` is a
        // properly sized out-parameter that statvfs fills before returning 0.
        let r = unsafe { libc::statvfs(c.as_ptr(), st.as_mut_ptr()) };
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: statvfs returned 0, so it filled `st`.
        let st = unsafe { st.assume_init() };
        // The field widths differ between Unix flavours (32-bit on macOS); widened alike.
        #[allow(clippy::useless_conversion)]
        let (avail, frsize) = (u64::from(st.f_bavail), u64::from(st.f_frsize));
        Ok(avail.saturating_mul(frsize))
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    #[allow(unsafe_code)]
    pub(super) fn free_space(path: &Path) -> io::Result<u64> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut avail: u64 = 0;
        // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call; the two
        // totals not asked for may be null, the one asked for is a valid u64.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut avail,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(avail)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_directory_has_free_space_and_a_missing_one_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(free_space(dir.path()).unwrap() > 0);
        assert!(free_space(&dir.path().join("missing")).is_err());
    }
}
