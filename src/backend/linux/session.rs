//! Paired device sessions and service connections over libimobiledevice.
//!
//! Every session reaches the device through an existing usbmuxd pairing record:
//! AirCard starts a lockdown session with the stored `HostID` and never calls
//! `lockdownd_pair`. See `pairing.rs` for why.

use std::ffi::{CString, c_char};
use std::ptr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::backend::{Afc, DeviceIdentity, DeviceSession, Received, ServiceConnection};
use crate::device::{DeviceTransport, UsbmuxDeviceEntry};

use super::afc::LinuxAfc;
use super::ffi::{
    IDEVICE_E_NOT_ENOUGH_DATA, IDEVICE_E_TIMEOUT, IDEVICE_LOOKUP_NETWORK, IDEVICE_LOOKUP_USBMUX,
    Libraries, afc_client_t, idevice_connection_t, idevice_t, libraries, lockdownd_client_t,
    lockdownd_service_descriptor_t, plist_t,
};
use super::pairing::host_id;
use super::plist_bridge::PlistHandle;

/// Longest framed message accepted from the device, in bytes.
const MAX_FRAME: usize = 64 * 1024 * 1024;

pub fn open_session(entry: &UsbmuxDeviceEntry) -> Result<Box<dyn DeviceSession>> {
    Ok(Box::new(LinuxDeviceSession::connect(entry)?))
}

/// Opens the AirTraffic service for a device that is already paired.
pub(super) fn open_paired_service(
    udid: &str,
    transport: DeviceTransport,
    service_name: &str,
) -> Result<LinuxServiceConnection> {
    let libraries = libraries()?;
    let handles = connect_handles(&libraries, udid, transport)?;
    start_service(&handles, service_name)
}

pub fn read_device_identity(entry: &UsbmuxDeviceEntry) -> Result<Option<DeviceIdentity>> {
    let libraries = libraries()?;
    let Ok(handles) = connect_handles(&libraries, &entry.udid, entry.transport) else {
        // An unpaired or locked device stays hidden instead of failing the scan.
        return Ok(None);
    };

    let identity = DeviceIdentity {
        name: lockdownd_string(&handles, "DeviceName").unwrap_or_else(|| "iPhone".to_string()),
        product_type: lockdownd_string(&handles, "ProductType")
            .unwrap_or_else(|| "iPhone".to_string()),
        ios_version: lockdownd_string(&handles, "ProductVersion")
            .unwrap_or_else(|| "Unknown".to_string()),
        build_version: lockdownd_string(&handles, "BuildVersion")
            .unwrap_or_else(|| "Unknown".to_string()),
    };

    Ok(Some(identity))
}

fn lockdownd_string(handles: &DeviceHandles, key: &str) -> Option<String> {
    let key = CString::new(key).ok()?;
    let mut value: plist_t = ptr::null_mut();
    let status = unsafe {
        (handles.libraries.lockdownd_get_value)(
            handles.lockdown,
            ptr::null(),
            key.as_ptr(),
            &mut value,
        )
    };
    if status != 0 || value.is_null() {
        return None;
    }

    PlistHandle::from_raw(Arc::clone(&handles.libraries), value)
        .to_value()
        .ok()?
        .as_string()
        .map(str::to_string)
}

/// The device handle and its lockdown client, shared by every service started
/// from the same device so they outlive the connection that uses them.
pub(super) struct DeviceHandles {
    libraries: Arc<Libraries>,
    device: idevice_t,
    lockdown: lockdownd_client_t,
}

impl Drop for DeviceHandles {
    fn drop(&mut self) {
        unsafe {
            if !self.lockdown.is_null() {
                (self.libraries.lockdownd_client_free)(self.lockdown);
            }
            if !self.device.is_null() {
                (self.libraries.idevice_free)(self.device);
            }
        }
    }
}

