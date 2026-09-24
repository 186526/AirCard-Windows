//! Device enumeration over usbmuxd.

use std::ffi::CStr;
use std::ptr;

use anyhow::{Result, bail};

use crate::device::{DeviceTransport, UsbmuxDeviceEntry};

use super::ffi::{CONNECTION_NETWORK, CONNECTION_USBMUXD, idevice_info_t, libraries};

/// Lists connected devices in the same shape the Windows backend produces.
///
/// `properties_plist` stays empty: on Linux the device is reached through
/// libimobiledevice, which looks the device up by UDID rather than from a
/// property list.
pub fn list_usbmux_devices() -> Result<Vec<UsbmuxDeviceEntry>> {
    let libraries = libraries()?;

    let mut devices: *mut idevice_info_t = ptr::null_mut();
    let mut count: i32 = 0;
    let status = unsafe { (libraries.idevice_get_device_list_extended)(&mut devices, &mut count) };
    if status != 0 {
        // `-3` (IDEVICE_E_NO_DEVICE) is what libimobiledevice reports when it
        // cannot reach the usbmuxd socket at all, which on a udev-activated
        // install simply means no iPhone is plugged in.
        bail!(
            "Could not reach usbmuxd (code {}). Connect and unlock an iPhone; usbmuxd starts automatically when it is plugged in. Otherwise check: systemctl status usbmuxd",
            status
        );
    }
    if devices.is_null() {
        return Ok(Vec::new());
    }
    // The library allocates the array even when no device is attached
    // (`count == 0`), so it must always be released.
    if count <= 0 {
        unsafe {
            (libraries.idevice_device_list_extended_free)(devices);
        }
        return Ok(Vec::new());
    }

    let mut entries = Vec::with_capacity(count as usize);
    for index in 0..count as isize {
        let device = unsafe { *devices.offset(index) };
        if device.is_null() {
            continue;
        }

        let udid = unsafe {
            let raw = (*device).udid;
            if raw.is_null() {
                continue;
            }
            CStr::from_ptr(raw).to_string_lossy().into_owned()
        };

        entries.push(UsbmuxDeviceEntry {
            udid,
            transport: transport_of(unsafe { (*device).conn_type }),
            properties_plist: Vec::new(),
        });
    }

    unsafe {
        (libraries.idevice_device_list_extended_free)(devices);
    }

    Ok(entries)
}

fn transport_of(connection_type: i32) -> DeviceTransport {
    match connection_type {
        CONNECTION_NETWORK => DeviceTransport::Wifi,
        CONNECTION_USBMUXD => DeviceTransport::Usb,
        _ => DeviceTransport::Other,
    }
}

