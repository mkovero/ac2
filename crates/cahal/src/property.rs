//! Typed reads and writes of HAL properties (`AudioObject…PropertyData`).
//!
//! Every read checks the size the HAL returned against the type, and every `CFString` the
//! HAL hands over (+1 retained) is released once read.

use std::ffi::c_void;
use std::mem::{MaybeUninit, size_of};
use std::ptr::NonNull;

use objc2_core_audio as ca;
use objc2_core_audio_types::{AudioStreamBasicDescription, AudioValueRange, AudioValueTranslation};
use objc2_core_foundation::{CFRetained, CFString};

use crate::types::{Error, FourCc, Scope, StreamSet};

/// The HAL's system object.
pub(crate) const SYSTEM: u32 = ca::kAudioObjectSystemObject as u32;
/// `kAudioObjectPropertyScopeGlobal`.
pub(crate) const GLOBAL: u32 = ca::kAudioObjectPropertyScopeGlobal;
/// `kAudioObjectPropertyElementMain`.
pub(crate) const MAIN: u32 = ca::kAudioObjectPropertyElementMain;

/// The scope selector of a direction.
pub(crate) fn scope(s: Scope) -> u32 {
    match s {
        Scope::Input => ca::kAudioObjectPropertyScopeInput,
        Scope::Output => ca::kAudioObjectPropertyScopeOutput,
    }
}

/// A property address.
pub(crate) fn addr(selector: u32, scope: u32, element: u32) -> ca::AudioObjectPropertyAddress {
    ca::AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: element,
    }
}

/// A global, main-element address.
pub(crate) fn global(selector: u32) -> ca::AudioObjectPropertyAddress {
    addr(selector, GLOBAL, MAIN)
}

fn check(call: &'static str, a: &ca::AudioObjectPropertyAddress, status: i32) -> Result<(), Error> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error::Status {
            call,
            selector: FourCc(a.mSelector),
            status,
        })
    }
}

/// Types a property can be read into: plain old data, valid for any bit pattern the HAL
/// writes.
///
/// # Safety
///
/// Every bit pattern of `size_of::<Self>()` bytes must be a valid value.
pub(crate) unsafe trait Plain: Copy {}
// SAFETY: integers and floats are valid for any bits.
unsafe impl Plain for u32 {}
// SAFETY: as above.
unsafe impl Plain for i32 {}
// SAFETY: as above.
unsafe impl Plain for f64 {}
// SAFETY: two f64.
unsafe impl Plain for AudioValueRange {}
// SAFETY: an f64 and eight u32.
unsafe impl Plain for AudioStreamBasicDescription {}

/// Whether the object has the property.
pub(crate) fn has(object: u32, a: ca::AudioObjectPropertyAddress) -> bool {
    // SAFETY: `a` is a valid address for the duration of the call.
    unsafe { ca::AudioObjectHasProperty(object, NonNull::from(&a)) }
}

/// The property's size in bytes.
fn size(object: u32, a: &ca::AudioObjectPropertyAddress) -> Result<u32, Error> {
    let mut n = 0u32;
    // SAFETY: `a` and `n` outlive the call; no qualifier.
    let st = unsafe {
        ca::AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(a),
            0,
            std::ptr::null(),
            NonNull::from(&mut n),
        )
    };
    check("AudioObjectGetPropertyDataSize", a, st)?;
    Ok(n)
}

/// Reads a property of exactly `size_of::<T>()` bytes.
pub(crate) fn get<T: Plain>(object: u32, a: ca::AudioObjectPropertyAddress) -> Result<T, Error> {
    let mut v = MaybeUninit::<T>::zeroed();
    let mut n = size_of::<T>() as u32;
    // SAFETY: `v` has room for `n` bytes; `a` and `n` outlive the call.
    let st = unsafe {
        ca::AudioObjectGetPropertyData(
            object,
            NonNull::from(&a),
            0,
            std::ptr::null(),
            NonNull::from(&mut n),
            NonNull::from(&mut v).cast(),
        )
    };
    check("AudioObjectGetPropertyData", &a, st)?;
    if n as usize != size_of::<T>() {
        return Err(Error::Unsupported("a property of an unexpected size"));
    }
    // SAFETY: zero-initialised and fully written, and `T: Plain` accepts any bits.
    Ok(unsafe { v.assume_init() })
}