/// Opens the device plus a lockdown session for an already paired iPhone.
fn connect_handles(
    libraries: &Arc<Libraries>,
    udid: &str,
    transport: DeviceTransport,
) -> Result<Arc<DeviceHandles>> {
    let udid_c = CString::new(udid).context("Device UDID contains a null byte")?;
    let options = match transport {
        DeviceTransport::Wifi => IDEVICE_LOOKUP_NETWORK,
        _ => IDEVICE_LOOKUP_USBMUX,
    };

    let mut device: idevice_t = ptr::null_mut();
    let status =
        unsafe { (libraries.idevice_new_with_options)(&mut device, udid_c.as_ptr(), options) };
    if status != 0 || device.is_null() {
        bail!(
            "idevice_new_with_options failed for {} over {} with code {}",
            udid,
            transport.label(),
            status
        );
    }

    let label = c"AirCard";
    let mut lockdown: lockdownd_client_t = ptr::null_mut();
    let status = unsafe { (libraries.lockdownd_client_new)(device, &mut lockdown, label.as_ptr()) };
    if status != 0 || lockdown.is_null() {
        unsafe { (libraries.idevice_free)(device) };
        bail!("Could not open a lockdown client for {} (code {})", udid, status);
    }

    let handles = Arc::new(DeviceHandles {
        libraries: Arc::clone(libraries),
        device,
        lockdown,
    });

    let host_id = host_id(libraries, udid)?;
    let host_id_c = CString::new(host_id).context("The stored HostID contains a null byte")?;
    let status = unsafe {
        (libraries.lockdownd_start_session)(
            handles.lockdown,
            host_id_c.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if status != 0 {
        bail!(
            "Starting a lockdown session for {} failed with code {}. Unlock the iPhone and pair it again over USB.",
            udid,
            status
        );
    }

    Ok(handles)
}

/// Starts `service_name` and connects to it, enabling TLS when the device asks.
fn start_service(handles: &Arc<DeviceHandles>, service_name: &str) -> Result<LinuxServiceConnection> {
    let name = CString::new(service_name).context("Service name contains a null byte")?;
    let mut descriptor: lockdownd_service_descriptor_t = ptr::null_mut();
    let status = unsafe {
        (handles.libraries.lockdownd_start_service)(
            handles.lockdown,
            name.as_ptr(),
            &mut descriptor,
        )
    };
    if status != 0 || descriptor.is_null() {
        bail!(
            "lockdownd_start_service('{}') failed with code {}",
            service_name,
            status
        );
    }

    let port = unsafe { (*descriptor).port };
    let ssl_enabled = unsafe { (*descriptor).ssl_enabled } != 0;
    let result = connect_port(handles, service_name, port, ssl_enabled);
    unsafe { (handles.libraries.lockdownd_service_descriptor_free)(descriptor) };
    result
}

fn connect_port(
    handles: &Arc<DeviceHandles>,
    service_name: &str,
    port: u16,
    ssl_enabled: bool,
) -> Result<LinuxServiceConnection> {
    if port == 0 {
        bail!("No port was returned for service '{}'", service_name);
    }

    let mut connection: idevice_connection_t = ptr::null_mut();
    let status =
        unsafe { (handles.libraries.idevice_connect)(handles.device, port, &mut connection) };
    if status != 0 || connection.is_null() {
        bail!(
            "Connecting to service '{}' on port {} failed with code {}",
            service_name,
            port,
            status
        );
    }

    if ssl_enabled {
        let status = unsafe { (handles.libraries.idevice_connection_enable_ssl)(connection) };
        if status != 0 {
            unsafe { (handles.libraries.idevice_disconnect)(connection) };
            bail!(
                "Service '{}' requires TLS, which libimobiledevice refused with code {}",
                service_name,
                status
            );
        }
    }

    Ok(LinuxServiceConnection::new(
        Arc::clone(handles),
        connection,
    ))
}

pub struct LinuxDeviceSession {
    handles: Arc<DeviceHandles>,
    udid: String,
    transport: DeviceTransport,
}

impl LinuxDeviceSession {
    fn connect(entry: &UsbmuxDeviceEntry) -> Result<Self> {
        let libraries = libraries()?;
        Ok(Self {
            handles: connect_handles(&libraries, &entry.udid, entry.transport)?,
            udid: entry.udid.clone(),
            transport: entry.transport,
        })
    }
}

impl DeviceSession for LinuxDeviceSession {
    fn udid(&self) -> &str {
        &self.udid
    }

    fn transport(&self) -> DeviceTransport {
        self.transport
    }

    fn start_service(&self, service_name: &str) -> Result<Box<dyn ServiceConnection + '_>> {
        Ok(Box::new(start_service(&self.handles, service_name)?))
    }

    fn open_afc(&self) -> Result<Box<dyn Afc + '_>> {
        let descriptor = {
            let name = CString::new("com.apple.afc").context("Service name contains a null byte")?;
            let mut descriptor: lockdownd_service_descriptor_t = ptr::null_mut();
            let status = unsafe {
                (self.handles.libraries.lockdownd_start_service)(
                    self.handles.lockdown,
                    name.as_ptr(),
                    &mut descriptor,
                )
            };
            if status != 0 || descriptor.is_null() {
                bail!("lockdownd_start_service('com.apple.afc') failed with code {}", status);
            }
            descriptor
        };

        if unsafe { (*descriptor).ssl_enabled } != 0 {
            unsafe { (self.handles.libraries.lockdownd_service_descriptor_free)(descriptor) };
            bail!(
                "com.apple.afc requested TLS. libimobiledevice cannot report whether the AFC connection is encrypted, so AirCard refuses to continue."
            );
        }

        let mut client: afc_client_t = ptr::null_mut();
        let status = unsafe {
            (self.handles.libraries.afc_client_new)(self.handles.device, descriptor, &mut client)
        };
        unsafe { (self.handles.libraries.lockdownd_service_descriptor_free)(descriptor) };

        if status != 0 || client.is_null() {
            bail!("afc_client_new failed with code {}", status);
        }

        Ok(Box::new(LinuxAfc::new(
            Arc::clone(&self.handles.libraries),
            client,
        )))
    }
}

