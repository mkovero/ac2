//! Sleeping until a deadline, with the wakeup on time.
//!
//! Live meters and plots are paced by threads that sleep between capture hand-offs, so a
//! wakeup that comes late shows up directly as a lower frame rate. macOS coalesces timers to
//! save power: an ordinary timed wait (`thread::sleep`, a timed condition or channel wait, a
//! plain kqueue timer) may fire long after its deadline, by several times the requested
//! interval on a loaded or virtualised machine, and raising the thread's QoS class does not
//! change that. A kqueue timer marked `NOTE_CRITICAL` opts out of coalescing; it is what
//! [`Sleeper`] uses there. Elsewhere a condition variable's timed wait already wakes on time.

use std::io;
use std::time::Instant;

/// Sleeps until a deadline or until its [`Waker`] is used. Owned by the sleeping thread.
#[derive(Debug)]
pub struct Sleeper(imp::Sleeper);

/// Wakes a [`Sleeper`] from another thread.
#[derive(Debug, Clone)]
pub struct Waker(imp::Waker);

/// A sleeper and the waker that ends its sleep.
pub fn sleeper() -> io::Result<(Sleeper, Waker)> {
    let (s, w) = imp::pair()?;
    Ok((Sleeper(s), Waker(w)))
}

impl Sleeper {
    /// Returns at `deadline`, when woken, or earlier for no reason at all: callers check
    /// what they are waiting for and sleep again.
    pub fn sleep_until(&self, deadline: Instant) {
        if deadline > Instant::now() {
            self.0.sleep_until(deadline);
        }
    }
}

impl Waker {
    /// Ends the current or the next sleep.
    pub fn wake(&self) {
        self.0.wake();
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod imp {
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::Arc;
    use std::time::Instant;

    const WAKE_ID: libc::uintptr_t = 1;
    const TIMER_ID: libc::uintptr_t = 2;

    #[derive(Debug)]
    pub(super) struct Sleeper {
        kq: Arc<OwnedFd>,
    }

    #[derive(Debug, Clone)]
    pub(super) struct Waker {
        kq: Arc<OwnedFd>,
    }

    fn event(
        ident: libc::uintptr_t,
        filter: i16,
        flags: u16,
        fflags: u32,
        data: isize,
    ) -> libc::kevent {
        libc::kevent {
            ident,
            filter,
            flags,
            fflags,
            data,
            udata: std::ptr::null_mut(),
        }
    }

    /// Applies `change` and, if `wait`, blocks for one event.
    fn kevent(kq: &OwnedFd, change: &libc::kevent, wait: bool) -> io::Result<()> {
        let mut out = event(0, 0, 0, 0, 0);
        let n_out = i32::from(wait);
        loop {
            // SAFETY: `change` and `out` are valid for one event each and outlive the call;
            // the descriptor is a kqueue owned by `kq`.
            let r = unsafe {
                libc::kevent(
                    kq.as_raw_fd(),
                    change,
                    1,
                    &raw mut out,
                    n_out,
                    std::ptr::null(),
                )
            };
            if r >= 0 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
            if wait {
                // Interrupted while waiting: the change was applied; a spurious return is
                // allowed.
                return Ok(());
            }
        }
    }

    pub(super) fn pair() -> io::Result<(Sleeper, Waker)> {
        // SAFETY: no arguments; returns a new descriptor or -1.
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a freshly created descriptor nothing else owns.
        let kq = Arc::new(unsafe { OwnedFd::from_raw_fd(fd) });
        // A user event the waker triggers; EV_CLEAR resets it once a sleep has seen it.
        kevent(
            &kq,
            &event(
                WAKE_ID,
                libc::EVFILT_USER,
                libc::EV_ADD | libc::EV_CLEAR,
                0,
                0,
            ),
            false,
        )?;
        Ok((
            Sleeper {
                kq: Arc::clone(&kq),
            },
            Waker { kq },
        ))
    }

    impl Sleeper {
        pub(super) fn sleep_until(&self, deadline: Instant) {
            let ns = deadline
                .saturating_duration_since(Instant::now())
                .as_nanos();
            let ns = isize::try_from(ns).unwrap_or(isize::MAX).max(1);
            // Re-adding the timer re-arms it, so a timer left over from a sleep the waker
            // ended can at most end this one early.
            let timer = event(
                TIMER_ID,
                libc::EVFILT_TIMER,
                libc::EV_ADD | libc::EV_ONESHOT,
                libc::NOTE_NSECONDS | libc::NOTE_CRITICAL,
                ns,
            );
            if kevent(&self.kq, &timer, true).is_err() {
                // Not expected for a valid kqueue. A coalesced sleep is late, but a caller
                // retrying a failing wait at once would spin.
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            }
        }
    }

    impl Waker {
        pub(super) fn wake(&self) {
            let trigger = event(WAKE_ID, libc::EVFILT_USER, 0, libc::NOTE_TRIGGER, 0);
            let _ = kevent(&self.kq, &trigger, false);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::io;
    use std::sync::{Arc, Condvar, Mutex, PoisonError};
    use std::time::Instant;

    #[derive(Debug, Default)]
    struct Shared {
        woken: Mutex<bool>,
        cv: Condvar,
    }

    #[derive(Debug)]
    pub(super) struct Sleeper(Arc<Shared>);

    #[derive(Debug, Clone)]
    pub(super) struct Waker(Arc<Shared>);

    pub(super) fn pair() -> io::Result<(Sleeper, Waker)> {
        let s = Arc::new(Shared::default());
        Ok((Sleeper(Arc::clone(&s)), Waker(s)))
    }

    impl Sleeper {
        pub(super) fn sleep_until(&self, deadline: Instant) {
            let mut woken = self.0.woken.lock().unwrap_or_else(PoisonError::into_inner);
            if !*woken {
                let wait = deadline.saturating_duration_since(Instant::now());
                woken = self
                    .0
                    .cv
                    .wait_timeout(woken, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            *woken = false;
        }
    }

    impl Waker {
        pub(super) fn wake(&self) {
            *self.0.woken.lock().unwrap_or_else(PoisonError::into_inner) = true;
            self.0.cv.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn sleeps_until_the_deadline() {
        let (s, _w) = sleeper().expect("sleeper");
        let t0 = Instant::now();
        let deadline = t0 + Duration::from_millis(20);
        while Instant::now() < deadline {
            s.sleep_until(deadline);
        }
        assert!(t0.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn a_wake_ends_the_sleep_and_one_before_it_is_not_lost() {
        let (s, w) = sleeper().expect("sleeper");
        w.wake();
        let t0 = Instant::now();
        s.sleep_until(t0 + Duration::from_secs(10));
        assert!(t0.elapsed() < Duration::from_secs(5));

        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            w.wake();
        });
        let t0 = Instant::now();
        s.sleep_until(t0 + Duration::from_secs(10));
        assert!(t0.elapsed() < Duration::from_secs(5));
        t.join().expect("waker thread");
    }
}