/// Reads a property that is an array of `T`.
pub(crate) fn get_vec<T: Plain>(
    object: u32,
    a: ca::AudioObjectPropertyAddress,
) -> Result<Vec<T>, Error> {
    let bytes = size(object, &a)?;
    // SAFETY: `T: Plain` is valid for any bits, zeros included.
    let zero = unsafe { std::mem::zeroed::<T>() };
    let mut v = vec![zero; bytes as usize / size_of::<T>()];
    if v.is_empty() {
        return Ok(v);
    }
    let mut n = (v.len() * size_of::<T>()) as u32;
    // SAFETY: `v` holds `n` bytes; `a` and `n` outlive the call.
    let st = unsafe {
        ca::AudioObjectGetPropertyData(
            object,
            NonNull::from(&a),
            0,
            std::ptr::null(),
            NonNull::from(&mut n),
            NonNull::new_unchecked(v.as_mut_ptr()).cast(),
        )
    };
    check("AudioObjectGetPropertyData", &a, st)?;
    v.truncate(n as usize / size_of::<T>());
    Ok(v)
}

/// The channels of each buffer of an `AudioBufferList` property
/// (`kAudioDevicePropertyStreamConfiguration`).
pub(crate) fn get_buffer_channels(
    object: u32,
    a: ca::AudioObjectPropertyAddress,
) -> Result<Vec<u32>, Error> {
    use objc2_core_audio_types::{AudioBuffer, AudioBufferList};
    let bytes = size(object, &a)? as usize;
    // u64 words keep the list's pointers aligned.
    let mut words = vec![0u64; bytes.div_ceil(8).max(1)];
    let mut n = bytes as u32;
    // SAFETY: `words` holds at least `n` bytes; `a` and `n` outlive the call.
    let st = unsafe {
        ca::AudioObjectGetPropertyData(
            object,
            NonNull::from(&a),
            0,
            std::ptr::null(),
            NonNull::from(&mut n),
            NonNull::new_unchecked(words.as_mut_ptr()).cast(),
        )
    };
    check("AudioObjectGetPropertyData", &a, st)?;
    let n = n as usize;
    if n < size_of::<u32>() {
        return Ok(Vec::new());
    }
    let list = words.as_ptr().cast::<AudioBufferList>();
    // SAFETY: the HAL wrote at least the count; `list` is aligned for the struct.
    let count = unsafe { (*list).mNumberBuffers } as usize;
    // SAFETY: field projection on the valid pointer, no read.
    let first = unsafe { std::ptr::addr_of!((*list).mBuffers) }.cast::<AudioBuffer>();
    let offset = first as usize - list as usize;
    if offset + count * size_of::<AudioBuffer>() > n {
        return Err(Error::Unsupported(
            "a stream configuration shorter than its count",
        ));
    }
    Ok((0..count)
        // SAFETY: within the `n` bytes the HAL wrote, checked above.
        .map(|i| unsafe { (*first.add(i)).mNumberChannels })
        .collect())
}

/// Reads a `CFString` property and releases it.
pub(crate) fn get_string(object: u32, a: ca::AudioObjectPropertyAddress) -> Result<String, Error> {
    let mut p: *mut CFString = std::ptr::null_mut();
    let mut n = size_of::<*mut CFString>() as u32;
    // SAFETY: `p` holds a pointer; `a` and `n` outlive the call.
    let st = unsafe {
        ca::AudioObjectGetPropertyData(
            object,
            NonNull::from(&a),
            0,
            std::ptr::null(),
            NonNull::from(&mut n),
            NonNull::from(&mut p).cast(),
        )
    };
    check("AudioObjectGetPropertyData", &a, st)?;
    owned_string(p)
}

fn owned_string(p: *mut CFString) -> Result<String, Error> {
    let Some(p) = NonNull::new(p) else {
        return Err(Error::NotFound);
    };
    // SAFETY: the HAL returns CFString properties retained for the caller, who releases
    // them: `CFRetained` takes that reference and releases it on drop.
    let s = unsafe { CFRetained::from_raw(p) };
    Ok(s.to_string())
}

/// Writes a property.
pub(crate) fn set<T: Plain>(
    object: u32,
    a: ca::AudioObjectPropertyAddress,
    value: &T,
) -> Result<(), Error> {
    // SAFETY: `value` holds `size_of::<T>()` bytes and outlives the call.
    let st = unsafe {
        ca::AudioObjectSetPropertyData(
            object,
            NonNull::from(&a),
            0,
            std::ptr::null(),
            size_of::<T>() as u32,
            NonNull::from(value).cast(),
        )
    };
    check("AudioObjectSetPropertyData", &a, st)
}

