use anyhow::{Result, bail};

use crate::backend::{DeviceSession, list_usbmux_devices, open_session, read_device_identity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DeviceTransport {
    Usb,
    Wifi,
    Other,
}

impl DeviceTransport {
    pub fn label(self) -> &'static str {
        match self {
            Self::Usb => "USB",
            Self::Wifi => "WiFi",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ConnectionMode {
    Auto,
    Usb,
    Wifi,
}

impl ConnectionMode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Usb, Self::Wifi];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (USB preferred)",
            Self::Usb => "USB only",
            Self::Wifi => "WiFi only",
        }
    }

    fn accepts(self, transport: DeviceTransport) -> bool {
        match self {
            Self::Auto => true,
            Self::Usb => transport == DeviceTransport::Usb,
            Self::Wifi => transport == DeviceTransport::Wifi,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceInfo {
    pub udid: String,
    pub name: String,
    pub product_type: String,
    pub ios_version: String,
    pub build_version: String,
    pub transports: Vec<DeviceTransport>,
}

impl DeviceInfo {
    pub fn has_transport(&self, transport: DeviceTransport) -> bool {
        self.transports.contains(&transport)
    }

    pub fn supports(&self, mode: ConnectionMode) -> bool {
        self.transports.iter().copied().any(|transport| mode.accepts(transport))
    }

    pub fn transport_summary(&self) -> String {
        self.transports
            .iter()
            .map(|transport| transport.label())
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

impl std::fmt::Display for DeviceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}, iOS {} [{}]) [{}]",
            self.name,
            self.product_type,
            self.ios_version,
            self.build_version,
            self.transport_summary(),
        )
    }
}

#[derive(Clone)]
pub struct UsbmuxDeviceEntry {
    pub udid: String,
    pub transport: DeviceTransport,
    /// Raw device properties as usbmuxd reported them.
    ///
    /// Only the Windows backend reads them: `AMDeviceCreateFromProperties` is
    /// the sole way to reach a device there. libimobiledevice looks devices up
    /// by UDID, so the Linux backend leaves this empty.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub properties_plist: Vec<u8>,
}

pub fn list_connected_devices() -> Result<Vec<DeviceInfo>> {
    let mut result = Vec::new();

    for entry in list_usbmux_devices()? {
        let Some(identity) = read_device_identity(&entry)? else {
            continue;
        };

        merge_device_info(&mut result, DeviceInfo {
            udid: entry.udid,
            name: identity.name,
            product_type: identity.product_type,
            ios_version: identity.ios_version,
            build_version: identity.build_version,
            transports: vec![entry.transport],
        });
    }

    Ok(result)
}

/// Opens a session with the first device that matches `target_udid` and `mode`.
pub fn open_active_session(
    target_udid: Option<&str>,
    mode: ConnectionMode,
) -> Result<Box<dyn DeviceSession>> {
    let candidates = ordered_candidates(list_usbmux_devices()?, target_udid, mode);
    if candidates.is_empty() {
        let target = target_udid.unwrap_or("any paired iPhone");
        bail!(
            "No {} connection is available for {}. For WiFi, pair once over USB, enable WiFi sync, then keep both devices on the same network.",
            mode.label(),
            target
        );
    }

    let mut failures = Vec::new();
    for entry in candidates {
        match open_session(&entry) {
            Ok(session) => return Ok(session),
            Err(err) => failures.push(format!("{}: {err:#}", entry.transport.label())),
        }
    }

    bail!("Could not open iPhone session. {}", failures.join("; "))
}

fn merge_device_info(devices: &mut Vec<DeviceInfo>, incoming: DeviceInfo) {
    if let Some(existing) = devices
        .iter_mut()
        .find(|device| device.udid.eq_ignore_ascii_case(&incoming.udid))
    {
        for transport in incoming.transports {
            if !existing.transports.contains(&transport) {
                existing.transports.push(transport);
            }
        }
        existing.transports.sort_by_key(|transport| transport_priority(*transport));

        if existing.name == "iPhone" && incoming.name != "iPhone" {
            existing.name = incoming.name;
        }
        if existing.product_type == "iPhone" && incoming.product_type != "iPhone" {
            existing.product_type = incoming.product_type;
        }
        if existing.ios_version == "Unknown" && incoming.ios_version != "Unknown" {
            existing.ios_version = incoming.ios_version;
        }
        if existing.build_version == "Unknown" && incoming.build_version != "Unknown" {
            existing.build_version = incoming.build_version;
        }
        return;
    }

    devices.push(incoming);
}

