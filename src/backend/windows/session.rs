//! Paired device sessions and service connections over Apple Mobile Device Support.

use std::ptr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::backend::{Afc, DeviceIdentity, DeviceSession, Received, ServiceConnection};
use crate::device::{DeviceTransport, UsbmuxDeviceEntry};

use super::afc::WindowsAfc;
use super::ffi::{
    AMDeviceRef, AMDServiceConnectionRef, AppleLibraries, K_CFPROPERTY_LIST_BINARY_FORMAT_V1_0,
    get_apple_libraries,
};
use super::misc::set_receive_timeout;

pub fn open_session(entry: &UsbmuxDeviceEntry) -> Result<Box<dyn DeviceSession>> {
    Ok(Box::new(WindowsDeviceSession::connect(entry)?))
}

pub fn read_device_identity(entry: &UsbmuxDeviceEntry) -> Result<Option<DeviceIdentity>> {
    let libs = get_apple_libraries()?;
    let cf_props = libs.create_cf_plist_from_bytes(&entry.properties_plist)?;

    let mut identity = DeviceIdentity {
        name: "iPhone".to_string(),
        product_type: "iPhone".to_string(),
        ios_version: "Unknown".to_string(),
        build_version: "Unknown".to_string(),
    };

    unsafe {
        let device = (libs.am_device_create_from_properties)(cf_props.raw);
        if device.is_null() {
            return Ok(None);
        }

        if (libs.am_device_connect)(device) == 0 {
            let _ = (libs.am_device_validate_pairing)(device);
            identity.name = device_value(&libs, device, "DeviceName").unwrap_or(identity.name);
            identity.product_type =
                device_value(&libs, device, "ProductType").unwrap_or(identity.product_type);
            identity.ios_version =
                device_value(&libs, device, "ProductVersion").unwrap_or(identity.ios_version);
            identity.build_version =
                device_value(&libs, device, "BuildVersion").unwrap_or(identity.build_version);
            (libs.am_device_disconnect)(device);
        }

        (libs.cf_release)(device);
    }

    Ok(Some(identity))
}

fn device_value(libs: &AppleLibraries, device: AMDeviceRef, key: &str) -> Option<String> {
    let cf_key = libs.create_cf_string(key).ok()?;
    unsafe {
        let value = (libs.am_device_copy_value)(device, ptr::null(), cf_key.raw);
        if value.is_null() {
            return None;
        }
        let text = libs.to_rust_string(value);
        (libs.cf_release)(value);
        Some(text)
    }
}

pub(super) struct WindowsDeviceSession {
    libs: Arc<AppleLibraries>,
    device: AMDeviceRef,
    udid: String,
    transport: DeviceTransport,
    connected: bool,
    session_started: bool,
}

impl Drop for WindowsDeviceSession {
    fn drop(&mut self) {
        unsafe {
            if self.session_started {
                (self.libs.am_device_stop_session)(self.device);
            }
            if self.connected {
                (self.libs.am_device_disconnect)(self.device);
            }
            if !self.device.is_null() {
                (self.libs.cf_release)(self.device);
            }
        }
    }
}

impl WindowsDeviceSession {
    fn connect(entry: &UsbmuxDeviceEntry) -> Result<Self> {
        let libs = get_apple_libraries()?;
        let transport = entry.transport;
        let udid = entry.udid.clone();
        let cf_props = libs.create_cf_plist_from_bytes(&entry.properties_plist)?;

        unsafe {
            let device = (libs.am_device_create_from_properties)(cf_props.raw);
            if device.is_null() {
                bail!("AMDeviceCreateFromProperties failed");
            }

            let connect_status = (libs.am_device_connect)(device);
            if connect_status != 0 {
                (libs.cf_release)(device);
                bail!("AMDeviceConnect failed with code {}", connect_status);
            }

            if (libs.am_device_is_paired)(device) == 0 {
                if transport == DeviceTransport::Wifi {
                    (libs.am_device_disconnect)(device);
                    (libs.cf_release)(device);
                    bail!("WiFi device is not paired. Connect it over USB once and trust this computer first");
                }
                (libs.am_device_pair)(device);
            }

            let mut validate_status = (libs.am_device_validate_pairing)(device);
            if validate_status != 0 && transport == DeviceTransport::Usb {
                (libs.am_device_pair)(device);
                validate_status = (libs.am_device_validate_pairing)(device);
            }
            if validate_status != 0 {
                (libs.am_device_disconnect)(device);
                (libs.cf_release)(device);
                bail!(
                    "AMDeviceValidatePairing failed with code {} over {}. Unlock the iPhone; WiFi connections must be trusted over USB first.",
                    validate_status,
                    transport.label()
                );
            }

            let session_status = (libs.am_device_start_session)(device);
            if session_status != 0 {
                (libs.am_device_disconnect)(device);
                (libs.cf_release)(device);
                bail!("AMDeviceStartSession failed with code {}", session_status);
            }

            Ok(Self {
                libs,
                device,
                udid,
                transport,
                connected: true,
                session_started: true,
            })
        }
    }

