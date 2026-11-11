//! JACK2 tears a client down by `pthread_cancel`ing its threads, which are asynchronously
//! cancellable: the cancel may act at any instruction, including inside a callback. A
//! cancellation is a forced unwind; if it reached a `catch_unwind`, glibc would abort the
//! process ("FATAL: exception not rethrown"). Every libjack callback body runs through one
//! guard; here a stand-in libjack thread runs that guard, and the log callback built on it,
//! while being cancelled. The process must survive, and the thread must end by cancellation.
//! A process abort fails every test in the binary, so this one has its own.
#![cfg(target_os = "linux")]

use std::ffi::{c_int, c_void};
use std::fs::File;
use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

// The libc crate binds neither on Linux; glibc and musl agree on both values.
const PTHREAD_CANCELED: *mut c_void = -1_isize as *mut c_void;
const PTHREAD_CANCEL_ASYNCHRONOUS: c_int = 1;
#[allow(unsafe_code)]
unsafe extern "C" {
    fn pthread_setcanceltype(kind: c_int, old: *mut c_int) -> c_int;
}

static LOGGER: OnceLock<NullLog> = OnceLock::new();
static CALLS: AtomicU64 = AtomicU64::new(0);
static INSIDE: AtomicBool = AtomicBool::new(false);
static LOG_PATH: AtomicBool = AtomicBool::new(false);

/// A callback body that holds a value with a destructor across a long stretch, so a cancel
/// sent while it runs finds it live.
fn body() {
    INSIDE.store(true, Ordering::Relaxed);
    let v = vec![1u64; std::hint::black_box(64)];
    let mut x = 0u64;
    for i in 0..20_000u64 {
        x = std::hint::black_box(x.wrapping_add(i + v[0]));
    }
    log::debug!("{x}");
    INSIDE.store(false, Ordering::Relaxed);
}

// Stands in for a libjack client thread: asynchronously cancellable, as JACK2 makes its
// client threads. Holds nothing with a destructor, so the forced unwind passes through
// this frame to `start_thread`, which is how a cancelled thread ends; "C-unwind" lets it
// (a "C" frame would abort on any unwind leaving it).
extern "C-unwind" fn libjack_thread(_: *mut c_void) -> *mut c_void {
    // SAFETY: changes only this thread's cancel type.
    #[allow(unsafe_code)]
    unsafe {
        pthread_setcanceltype(PTHREAD_CANCEL_ASYNCHRONOUS, std::ptr::null_mut())
    };
    loop {
        if LOG_PATH.load(Ordering::Relaxed) {
            ac2_audio::libjack_error_for_test(
                c"JackEngine::XRun: client ac2d finished after current callback",
            );
        } else {
            ac2_audio::libjack_guarded_for_test(&mut body);
        }
        CALLS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Starts a stand-in thread, lets it run, cancels it (inside the guarded body when
/// `inside`, else wherever it is) and checks it ended by cancellation.
#[allow(unsafe_code)]
fn cancel_one(inside: bool) {
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
    let t0 = Instant::now();
    while CALLS.load(Ordering::Relaxed) < before + 20 || (inside && !INSIDE.load(Ordering::Relaxed))
    {
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "callback thread stalled"
        );
        std::hint::spin_loop();
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

#[test]
fn cancelling_a_libjack_thread_inside_a_callback_ends_the_thread() {
    let logger = LOGGER.get_or_init(|| NullLog(File::create("/dev/null").expect("open /dev/null")));
    log::set_logger(logger).expect("first logger in this process");
    log::set_max_level(log::LevelFilter::Trace);

    for round in 0..100 {
        cancel_one(round % 2 == 0);
    }
    LOG_PATH.store(true, Ordering::Relaxed);
    for _ in 0..50 {
        cancel_one(false);
    }
}