/// Opens the AirTraffic service for `udid` over `transport`.
///
/// The transport is passed in rather than re-derived, so the route the caller
/// validated with `ensure_transport_available` is the one actually used when a
/// device is reachable over both USB and WiFi.
pub fn open_air_traffic(
    udid: &str,
    transport: DeviceTransport,
) -> Result<Box<dyn crate::backend::AtConnection>> {
    let service = open_paired_service(udid, transport, "com.apple.atc")?;
    Ok(Box::new(super::at::LinuxAtConnection::new(service)))
}

/// A raw byte stream to one service on the device.
pub struct LinuxServiceConnection {
    handles: Arc<DeviceHandles>,
    connection: idevice_connection_t,
}

impl Drop for LinuxServiceConnection {
    fn drop(&mut self) {
        if !self.connection.is_null() {
            unsafe {
                (self.handles.libraries.idevice_disconnect)(self.connection);
            }
        }
    }
}

impl LinuxServiceConnection {
    pub(super) fn new(handles: Arc<DeviceHandles>, connection: idevice_connection_t) -> Self {
        Self {
            handles,
            connection,
        }
    }

    /// One receive call, returning the byte count and the library status code.
    fn receive_once(&self, buffer: &mut [u8], timeout: Duration) -> (usize, i32) {
        let mut received: u32 = 0;
        let status = unsafe {
            (self.handles.libraries.idevice_connection_receive_timeout)(
                self.connection,
                buffer.as_mut_ptr() as *mut c_char,
                buffer.len() as u32,
                &mut received,
                timeout.as_millis().min(u32::MAX as u128) as u32,
            )
        };
        (received as usize, status)
    }

    /// Reads exactly `buffer.len()` bytes for the start of a message.
    ///
    /// Returns `Idle` when nothing has arrived at all, which lets the caller
    /// retry the whole message without losing framing. Once the first byte has
    /// arrived the rest is read to completion, because abandoning a partially
    /// consumed frame would desynchronise the stream.
    pub(super) fn receive_exact(&self, buffer: &mut [u8], timeout: Duration) -> Result<Received> {
        self.read_fully(buffer, timeout, true)
    }

