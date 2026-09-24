//! usbmux device enumeration over TCP `127.0.0.1:27015`.
//!
//! On Windows usbmuxd listens on a TCP port, so AirCard speaks the usbmux
//! protocol directly and keeps each device's raw property list: it is the only
//! input `AMDeviceCreateFromProperties` accepts.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::device::{DeviceTransport, UsbmuxDeviceEntry};

/// Longest usbmux response accepted, in bytes.
const MAX_RESPONSE: usize = 16 * 1024 * 1024;

pub fn list_usbmux_devices() -> Result<Vec<UsbmuxDeviceEntry>> {
    let addr: SocketAddr = "127.0.0.1:27015".parse().unwrap();
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .context("Could not connect to the device multiplexer (usbmuxd) at 127.0.0.1:27015. Please ensure it is installed and running.")?;

    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;

    let mut request = HashMap::new();
    request.insert(
        "MessageType".to_string(),
        plist::Value::String("ListDevices".to_string()),
    );
    request.insert(
        "ClientVersionString".to_string(),
        plist::Value::String("aircard".to_string()),
    );
    request.insert(
        "ProgName".to_string(),
        plist::Value::String("aircard".to_string()),
    );

    let mut plist_bytes = Vec::new();
    plist::to_writer_xml(
        &mut plist_bytes,
        &plist::Value::Dictionary(request.into_iter().collect()),
    )
    .context("Failed to serialize ListDevices request")?;

    let length = (plist_bytes.len() + 16) as u32;
    let mut header = Vec::with_capacity(16);
    header.extend_from_slice(&length.to_le_bytes());
    header.extend_from_slice(&1u32.to_le_bytes()); // version
    header.extend_from_slice(&8u32.to_le_bytes()); // PLIST message type
    header.extend_from_slice(&1u32.to_le_bytes()); // tag

    stream.write_all(&header)?;
    stream.write_all(&plist_bytes)?;
    stream.flush()?;

    let mut response_header = [0u8; 16];
    stream.read_exact(&mut response_header)?;

    let response_len = u32::from_le_bytes([
        response_header[0],
        response_header[1],
        response_header[2],
        response_header[3],
    ]) as usize;
    if response_len < 16 || response_len > MAX_RESPONSE {
        bail!("Invalid usbmux response length: {}", response_len);
    }

    let mut payload = vec![0u8; response_len - 16];
    stream.read_exact(&mut payload)?;

    let value = plist::Value::from_reader(std::io::Cursor::new(payload))
        .context("Failed to parse usbmux ListDevices response plist")?;
    let root = value
        .as_dictionary()
        .context("Expected dictionary in usbmux response")?;
    let device_list = root
        .get("DeviceList")
        .and_then(|value| value.as_array())
        .context("Expected DeviceList array in usbmux response")?;

    let mut entries = Vec::new();
    for entry in device_list {
        let Some(device) = entry.as_dictionary() else {
            continue;
        };
        let Some(properties) = device.get("Properties") else {
            continue;
        };
        let Some(properties_dict) = properties.as_dictionary() else {
            continue;
        };

        let udid = properties_dict
            .get("SerialNumber")
            .and_then(|value| value.as_string())
            .unwrap_or_default()
            .to_string();
        let connection_type = properties_dict
            .get("ConnectionType")
            .and_then(|value| value.as_string())
            .unwrap_or("USB");

        let mut properties_plist = Vec::new();
        plist::to_writer_binary(&mut properties_plist, properties)
            .context("Failed to serialize device properties to binary plist")?;

        entries.push(UsbmuxDeviceEntry {
            udid,
            transport: transport_of(connection_type),
            properties_plist,
        });
    }

    Ok(entries)
}

/// Maps the `ConnectionType` string usbmuxd reports to a transport.
fn transport_of(connection_type: &str) -> DeviceTransport {
    if connection_type.eq_ignore_ascii_case("USB") {
        DeviceTransport::Usb
    } else if connection_type.eq_ignore_ascii_case("Network") {
        DeviceTransport::Wifi
    } else {
        DeviceTransport::Other
    }
}
