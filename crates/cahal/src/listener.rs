//! Property listeners: a boxed closure registered for some addresses of one object.
//!
//! The HAL calls listeners on its own notification thread. The closure is freed only after
//! every address is unregistered and no call is still inside it.

use std::ffi::c_void;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

use objc2_core_audio as ca;

use crate::types::{Error, FourCc};

type Callback = Box<dyn Fn(u32) + Send + Sync>;

struct Inner {
    f: Callback,
    inside: AtomicUsize,
}

/// A registered listener; dropping it unregisters it.
pub struct Listener {
    object: u32,
    addrs: Vec<ca::AudioObjectPropertyAddress>,
    inner: NonNull<Inner>,
}

// SAFETY: `inner` is only shared with the HAL's notification thread, through `Fn + Send +
// Sync`; the pointer itself is owned here.
unsafe impl Send for Listener {}
// SAFETY: as above; `&Listener` gives no access to the closure.
unsafe impl Sync for Listener {}

impl fmt::Debug for Listener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Listener")
            .field("object", &self.object)
            .field(
                "selectors",
                &self
                    .addrs
                    .iter()
                    .map(|a| FourCc(a.mSelector))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

unsafe extern "C-unwind" fn trampoline(
    _object: u32,
    count: u32,
    addrs: NonNull<ca::AudioObjectPropertyAddress>,
    data: *mut c_void,
) -> i32 {
    // SAFETY: `data` is the `Inner` registered with this trampoline, freed only after
    // unregistering and waiting for `inside` to drop to zero.
    let inner = unsafe { &*data.cast::<Inner>() };
    inner.inside.fetch_add(1, Ordering::SeqCst);
    for i in 0..count as usize {
        // SAFETY: the HAL passes `count` addresses.
        let sel = unsafe { addrs.add(i).as_ref() }.mSelector;
        // A panic must not unwind into the HAL.
        let _ = catch_unwind(AssertUnwindSafe(|| (inner.f)(sel)));
    }
    inner.inside.fetch_sub(1, Ordering::SeqCst);
    0
}

impl Listener {
    /// Registers `f` for `addrs` on `object`. `f` gets each changed address's selector.
    pub(crate) fn register(
        object: u32,
        addrs: Vec<ca::AudioObjectPropertyAddress>,
        f: Callback,
    ) -> Result<Self, Error> {
        let inner = NonNull::from(Box::leak(Box::new(Inner {
            f,
            inside: AtomicUsize::new(0),
        })));
        let mut l = Self {
            object,
            addrs: Vec::with_capacity(addrs.len()),
            inner,
        };
        for a in addrs {
            // SAFETY: `inner` stays alive until `Drop` has unregistered this address.
            let st = unsafe {
                ca::AudioObjectAddPropertyListener(
                    object,
                    NonNull::from(&a),
                    Some(trampoline),
                    inner.as_ptr().cast(),
                )
            };
            if st != 0 {
                // Dropping `l` unregisters what was registered so far.
                return Err(Error::Status {
                    call: "AudioObjectAddPropertyListener",
                    selector: FourCc(a.mSelector),
                    status: st,
                });
            }
            l.addrs.push(a);
        }
        Ok(l)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        for a in &self.addrs {
            // SAFETY: the same address, trampoline and data as registered.
            unsafe {
                ca::AudioObjectRemovePropertyListener(
                    self.object,
                    NonNull::from(a),
                    Some(trampoline),
                    self.inner.as_ptr().cast(),
                );
            }
        }
        // SAFETY: `inner` is alive until the box is rebuilt below.
        let inside = unsafe { &self.inner.as_ref().inside };
        while inside.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
        // SAFETY: leaked in `register`, unregistered everywhere, and no call is inside.
        drop(unsafe { Box::from_raw(self.inner.as_ptr()) });
    }
}