    pub(super) fn libs(&self) -> &Arc<AppleLibraries> {
        &self.libs
    }

    pub(super) fn service_connection(&self, service_name: &str) -> Result<AMDServiceConnectionRef> {
        let cf_name = self.libs.create_cf_string(service_name)?;
        let mut service_conn: AMDServiceConnectionRef = ptr::null_mut();
        let status = unsafe {
            (self.libs.am_device_secure_start_service)(
                self.device,
                cf_name.raw,
                ptr::null(),
                &mut service_conn,
            )
        };
        if status != 0 || service_conn.is_null() {
            bail!("AMDeviceSecureStartService('{}') failed with code {}", service_name, status);
        }
        Ok(service_conn)
    }
}

impl DeviceSession for WindowsDeviceSession {
    fn udid(&self) -> &str {
        &self.udid
    }

    fn transport(&self) -> DeviceTransport {
        self.transport
    }

    fn start_service(&self, service_name: &str) -> Result<Box<dyn ServiceConnection + '_>> {
        let conn = self.service_connection(service_name)?;
        Ok(Box::new(WindowsServiceConnection {
            libs: Arc::clone(&self.libs),
            conn,
        }))
    }

    fn open_afc(&self) -> Result<Box<dyn Afc + '_>> {
        Ok(Box::new(WindowsAfc::open(self)?))
    }
}

pub(super) struct WindowsServiceConnection {
    libs: Arc<AppleLibraries>,
    conn: AMDServiceConnectionRef,
}

impl Drop for WindowsServiceConnection {
    fn drop(&mut self) {
        if !self.conn.is_null() {
            unsafe {
                (self.libs.amd_service_connection_invalidate)(self.conn);
            }
        }
    }
}

impl WindowsServiceConnection {
    fn socket(&self) -> Result<usize> {
        let socket = unsafe { (self.libs.amd_service_connection_get_socket)(self.conn) };
        if socket <= 0 {
            bail!("Service connection has no socket");
        }
        Ok(socket as usize)
    }
}

impl ServiceConnection for WindowsServiceConnection {
    fn send_all(&self, data: &[u8]) -> Result<()> {
        let mut sent = 0;
        while sent < data.len() {
            let chunk = std::cmp::min(65536, data.len() - sent);
            let written = unsafe {
                (self.libs.amd_service_connection_send)(self.conn, data.as_ptr().add(sent), chunk)
            };
            if written <= 0 {
                bail!("AMDServiceConnectionSend failed after {} bytes", sent);
            }
            sent += written as usize;
        }
        Ok(())
    }

    fn receive(&self, buffer: &mut [u8], timeout: Duration) -> Result<Received> {
        set_receive_timeout(self.socket()?, timeout);
        let read = unsafe {
            (self.libs.amd_service_connection_receive)(self.conn, buffer.as_mut_ptr(), buffer.len())
        };
        Ok(match read {
            0 => Received::Closed,
            n if n < 0 => Received::Idle,
            n => Received::Data { len: n as usize },
        })
    }

    fn send_plist_message(&self, message: &plist::Value) -> Result<()> {
        let mut bytes = Vec::new();
        plist::to_writer_binary(&mut bytes, message)
            .context("Failed to serialize service message")?;
        let cf_message = self.libs.create_cf_plist_from_bytes(&bytes)?;

        let status = unsafe {
            (self.libs.amd_service_connection_send_message)(
                self.conn,
                cf_message.raw,
                K_CFPROPERTY_LIST_BINARY_FORMAT_V1_0,
            )
        };
        if status != 0 {
            bail!("AMDServiceConnectionSendMessage failed with code {}", status);
        }
        Ok(())
    }

    fn receive_plist_message(&self, timeout: Duration) -> Result<Option<plist::Value>> {
        set_receive_timeout(self.socket()?, timeout);

        let mut response: *const std::ffi::c_void = ptr::null();
        let mut format: isize = 0;
        let status = unsafe {
            (self.libs.amd_service_connection_receive_message)(self.conn, &mut response, &mut format)
        };

        if status != 0 {
            if !response.is_null() {
                unsafe { (self.libs.cf_release)(response) };
            }
            bail!("AMDServiceConnectionReceiveMessage failed with code {}", status);
        }
        if response.is_null() {
            return Ok(None);
        }

        let bytes = self.libs.cf_plist_to_bytes(response);
        unsafe { (self.libs.cf_release)(response) };

        let bytes = bytes.context("Failed to read the device service message")?;
        let value = plist::Value::from_reader(std::io::Cursor::new(bytes))
            .context("Failed to parse the device service message")?;
        Ok(Some(value))
    }
}
