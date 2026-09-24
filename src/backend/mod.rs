//! Device communication backend.
//!
//! Every skill that talks to the iPhone goes through the traits below. The
//! Windows implementation uses Apple's MobileDevice / AirTrafficHost libraries
//! (`src/backend/windows`), the Linux implementation uses libimobiledevice
//! (`src/backend/linux`).

use std::time::Duration;

use anyhow::Result;

use crate::device::DeviceTransport;

#[cfg(unix)]
mod linux;
#[cfg(unix)]
use linux::MessageHandle as AtMessageHandle;
#[cfg(unix)]
pub use linux::{
    list_usbmux_devices, open_air_traffic, open_session, random_bytes, read_device_identity,
    verify_support,
};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows::MessageHandle as AtMessageHandle;
#[cfg(windows)]
pub use windows::{
    list_usbmux_devices, open_air_traffic, open_session, random_bytes, read_device_identity,
    verify_support,
};

/// Result of a timed receive on a service connection.
pub enum Received {
    /// The peer closed the connection.
    Closed,
    /// No data arrived within the timeout.
    Idle,
    /// `len` bytes were written into the caller's buffer.
    Data { len: usize },
}

/// A byte stream to one service running on the device.
pub trait ServiceConnection {
    /// Sends the whole buffer or fails.
    fn send_all(&self, data: &[u8]) -> Result<()>;

    /// Receives up to `buffer.len()` bytes, giving up after `timeout`.
    fn receive(&self, buffer: &mut [u8], timeout: Duration) -> Result<Received>;

    /// Sends one property list as a single framed message.
    fn send_plist_message(&self, message: &plist::Value) -> Result<()>;

    /// Receives one framed property list message, or fails on timeout.
    fn receive_plist_message(&self, timeout: Duration) -> Result<Option<plist::Value>>;
}

/// A paired device session that can start services on the device.
///
/// The session owns the device connection, so it must outlive every service
/// handle started from it.
pub trait DeviceSession {
    fn udid(&self) -> &str;

    fn transport(&self) -> DeviceTransport;

    fn start_service(&self, service_name: &str) -> Result<Box<dyn ServiceConnection + '_>>;

    fn open_afc(&self) -> Result<Box<dyn Afc + '_>>;
}

/// File access inside the device's media partition.
pub trait Afc {
    fn exists(&self, path: &str) -> bool;

    fn read_file(&self, path: &str) -> Result<Vec<u8>>;

    fn write_file(&self, path: &str, data: &[u8]) -> Result<()>;

    fn make_directory(&self, path: &str) -> Result<()>;

    fn remove_path(&self, path: &str) -> Result<()>;

    fn list_directory(&self, path: &str) -> Result<Vec<String>>;

    fn make_directory_recursive(&self, path: &str) -> Result<()> {
        let mut current = String::new();
        for part in path.split('/') {
            if part.is_empty() {
                continue;
            }
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(part);
            self.make_directory(&current)?;
        }
        Ok(())
    }

    fn remove_tree(&self, path: &str) -> Result<()> {
        self.remove_tree_internal(path, 0)
    }

    fn remove_tree_internal(&self, path: &str, depth: usize) -> Result<()> {
        if depth > 32 || !self.exists(path) {
            return Ok(());
        }

        if let Ok(children) = self.list_directory(path) {
            for child in children {
                let child_path = format!("{}/{}", path.trim_end_matches('/'), child);
                self.remove_tree_internal(&child_path, depth + 1)?;
            }
        }

        self.remove_path(path)
    }
}

/// One AirTraffic message: a name plus the parameters the device attached to it.
pub struct AtMessage {
    pub name: String,
    handle: AtMessageHandle,
}

impl AtMessage {
    pub(crate) fn new(name: String, handle: AtMessageHandle) -> Self {
        Self { name, handle }
    }

    /// Reads one parameter carried by the message.
    pub fn param(&self, key: &str) -> Option<plist::Value> {
        self.handle.param(key)
    }
}

/// The AirTraffic (`com.apple.atc`) host connection that drives the Books sync.
pub trait AtConnection {
    /// Returns `None` when no message arrived before `timeout` elapsed.
    fn read_message(&self, timeout: Duration) -> Result<Option<AtMessage>>;

    /// Answers a `Ping`. The device drops the connection when a `Ping` goes
    /// unanswered, so every read loop must reply.
    fn send_pong(&self) -> Result<()>;

    fn send_host_info(&self, host_info: &plist::Value) -> Result<()>;

    fn send_sync_request(
        &self,
        dataclasses: &plist::Value,
        anchors: &plist::Value,
        host_info: &plist::Value,
    ) -> Result<()>;

    fn send_metadata_sync_finished(
        &self,
        sync_types: &plist::Value,
        anchors: &plist::Value,
    ) -> Result<()>;

    fn send_asset_completed(&self, asset_id: &str, dataclass: &str, destination: &str) -> Result<()>;
}

/// Human readable device properties read through lockdown.
pub struct DeviceIdentity {
    pub name: String,
    pub product_type: String,
    pub ios_version: String,
    pub build_version: String,
}
