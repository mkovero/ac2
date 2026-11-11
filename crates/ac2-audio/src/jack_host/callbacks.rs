//! The client's callbacks, registered with libjack directly.
//!
//! JACK2 stops a client's threads with `pthread_cancel`, and those threads are
//! asynchronously cancellable: a cancel can act at any instruction, and `jack_deactivate`
//! sends it without waiting for a cycle in progress to end. A cancellation is a forced
//! unwind. If it starts while a `catch_unwind` is on the stack, or at an instruction of a
//! Rust frame whose exception table does not cover that instruction, glibc or the Rust
//! personality aborts the process. The `jack` crate's trampolines hold a `catch_unwind`
//! around every callback, so a guard inside a handler cannot close that window; every
//! callback here enters through [`guarded`] instead.

use std::ffi::{c_char, c_int, c_void};
use std::mem::ManuallyDrop;
use std::ptr::NonNull;
use std::sync::Arc;

use jack::jack_sys as j;
use jack::{Client, ProcessScope};

use super::{Notifications, Process, backend_err};
use crate::error::{AudioError, Operation};
use crate::events::BackendEvents;

// The libc crate binds neither on Linux; glibc and musl agree on the values.
const PTHREAD_CANCEL_DISABLE: c_int = 1;
#[allow(unsafe_code)]
unsafe extern "C" {
    fn pthread_setcancelstate(state: c_int, oldstate: *mut c_int) -> c_int;
}

/// Runs a callback body on a libjack thread; false when the body panicked.
///
/// The body runs under `catch_unwind` (a panic must not unwind into libjack) with
/// cancellation disabled, so no cancel acts while the `catch_unwind` or any value with a
/// destructor is live. On an asynchronously cancellable thread a cancel that arrived
/// meanwhile acts inside the restoring `pthread_setcancelstate`; the Rust frames then on the
/// stack are this one and the trampoline that called it. Both are `extern "C"` and call only
/// functions that cannot unwind, so they get no landing pad and no exception table, and the
/// forced unwind passes through them into libjack. Nothing with a destructor may live in
/// either frame: that is why the body is borrowed rather than owned. On glibc
/// `pthread_setcancelstate` is an atomic update of the thread's own state, no system call,
/// so the process callback may use it.
// Only Rust calls these two; the "C" ABI is what makes a call to them non-unwinding.
#[allow(improper_ctypes_definitions)]
pub(super) extern "C" fn guarded(body: &mut dyn FnMut()) -> bool {
    let mut old: c_int = 0;
    // SAFETY: changes only the calling thread's cancel state; not a cancellation point.
    #[allow(unsafe_code)]
    unsafe {
        pthread_setcancelstate(PTHREAD_CANCEL_DISABLE, &mut old)
    };
    let ok = caught(body);
    let mut ignored: c_int = 0;
    // SAFETY: as above. Restoring an enabled state acts on a pending cancel here, which is
    // safe because nothing with a destructor and no `catch_unwind` is live in this frame.
    #[allow(unsafe_code)]
    unsafe {
        pthread_setcancelstate(old, &mut ignored)
    };
    ok
}

// Kept out of line: inlined, its `catch_unwind` would give `guarded` or a trampoline an
// exception table, and the restore call would sit outside it.
#[allow(improper_ctypes_definitions)]
#[inline(never)]
extern "C" fn caught(body: &mut dyn FnMut()) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_ok()
}

/// What libjack's callback argument points at. Each callback reaches only its own fields
/// through the raw pointer, never the whole struct: the process callback `client` and
/// `process`, the notification callbacks `notify`, shutdown `events`. So the threads never
/// hold references to the same field, except shared ones to `client`.
struct Handlers {
    client: ManuallyDrop<Client>,
    events: Arc<BackendEvents>,
    notify: Notifications,
    process: Process,
}

/// An activated client and its callbacks. Dropping it deactivates and closes the client,
/// which stops and joins libjack's threads, before the callbacks' state is freed.
pub(super) struct Active {
    handlers: NonNull<Handlers>,
}

// Both halves move to libjack's threads, as the `jack` crate's handlers did.
const _: fn() = || {
    fn send<T: Send>() {}
    send::<Notifications>();
    send::<Process>();
};

// SAFETY: `Active` owns the handlers; they are Send (checked above) and `Client` is Send
// and Sync. Other threads reach only `&Client` through `as_client`.
#[allow(unsafe_code)]
unsafe impl Send for Active {}
// SAFETY: as above.
#[allow(unsafe_code)]
unsafe impl Sync for Active {}