/// `AudioHardwareIOProcStreamUsage` with room for every stream a [`StreamSet`] names; the
/// HAL reads `mNumberStreams` entries of the trailing array.
#[repr(C)]
struct StreamUsage {
    proc: *mut c_void,
    count: u32,
    on: [u32; StreamSet::MAX],
}

/// `kAudioDevicePropertyIOProcStreamUsage`: which of the device's `count` streams in
/// `scope` the IOProc `proc` is served. Set between creating the IOProc and starting it.
pub(crate) fn set_stream_usage(
    device: u32,
    scope: u32,
    proc: NonNull<c_void>,
    count: usize,
    used: StreamSet,
) -> Result<(), Error> {
    let count = count.min(StreamSet::MAX);
    let mut u = StreamUsage {
        proc: proc.as_ptr(),
        count: count as u32,
        on: [0; StreamSet::MAX],
    };
    for (i, on) in u.on.iter_mut().enumerate().take(count) {
        *on = u32::from(used.contains(i));
    }
    let a = addr(ca::kAudioDevicePropertyIOProcStreamUsage, scope, MAIN);
    let bytes = std::mem::offset_of!(StreamUsage, on) + count * size_of::<u32>();
    // SAFETY: `u` holds at least `bytes` bytes laid out as the HAL's variable-length
    // struct, and outlives the call.
    let st = unsafe {
        ca::AudioObjectSetPropertyData(
            device,
            NonNull::from(&a),
            0,
            std::ptr::null(),
            bytes as u32,
            NonNull::from(&u).cast(),
        )
    };
    check("AudioObjectSetPropertyData", &a, st)
}

/// `kAudioHardwarePropertyTranslateUIDToDevice`: the object of the device with `uid`, or
/// `None` when no such device is there.
pub(crate) fn device_for_uid(uid: &str) -> Result<Option<u32>, Error> {
    let a = global(ca::kAudioHardwarePropertyTranslateUIDToDevice);
    let cf = CFString::from_str(uid);
    let q: *const CFString = &*cf;
    let mut id = 0u32;
    let mut n = size_of::<u32>() as u32;
    // SAFETY: the qualifier is a CFStringRef that `cf` keeps alive; `id`, `a` and `n`
    // outlive the call.
    let st = unsafe {
        ca::AudioObjectGetPropertyData(
            SYSTEM,
            NonNull::from(&a),
            size_of::<*const CFString>() as u32,
            (&raw const q).cast::<c_void>(),
            NonNull::from(&mut n),
            NonNull::from(&mut id).cast(),
        )
    };
    check("AudioObjectGetPropertyData", &a, st)?;
    // kAudioObjectUnknown (0) when no device has the UID.
    Ok((id != 0).then_some(id))
}

/// `kAudioDevicePropertyClockSourceNameForIDCFString` for the source `id`.
pub(crate) fn clock_source_name(device: u32, scope: u32, id: u32) -> Result<String, Error> {
    let a = addr(
        ca::kAudioDevicePropertyClockSourceNameForIDCFString,
        scope,
        MAIN,
    );
    let mut input = id;
    let mut out: *mut CFString = std::ptr::null_mut();
    let mut t = AudioValueTranslation {
        mInputData: NonNull::from(&mut input).cast(),
        mInputDataSize: size_of::<u32>() as u32,
        mOutputData: NonNull::from(&mut out).cast(),
        mOutputDataSize: size_of::<*mut CFString>() as u32,
    };
    let mut n = size_of::<AudioValueTranslation>() as u32;
    // SAFETY: the translation's pointers name `input` and `out`, which outlive the call,
    // as do `t`, `a` and `n`.
    let st = unsafe {
        ca::AudioObjectGetPropertyData(
            device,
            NonNull::from(&a),
            0,
            std::ptr::null(),
            NonNull::from(&mut n),
            NonNull::from(&mut t).cast(),
        )
    };
    check("AudioObjectGetPropertyData", &a, st)?;
    owned_string(out)
}

/// Ticks per second of the host clock (`mHostTime`).
pub(crate) fn host_ticks_per_second() -> f64 {
    // SAFETY: a plain query with no arguments.
    unsafe { ca::AudioGetHostClockFrequency() }
}