fn transport_priority(transport: DeviceTransport) -> u8 {
    match transport {
        DeviceTransport::Usb => 0,
        DeviceTransport::Wifi => 1,
        DeviceTransport::Other => 2,
    }
}

fn ordered_candidates(
    mut entries: Vec<UsbmuxDeviceEntry>,
    target_udid: Option<&str>,
    mode: ConnectionMode,
) -> Vec<UsbmuxDeviceEntry> {
    entries.retain(|entry| {
        target_udid
            .map(|target| entry.udid.eq_ignore_ascii_case(target))
            .unwrap_or(true)
            && mode.accepts(entry.transport)
    });
    entries.sort_by_key(|entry| transport_priority(entry.transport));
    entries
}

pub fn ensure_transport_available(
    udid: &str,
    transport: DeviceTransport,
) -> Result<()> {
    let available = list_usbmux_devices()?.into_iter().any(|entry| {
        entry.udid.eq_ignore_ascii_case(udid) && entry.transport == transport
    });
    if available {
        return Ok(());
    }

    bail!(
        "iPhone {} is no longer available over {}. Refresh devices and reconnect before retrying.",
        udid,
        transport.label()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(udid: &str, transport: DeviceTransport) -> UsbmuxDeviceEntry {
        UsbmuxDeviceEntry {
            udid: udid.to_string(),
            transport,
            properties_plist: Vec::new(),
        }
    }

    #[test]
    fn test_connection_mode_candidate_order() {
        let entries = vec![
            entry("phone", DeviceTransport::Wifi),
            entry("other", DeviceTransport::Usb),
            entry("phone", DeviceTransport::Usb),
        ];

        let auto = ordered_candidates(entries.clone(), Some("phone"), ConnectionMode::Auto);
        assert_eq!(auto.len(), 2);
        assert_eq!(auto[0].transport, DeviceTransport::Usb);
        assert_eq!(auto[1].transport, DeviceTransport::Wifi);

        let wifi = ordered_candidates(entries, Some("phone"), ConnectionMode::Wifi);
        assert_eq!(wifi.len(), 1);
        assert_eq!(wifi[0].transport, DeviceTransport::Wifi);
    }

    #[test]
    fn test_merge_device_transports() {
        let mut devices = vec![DeviceInfo {
            udid: "phone".to_string(),
            name: "iPhone".to_string(),
            product_type: "iPhone".to_string(),
            ios_version: "Unknown".to_string(),
            build_version: "Unknown".to_string(),
            transports: vec![DeviceTransport::Wifi],
        }];

        merge_device_info(
            &mut devices,
            DeviceInfo {
                udid: "PHONE".to_string(),
                name: "LeeSa's iPhone".to_string(),
                product_type: "iPhone17,1".to_string(),
                ios_version: "18.6".to_string(),
                build_version: "22G86".to_string(),
                transports: vec![DeviceTransport::Usb],
            },
        );

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "LeeSa's iPhone");
        assert_eq!(
            devices[0].transports,
            vec![DeviceTransport::Usb, DeviceTransport::Wifi]
        );
    }

    #[test]
    fn test_usbmux_query() {
        match list_usbmux_devices() {
            Ok(devs) => {
                println!("Detected {} usbmux device(s)", devs.len());
                if !devs.is_empty() {
                    println!("Detected usbmux device: {}", devs[0].udid);
                }
            }
            Err(e) => {
                println!("usbmuxd not running on this host (expected in CI): {e}");
            }
        }
    }

    #[test]
    fn test_list_connected_devices() {
        match list_connected_devices() {
            Ok(devs) => {
                for d in &devs {
                    println!("Connected iPhone: {}", d);
                }
            }
            Err(e) => {
                println!("Device support not installed on this host (expected in CI): {e}");
            }
        }
    }
}
