//! The HAL's system object: the device list, defaults, lookup by UID.

use objc2_core_audio as ca;

use crate::device::Device;
use crate::listener::Listener;
use crate::property::{self, SYSTEM, global};
use crate::types::{Error, Scope, SystemProperty};

/// The HAL's system object.
#[derive(Clone, Copy, Debug)]
pub struct System;

fn selector(p: SystemProperty) -> u32 {
    match p {
        SystemProperty::Devices => ca::kAudioHardwarePropertyDevices,
        SystemProperty::DefaultInput => ca::kAudioHardwarePropertyDefaultInputDevice,
        SystemProperty::DefaultOutput => ca::kAudioHardwarePropertyDefaultOutputDevice,
        SystemProperty::ServiceRestarted => ca::kAudioHardwarePropertyServiceRestarted,
    }
}

impl System {
    /// Every device the HAL lists.
    pub fn devices() -> Result<Vec<Device>, Error> {
        Ok(
            property::get_vec::<u32>(SYSTEM, global(ca::kAudioHardwarePropertyDevices))?
                .into_iter()
                .map(Device::from_id)
                .collect(),
        )
    }

    /// The system's default device for `scope`, if any.
    pub fn default_device(scope: Scope) -> Result<Option<Device>, Error> {
        let sel = match scope {
            Scope::Input => ca::kAudioHardwarePropertyDefaultInputDevice,
            Scope::Output => ca::kAudioHardwarePropertyDefaultOutputDevice,
        };
        let id = property::get::<u32>(SYSTEM, global(sel))?;
        Ok((id != 0).then(|| Device::from_id(id)))
    }

    /// The device whose UID is `uid`, if it is there.
    pub fn device_by_uid(uid: &str) -> Result<Option<Device>, Error> {
        Ok(property::device_for_uid(uid)?.map(Device::from_id))
    }

    /// Calls `f` on the HAL's notification thread whenever one of `props` changes. `f`
    /// must not block.
    pub fn listen(
        props: &[SystemProperty],
        f: impl Fn(SystemProperty) + Send + Sync + 'static,
    ) -> Result<Listener, Error> {
        let props: Vec<SystemProperty> = props.to_vec();
        let addrs = props.iter().map(|&p| global(selector(p))).collect();
        Listener::register(
            SYSTEM,
            addrs,
            Box::new(move |sel| {
                if let Some(&p) = props.iter().find(|&&p| selector(p) == sel) {
                    f(p);
                }
            }),
        )
    }
}