    /// Reads the rest of a message whose header has already been consumed.
    ///
    /// A timeout here cannot mean "no message": the header is gone, so the body
    /// must be drained to keep the stream aligned. Empty reads are retried up
    /// to `MAX_STALLED_READS` times before the message is declared dead.
    pub(super) fn receive_body(&self, buffer: &mut [u8], timeout: Duration) -> Result<Received> {
        self.read_fully(buffer, timeout, false)
    }

    fn read_fully(
        &self,
        buffer: &mut [u8],
        timeout: Duration,
        idle_when_empty: bool,
    ) -> Result<Received> {
        let mut filled = 0;
        let mut stalled = 0;
        while filled < buffer.len() {
            let (received, status) = self.receive_once(&mut buffer[filled..], timeout);
            if received > 0 {
                filled += received;
                stalled = 0;
                continue;
            }
            if filled == 0 && idle_when_empty {
                return Ok(if is_timeout(status) {
                    Received::Idle
                } else {
                    Received::Closed
                });
            }
            if !is_timeout(status) {
                if filled == 0 {
                    return Ok(Received::Closed);
                }
                bail!(
                    "The device stopped sending after {} of {} bytes",
                    filled,
                    buffer.len()
                );
            }
            // Mid-message, a timeout usually means a slow link rather than a
            // dead one, so retry before giving up on a frame already in flight.
            stalled += 1;
            if stalled > MAX_STALLED_READS {
                bail!(
                    "The device stopped sending after {} of {} bytes",
                    filled,
                    buffer.len()
                );
            }
        }
        Ok(Received::Data { len: filled })
    }
}

/// Consecutive empty reads tolerated in the middle of one message.
const MAX_STALLED_READS: u32 = 4;

/// Timeouts and empty partial reads both mean "nothing arrived yet".
fn is_timeout(status: i32) -> bool {
    matches!(status, IDEVICE_E_TIMEOUT | IDEVICE_E_NOT_ENOUGH_DATA)
}

impl ServiceConnection for LinuxServiceConnection {
    fn send_all(&self, data: &[u8]) -> Result<()> {
        let mut sent = 0;
        while sent < data.len() {
            let mut written: u32 = 0;
            let status = unsafe {
                (self.handles.libraries.idevice_connection_send)(
                    self.connection,
                    data[sent..].as_ptr() as *const c_char,
                    (data.len() - sent) as u32,
                    &mut written,
                )
            };
            if status != 0 || written == 0 {
                bail!(
                    "Sending to the device failed after {} bytes (code {})",
                    sent,
                    status
                );
            }
            sent += written as usize;
        }
        Ok(())
    }

    fn receive(&self, buffer: &mut [u8], timeout: Duration) -> Result<Received> {
        let (received, status) = self.receive_once(buffer, timeout);
        if received > 0 {
            return Ok(Received::Data { len: received });
        }

        Ok(if is_timeout(status) {
            Received::Idle
        } else {
            Received::Closed
        })
    }

    /// Sends one message in the four-byte big-endian framing that
    /// `com.apple.streaming_zip_conduit` expects.
    fn send_plist_message(&self, message: &plist::Value) -> Result<()> {
        let mut body = Vec::new();
        plist::to_writer_binary(&mut body, message)
            .context("Failed to encode the service message")?;

        let mut framed = Vec::with_capacity(body.len() + 4);
        framed.extend_from_slice(&(body.len() as u32).to_be_bytes());
        framed.extend_from_slice(&body);
        self.send_all(&framed)
    }

    /// Receives one message in the same framing; `None` means the call timed out.
    fn receive_plist_message(&self, timeout: Duration) -> Result<Option<plist::Value>> {
        let mut header = [0u8; 4];
        match self.receive_exact(&mut header, timeout)? {
            Received::Idle => return Ok(None),
            Received::Closed => bail!("The device closed the streaming zip connection"),
            Received::Data { .. } => {}
        }

        let length = u32::from_be_bytes(header) as usize;
        if length == 0 || length > MAX_FRAME {
            bail!("Device sent an implausible message length of {} bytes", length);
        }

        let mut body = vec![0u8; length];
        self.receive_body(&mut body, timeout)?;

        let value = plist::Value::from_reader(std::io::Cursor::new(body))
            .context("Failed to decode the service message")?;
        Ok(Some(value))
    }
}
