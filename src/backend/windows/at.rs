//! AirTraffic (`com.apple.atc`) host connection through AirTrafficHost.dll.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::backend::{AtConnection, AtMessage};
use crate::device::DeviceTransport;

use super::ffi::{
    ATHostConnectionRef, AppleLibraries, CFDictionaryRef, CFTypeGuard, CFTypeRef,
    get_apple_libraries,
};

/// A message returned by `ATHostConnectionReadMessage`.
///
/// The dictionary stays owned by AirTrafficHost until it is released here, and
/// parameters are read out of it through `ATCFMessageGetParam`.
pub struct MessageHandle {
    libs: Arc<AppleLibraries>,
    message: CFDictionaryRef,
}

impl Drop for MessageHandle {
    fn drop(&mut self) {
        if !self.message.is_null() {
            unsafe {
                (self.libs.cf_release)(self.message);
            }
        }
    }
}

impl MessageHandle {
    pub fn param(&self, key: &str) -> Option<plist::Value> {
        let cf_key = self.libs.create_cf_string(key).ok()?;
        unsafe {
            let value: CFTypeRef = (self.libs.at_cf_message_get_param)(self.message, cf_key.raw);
            if value.is_null() {
                return None;
            }
            let bytes = self.libs.cf_plist_to_bytes(value).ok()?;
            plist::Value::from_reader(std::io::Cursor::new(bytes)).ok()
        }
    }
}

pub struct WindowsAtConnection {
    libs: Arc<AppleLibraries>,
    conn: ATHostConnectionRef,
}

impl Drop for WindowsAtConnection {
    fn drop(&mut self) {
        if !self.conn.is_null() {
            unsafe {
                (self.libs.at_host_connection_release)(self.conn);
            }
        }
    }
}

/// Opens the AirTraffic host connection through `AirTrafficHost.dll`.
///
/// The Windows library owns its own device handle and selects its route from
/// the UDID alone, so `transport` is only part of the shared signature.
pub fn open_air_traffic(
    udid: &str,
    _transport: DeviceTransport,
) -> Result<Box<dyn AtConnection>> {
    let libs = get_apple_libraries()?;
    let cf_udid = libs.create_cf_string(udid)?;
    let conn = unsafe { (libs.at_host_connection_create)(cf_udid.raw) };
    if conn.is_null() {
        bail!("ATHostConnectionCreate failed for UDID: {}", udid);
    }

    Ok(Box::new(WindowsAtConnection { libs, conn }))
}

impl WindowsAtConnection {
    fn create_plist(&self, value: &plist::Value) -> Result<CFTypeGuard> {
        let mut bytes = Vec::new();
        plist::to_writer_binary(&mut bytes, value)
            .context("Failed to serialize AirTraffic message")?;
        self.libs.create_cf_plist_from_bytes(&bytes)
    }
}

impl AtConnection for WindowsAtConnection {
    /// `timeout` is unused: AirTrafficHost returns a null message when nothing
    /// is pending, and the caller's retry loop provides the pacing.
    fn read_message(&self, _timeout: Duration) -> Result<Option<AtMessage>> {
        let message = unsafe { (self.libs.at_host_connection_read_message)(self.conn) };
        if message.is_null() {
            return Ok(None);
        }

        let name_ref = unsafe { (self.libs.at_cf_message_get_name)(message) };
        let name = self.libs.to_rust_string(name_ref);
        Ok(Some(AtMessage::new(
            name,
            MessageHandle {
                libs: Arc::clone(&self.libs),
                message,
            },
        )))
    }

    /// Windows keeps Ping/Pong inside `AirTrafficHost.dll`: the sync loop never
    /// observes a `Ping` on this backend, so there is nothing to answer.
    fn send_pong(&self) -> Result<()> {
        Ok(())
    }

    fn send_host_info(&self, host_info: &plist::Value) -> Result<()> {
        let cf_host_info = self.create_plist(host_info)?;
        unsafe {
            (self.libs.at_host_connection_send_host_info)(self.conn, cf_host_info.raw);
        }
        Ok(())
    }

    fn send_sync_request(
        &self,
        dataclasses: &plist::Value,
        anchors: &plist::Value,
        host_info: &plist::Value,
    ) -> Result<()> {
        let cf_dataclasses = self.create_plist(dataclasses)?;
        let cf_anchors = self.create_plist(anchors)?;
        let cf_host_info = self.create_plist(host_info)?;
        unsafe {
            (self.libs.at_host_connection_send_sync_request)(
                self.conn,
                cf_dataclasses.raw,
                cf_anchors.raw,
                cf_host_info.raw,
            );
        }
        Ok(())
    }

    fn send_metadata_sync_finished(
        &self,
        sync_types: &plist::Value,
        anchors: &plist::Value,
    ) -> Result<()> {
        let cf_sync_types = self.create_plist(sync_types)?;
        let cf_anchors = self.create_plist(anchors)?;
        unsafe {
            (self.libs.at_host_connection_send_metadata_sync_finished)(
                self.conn,
                cf_sync_types.raw,
                cf_anchors.raw,
            );
        }
        Ok(())
    }

    fn send_asset_completed(
        &self,
        asset_id: &str,
        dataclass: &str,
        destination: &str,
    ) -> Result<()> {
        let cf_asset_id = self.libs.create_cf_string(asset_id)?;
        let cf_dataclass = self.libs.create_cf_string(dataclass)?;
        let cf_destination = self.libs.create_cf_string(destination)?;
        unsafe {
            (self.libs.at_host_connection_send_asset_completed)(
                self.conn,
                cf_asset_id.raw,
                cf_dataclass.raw,
                cf_destination.raw,
            );
        }
        Ok(())
    }
}