impl Active {
    /// Registers the callbacks on an open, inactive client and activates it.
    pub(super) fn activate(
        client: Client,
        events: Arc<BackendEvents>,
        notify: Notifications,
        process: Process,
    ) -> Result<Self, AudioError> {
        let raw = client.raw();
        let handlers = NonNull::from(Box::leak(Box::new(Handlers {
            client: ManuallyDrop::new(client),
            events,
            notify,
            process,
        })));
        // From here on dropping `active` closes the client and frees the handlers.
        let active = Active { handlers };
        let arg = handlers.as_ptr().cast::<c_void>();
        // SAFETY: `raw` is the open client `active` owns; `arg` stays valid until `active`'s
        // drop has closed the client, after which libjack calls none of these.
        #[allow(unsafe_code)]
        let set = unsafe {
            j::jack_on_info_shutdown(raw, Some(on_shutdown), arg);
            j::jack_set_process_callback(raw, Some(on_process), arg)
                | j::jack_set_buffer_size_callback(raw, Some(on_buffer_size), arg)
                | j::jack_set_sample_rate_callback(raw, Some(on_sample_rate), arg)
                | j::jack_set_xrun_callback(raw, Some(on_xrun), arg)
        };
        if set != 0 {
            return Err(backend_err(
                Operation::Start,
                "libjack refused a callback registration",
            ));
        }
        // SAFETY: as above.
        #[allow(unsafe_code)]
        let rc = unsafe { j::jack_activate(raw) };
        if rc != 0 {
            return Err(backend_err(
                Operation::Start,
                format_args!("jack_activate failed ({rc})"),
            ));
        }
        Ok(active)
    }

    pub(super) fn as_client(&self) -> &Client {
        // SAFETY: the handlers live as long as `self`; the client field is only read.
        #[allow(unsafe_code)]
        unsafe {
            &(*self.handlers.as_ptr()).client
        }
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        let h = self.handlers.as_ptr();
        // SAFETY: deactivating stops the process thread and closing joins libjack's other
        // threads, so no callback runs once `jack_client_close` returns; only then is the
        // allocation leaked in `activate` freed, once. Closing reaches only `client`.
        #[allow(unsafe_code)]
        unsafe {
            j::jack_deactivate((*h).client.raw());
            ManuallyDrop::drop(&mut (*h).client);
            drop(Box::from_raw(h));
        }
    }
}

// The trampolines below are `extern "C"`, call only `guarded`, and keep nothing with a
// destructor: see `guarded` for why each part of that matters.

#[allow(unsafe_code)]
unsafe extern "C" fn on_process(frames: j::jack_nframes_t, arg: *mut c_void) -> c_int {
    let h = arg.cast::<Handlers>();
    let ok = guarded(&mut || {
        // SAFETY: `arg` is the live `Handlers` (see `Active::activate`); only this thread
        // mutates `process`, and `client` is only read. The scope is this cycle's.
        let (client, process, ps) = unsafe {
            let client: &Client = &(*h).client;
            (
                client,
                &mut (*h).process,
                ProcessScope::from_raw(frames, client.raw()),
            )
        };
        process.cycle(client, &ps);
    });
    // Non-zero stops libjack calling a process callback whose state a panic left unknown.
    c_int::from(!ok)
}

#[allow(unsafe_code)]
unsafe extern "C" fn on_buffer_size(frames: j::jack_nframes_t, arg: *mut c_void) -> c_int {
    let h = arg.cast::<Handlers>();
    // SAFETY: the live `Handlers`; only the notification thread touches `notify`.
    guarded(&mut || unsafe { (*h).notify.buffer_size(frames) });
    0
}

#[allow(unsafe_code)]
unsafe extern "C" fn on_sample_rate(rate: j::jack_nframes_t, arg: *mut c_void) -> c_int {
    let h = arg.cast::<Handlers>();
    // SAFETY: as in `on_buffer_size`.
    guarded(&mut || unsafe { (*h).notify.sample_rate(rate) });
    0
}

#[allow(unsafe_code)]
unsafe extern "C" fn on_xrun(arg: *mut c_void) -> c_int {
    let h = arg.cast::<Handlers>();
    // SAFETY: as in `on_buffer_size`.
    guarded(&mut || unsafe { (*h).notify.xrun() });
    0
}

#[allow(unsafe_code)]
unsafe extern "C" fn on_shutdown(_: j::jack_status_t, _: *const c_char, arg: *mut c_void) {
    let h = arg.cast::<Handlers>();
    // SAFETY: the live `Handlers`; `events` is only read, through a shared reference. libjack
    // runs this where the client must not be used, and it touches only atomics.
    guarded(&mut || unsafe { (*h).events.end() });
}
