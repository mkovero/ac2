//! JACK2 tears a client down by `pthread_cancel`ing its threads, and a thread may be inside
//! the libjack log callback when that happens. The log write is a cancellation point, where
//! glibc starts a forced unwind; if that unwind reached the callback's `catch_unwind`, it
//! would be swallowed and glibc would abort the process ("FATAL: exception not rethrown").
//! The process must survive, and the thread must end by cancellation. A process abort fails
//! every test in the binary, so this one has its own.
#![cfg(target_os = "linux")]

use std::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Writes every record to /dev/null: `write(2)` is a cancellation point, as it is for the
/// daemon's real logger.
struct NullLog(File);

impl log::Log for NullLog {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        let _ = writeln!(&self.0, "{}", record.args());
    }
    fn flush(&self) {}
}

// The libc crate binds neither on Linux; glibc and musl agree on both. "C-unwind": the
// forced unwind of a cancellation starts here and must be allowed to leave.
const PTHREAD_CANCELED: *mut c_void = -1_isize as *mut c_void;
#[allow(unsafe_code)]
unsafe extern "C-unwind" {
    fn pthread_testcancel();
}

static LOGGER: OnceLock<NullLog> = OnceLock::new();
static CALLS: AtomicU64 = AtomicU64::new(0);

// Stands in for a libjack client thread: its own cancellation point outside the callback is
// where a deferred cancellation acts. No destructors, so the forced unwind passes through
// this frame to `start_thread`, which is how a cancelled thread ends; "C-unwind" lets it
// (a "C" frame would abort on any unwind leaving it).
extern "C-unwind" fn libjack_thread(_: *mut c_void) -> *mut c_void {
    loop {
        ac2_audio::libjack_error_for_test(
            c"JackEngine::XRun: client ac2d finished after current callback",
        );
        CALLS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: plain libc call; the unwind it may start crosses no Rust destructors.
        #[allow(unsafe_code)]
        unsafe {
            pthread_testcancel()
        };
    }
}

#[test]
#[allow(unsafe_code)]
fn cancelling_a_thread_inside_the_libjack_log_callback_ends_the_thread() {
    let logger = LOGGER.get_or_init(|| NullLog(File::create("/dev/null").expect("open /dev/null")));
    log::set_logger(logger).expect("first logger in this process");
    log::set_max_level(log::LevelFilter::Trace);

    for _ in 0..20 {
        let mut thread: libc::pthread_t = 0;
        let before = CALLS.load(Ordering::Relaxed);
        // SAFETY: the entry point takes no argument and never returns normally.
        let rc = unsafe {
            libc::pthread_create(
                &mut thread,
                std::ptr::null(),
                // SAFETY: the two ABIs differ only in whether unwinding may leave the frame.
                std::mem::transmute::<
                    extern "C-unwind" fn(*mut c_void) -> *mut c_void,
                    extern "C" fn(*mut c_void) -> *mut c_void,
                >(libjack_thread),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, 0);
        // Let it run long enough to be inside the callback at a random point.
        let t0 = Instant::now();
        while CALLS.load(Ordering::Relaxed) < before + 100 {
            assert!(
                t0.elapsed() < Duration::from_secs(10),
                "callback thread stalled"
            );
            std::thread::yield_now();
        }
        let mut ret: *mut c_void = std::ptr::null_mut();
        // SAFETY: `thread` is a joinable thread created above and joined exactly once.
        unsafe {
            assert_eq!(libc::pthread_cancel(thread), 0);
            assert_eq!(libc::pthread_join(thread, &mut ret), 0);
        }
        assert_eq!(
            ret, PTHREAD_CANCELED,
            "thread ended other than by cancellation"
        );
    }
}
