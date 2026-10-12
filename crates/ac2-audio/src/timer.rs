//! Sleeping until a deadline, with the wakeup on time.
//!
//! Live meters and plots are paced by threads that sleep between capture hand-offs, so a
//! wakeup that comes late shows up directly as a lower frame rate. macOS coalesces timers to
//! save power: an ordinary timed wait (`thread::sleep`, a timed condition or channel wait, a
//! plain kqueue timer) may fire long after its deadline, by several times the requested
//! interval on a loaded or virtualised machine, and raising the thread's QoS class does not
//! change that. A kqueue timer marked `NOTE_CRITICAL` opts out of coalescing; it is what
//! [`Sleeper`] uses there. Elsewhere a condition variable's timed wait already wakes on time.
//!
//! Windows rounds timed waits to its timer tick instead; [`fine_resolution`] shortens the
//! tick, and [`pro_audio_thread`] puts a thread that drains captured audio in the audio
//! scheduling class.

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

/// Keeps the process's timed waits at about a millisecond while alive.
///
/// Windows ends every timed wait that is not a high-resolution waitable timer on a
/// system clock tick, 15.6 ms unless a process asks for less. That covers a condition
/// variable's or channel's timed wait (the [`Sleeper`] above, the daemon's job and control
/// loops), `zmq_poll`, and tokio's and the client's timers. A 33 ms frame hand-off then
/// comes after 33 to 47 ms, so a 30 fps stream arrives at an uneven 21 to 30, and a frame
/// held back 16 ms waits up to 31. One millisecond keeps every one of those waits within
/// a millisecond of its deadline. Elsewhere this does nothing: the kernels there already
/// wake on time (macOS through [`Sleeper`]'s kqueue timer).
#[derive(Debug)]
#[must_use = "the resolution falls back when this is dropped"]
pub struct FineResolution {
    #[cfg(windows)]
    begun: bool,
}

/// Asks for millisecond timer resolution until the returned value is dropped.
pub fn fine_resolution() -> FineResolution {
    FineResolution {
        #[cfg(windows)]
        begun: win::time_begin_period(),
    }
}

#[cfg(windows)]
impl Drop for FineResolution {
    fn drop(&mut self) {
        if self.begun {
            win::time_end_period();
        }
    }
}

/// The calling thread registered with the Windows multimedia scheduler (MMCSS) as a
/// "Pro Audio" task until dropped, on the thread that registered it.
///
/// A thread that moves captured audio on from the device's ring has a deadline set by the
/// ring's length, not by how busy the machine is: at normal priority, analysis threads
/// using every core can starve it. MMCSS raises it into the band the audio stack itself
/// uses while capping it below a full core, so a runaway thread there cannot freeze the
/// desktop. Elsewhere this does nothing: JACK and Core Audio deliver blocks on their own
/// real-time threads.
#[derive(Debug)]
#[must_use = "the thread leaves MMCSS when this is dropped"]
pub struct ProAudioThread {
    #[cfg(windows)]
    _task: win::MmcssTask,
    /// Reverting must happen on the registered thread.
    _not_send: std::marker::PhantomData<*const ()>,
}

/// Registers the calling thread as a "Pro Audio" task (Windows; elsewhere a no-op).
///
/// # Errors
/// The scheduler refused (service stopped, task name missing from the registry); the
/// thread keeps its ordinary priority.
pub fn pro_audio_thread() -> io::Result<ProAudioThread> {
    Ok(ProAudioThread {
        #[cfg(windows)]
        _task: win::MmcssTask::join_pro_audio()?,
        _not_send: std::marker::PhantomData,
    })
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod win {
    use std::io;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Media::{TIMERR_NOERROR, timeBeginPeriod, timeEndPeriod};
    use windows_sys::Win32::System::Threading::{
        AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
    };

    const PERIOD_MS: u32 = 1;

    pub(super) fn time_begin_period() -> bool {
        // SAFETY: no pointers; a request the system may round to what the hardware offers.
        unsafe { timeBeginPeriod(PERIOD_MS) == TIMERR_NOERROR }
    }

    pub(super) fn time_end_period() {
        // SAFETY: paired with a successful `timeBeginPeriod` of the same period.
        unsafe {
            timeEndPeriod(PERIOD_MS);
        }
    }

    #[derive(Debug)]
    pub(super) struct MmcssTask(HANDLE);

    impl MmcssTask {
        pub(super) fn join_pro_audio() -> io::Result<Self> {
            let name: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
            let mut index = 0u32;
            // SAFETY: `name` is NUL-terminated UTF-16 and outlives the call; `index` is a
            // valid out pointer (0 asks for a new task).
            let h = unsafe { AvSetMmThreadCharacteristicsW(name.as_ptr(), &raw mut index) };
            if h.is_null() {
                Err(io::Error::last_os_error())
            } else {
                Ok(Self(h))
            }
        }
    }

    impl Drop for MmcssTask {
        fn drop(&mut self) {
            // SAFETY: the handle came from `AvSetMmThreadCharacteristicsW` on this thread
            // (`ProAudioThread` is not `Send`) and is reverted once.
            unsafe {
                AvRevertMmThreadCharacteristics(self.0);
            }
        }
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

    #[test]
    fn scheduling_guards_are_taken_and_released() {
        let fine = fine_resolution();
        // A Windows host may run without the multimedia scheduler; elsewhere it is a no-op.
        let task = pro_audio_thread();
        #[cfg(not(windows))]
        assert!(task.is_ok());
        drop(task);
        drop(fine);
    }
}
